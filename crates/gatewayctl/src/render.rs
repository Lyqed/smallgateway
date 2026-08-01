//! Rendered-manifest compilation: config repo directory -> per-node
//! `RenderedSnapshot` (docs/07-control-plane.md, "Truth in Git" +
//! "Rendered-manifest compilation").
//!
//! Rendering is `compose + validate + stamp version + hash`, and it reuses
//! gateway-core's scope composition and validation verbatim: the control plane
//! produces exactly the flat `Config` a standalone data plane would have read
//! from one hand-written file, so the node validates and serves it through the
//! unchanged `Reloader::reload` path.
//!
//! ## The repo layout (M1)
//!
//! The directory structure mirrors the four scopes (docs/07, "Repo layout
//! mirrors the four scopes"). M1 assembles the fragments into one flat
//! `Config` document rather than integrating libgit2 — the Git layer sits
//! ABOVE this and is deferred (see crates/gatewayctl/README.md). A plain
//! directory is the repo for M1; `source_commit` is a content-derived id.
//!
//! ```text
//! <repo>/
//!   providers.yaml            # the `providers:` map (fleet-wide provider refs)
//!   rejections.yaml           # the mandatory GB-4 `rejections:` block
//!   auth.yaml                 # optional `auth:` block
//!   fleet/base.chain.yaml     # the fleet-scope `attribution:`/`labels:`
//!   projects/<p>/base.chain.yaml   # a project scope
//!   routes/<name>.route.yaml  # one `routes:` entry per file
//!   apps.yaml                 # optional `apps:` block
//! ```
//!
//! ## Determinism (the six-month rule, mechanical)
//!
//! Compilation is a pure function of the repo bytes. Files are read in a fixed
//! (sorted) order, assembled into a `BTreeMap`-backed intermediate, and
//! serialized canonically (serde_yaml over sorted keys). No wall-clock, no
//! external lookups, no randomness. Same repo -> same flat bytes -> same
//! `render_hash`, forever (docs/07, "Reproducible from a commit hash").
//! `compiled_at` is stamped OUTSIDE the hashed bytes so it never perturbs the
//! hash.

use std::path::{Path, PathBuf};

use gateway_core::config::Config;
use gateway_core::snapshot::content_hash;
use gateway_proto::RenderedSnapshot;

/// A validation or IO failure while rendering the repo. The `reason` string is
/// carried verbatim into logs and (on the node side) into a NACK.
#[derive(Debug)]
pub enum RenderError {
    Io(String),
    /// The assembled flat config did not validate. Carries the precise,
    /// collected gateway-core errors.
    Invalid(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Io(e) => write!(f, "config repo IO error: {e}"),
            RenderError::Invalid(e) => write!(f, "rendered config invalid: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// The immutable result of rendering a repo: the canonical flat config bytes,
/// its hash, and the synthetic commit id — everything a `RenderedSnapshot`
/// needs except the per-node `fleet_version` (assigned at delivery, per node).
#[derive(Debug, Clone)]
pub struct Rendered {
    /// Canonical flat `Config` YAML bytes — what ships in
    /// `RenderedSnapshot.config` and what the node re-parses with
    /// `Config::from_yaml`.
    pub config_bytes: Vec<u8>,
    /// SHA-256 (lowercase hex) of `config_bytes` — the RENDERED bytes, not the
    /// source fragments (docs/07's stated distinction).
    pub render_hash: String,
    /// Content-derived repo id (M1 stand-in for a Git commit hash). Stable for
    /// identical repo content.
    pub source_commit: String,
}

impl Rendered {
    /// Build the wire snapshot for one node at one delivered version. The
    /// bytes and hash are node-independent in M1 (selectors are a Phase 5
    /// stub, so every node gets the same rendered config); `node_id` and
    /// `fleet_version` are the per-node addressing.
    pub fn to_snapshot(&self, node_id: &str, fleet_version: u64, compiled_at: i64) -> RenderedSnapshot {
        RenderedSnapshot {
            node_id: node_id.to_string(),
            source_commit: self.source_commit.clone(),
            render_hash: self.render_hash.clone(),
            fleet_version,
            config: self.config_bytes.clone(),
            compiled_at,
        }
    }
}

/// Load, compose, validate, and canonically serialize the repo at `root`.
///
/// The returned `config_bytes` are guaranteed to parse and validate via
/// `Config::from_yaml` (this function calls it as the render-time gate — a repo
/// that would produce an invalid flat config fails HERE, in the control plane,
/// before any node sees it).
pub fn render_repo(root: &Path) -> Result<Rendered, RenderError> {
    let flat = assemble_flat_yaml(root)?;

    // Render-time validation gate: the exact gateway-core composition +
    // validation the node will re-run. If it fails here, the operator's repo is
    // broken and nothing is delivered.
    Config::from_yaml(&flat).map_err(|e| RenderError::Invalid(e.to_string()))?;

    let render_hash = content_hash(&flat);
    // M1 synthetic commit id: the hash of the canonical bytes, prefixed so it
    // is visibly not a real Git sha. The Git layer replaces this with the
    // actual commit; every consumer only relies on "stable for identical repo
    // content", which this satisfies.
    let source_commit = format!("m1-{}", &render_hash[..16]);

    Ok(Rendered {
        config_bytes: flat.into_bytes(),
        render_hash,
        source_commit,
    })
}

/// Read the repo fragments and assemble them into one canonical flat `Config`
/// YAML string. Pure over the repo bytes: fixed read order, sorted keys.
fn assemble_flat_yaml(root: &Path) -> Result<String, RenderError> {
    if !root.is_dir() {
        return Err(RenderError::Io(format!(
            "config repo {} is not a directory",
            root.display()
        )));
    }

    // A serde_yaml::Value tree, built deterministically, then serialized. Using
    // Value (not string concatenation) keeps the output canonical regardless of
    // fragment formatting — serde_yaml emits mapping keys in insertion order, so
    // we insert in a fixed order and let nested maps stay BTreeMap-ordered.
    let mut doc = serde_yaml::Mapping::new();

    // providers: (required)
    let providers = read_yaml_mapping(&root.join("providers.yaml"), "providers.yaml")?;
    doc.insert("providers".into(), sorted_value(providers));

    // fleet: (optional scope)
    let fleet_path = root.join("fleet").join("base.chain.yaml");
    if fleet_path.exists() {
        let fleet = read_yaml_mapping(&fleet_path, "fleet/base.chain.yaml")?;
        doc.insert("fleet".into(), sorted_value(fleet));
    }

    // projects: (optional; one directory per project)
    let projects = read_projects(root)?;
    if !projects.is_empty() {
        let mut pm = serde_yaml::Mapping::new();
        for (name, val) in projects {
            pm.insert(name.into(), val);
        }
        doc.insert("projects".into(), serde_yaml::Value::Mapping(pm));
    }

    // routes: (required; one file per route, sorted by filename for order)
    let routes = read_routes(root)?;
    if routes.is_empty() {
        return Err(RenderError::Io(format!(
            "config repo {} has no routes/*.route.yaml files",
            root.display()
        )));
    }
    doc.insert("routes".into(), serde_yaml::Value::Sequence(routes));

    // apps: (optional)
    let apps_path = root.join("apps.yaml");
    if apps_path.exists() {
        let apps = read_yaml_mapping(&apps_path, "apps.yaml")?;
        doc.insert("apps".into(), sorted_value(apps));
    }

    // rejections: (required GB-4 block)
    let rejections = read_yaml_mapping(&root.join("rejections.yaml"), "rejections.yaml")?;
    doc.insert("rejections".into(), sorted_value(rejections));

    // auth: (optional)
    let auth_path = root.join("auth.yaml");
    if auth_path.exists() {
        let auth = read_yaml_mapping(&auth_path, "auth.yaml")?;
        doc.insert("auth".into(), sorted_value(auth));
    }

    serde_yaml::to_string(&serde_yaml::Value::Mapping(doc))
        .map_err(|e| RenderError::Io(format!("serializing flat config: {e}")))
}

/// One project directory -> its scope Value. Each `projects/<name>/` contributes
/// a `base.chain.yaml` (the project scope). Read sorted by project name.
fn read_projects(root: &Path) -> Result<Vec<(String, serde_yaml::Value)>, RenderError> {
    let dir = root.join("projects");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut names: Vec<PathBuf> = list_dir(&dir)?
        .into_iter()
        .filter(|p| p.is_dir())
        .collect();
    names.sort();
    let mut out = Vec::new();
    for pdir in names {
        let name = pdir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| RenderError::Io(format!("bad project dir name: {}", pdir.display())))?
            .to_string();
        let chain = pdir.join("base.chain.yaml");
        if chain.exists() {
            let scope = read_yaml_mapping(&chain, "project base.chain.yaml")?;
            out.push((name, sorted_value(scope)));
        }
    }
    Ok(out)
}

/// One `routes/*.route.yaml` file -> one `routes:` sequence entry. Sorted by
/// filename so route order is deterministic and reviewable.
fn read_routes(root: &Path) -> Result<Vec<serde_yaml::Value>, RenderError> {
    let dir = root.join("routes");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = list_dir(&dir)?
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".route.yaml"))
        })
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let val = read_yaml_value(&f, "route file")?;
        out.push(sorted_value_any(val));
    }
    Ok(out)
}

fn list_dir(dir: &Path) -> Result<Vec<PathBuf>, RenderError> {
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(dir).map_err(|e| RenderError::Io(format!("{}: {e}", dir.display())))?
    {
        let entry = entry.map_err(|e| RenderError::Io(format!("{}: {e}", dir.display())))?;
        out.push(entry.path());
    }
    Ok(out)
}

fn read_yaml_value(path: &Path, what: &str) -> Result<serde_yaml::Value, RenderError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| RenderError::Io(format!("{what} {}: {e}", path.display())))?;
    serde_yaml::from_str(&text)
        .map_err(|e| RenderError::Io(format!("{what} {}: parse: {e}", path.display())))
}

fn read_yaml_mapping(path: &Path, what: &str) -> Result<serde_yaml::Mapping, RenderError> {
    match read_yaml_value(path, what)? {
        serde_yaml::Value::Mapping(m) => Ok(m),
        other => Err(RenderError::Io(format!(
            "{what} {}: expected a mapping, got {other:?}",
            path.display()
        ))),
    }
}

/// Canonicalize a mapping: recursively sort every mapping's keys. This is what
/// makes `render_hash` reproducible regardless of source key order or
/// whitespace.
fn sorted_value(m: serde_yaml::Mapping) -> serde_yaml::Value {
    sorted_value_any(serde_yaml::Value::Mapping(m))
}

fn sorted_value_any(v: serde_yaml::Value) -> serde_yaml::Value {
    match v {
        serde_yaml::Value::Mapping(m) => {
            let mut entries: Vec<(serde_yaml::Value, serde_yaml::Value)> = m.into_iter().collect();
            entries.sort_by_key(|(k, _)| yaml_key_str(k));
            let mut out = serde_yaml::Mapping::new();
            for (k, val) in entries {
                out.insert(k, sorted_value_any(val));
            }
            serde_yaml::Value::Mapping(out)
        }
        // Sequences keep their order (route/list order is semantic); only
        // recurse into their elements.
        serde_yaml::Value::Sequence(seq) => {
            serde_yaml::Value::Sequence(seq.into_iter().map(sorted_value_any).collect())
        }
        scalar => scalar,
    }
}

fn yaml_key_str(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// A tiny helper for tests and callers that want the parsed flat config back.
pub fn parse_rendered(bytes: &[u8]) -> Result<Config, RenderError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| RenderError::Io(format!("rendered config is not utf-8: {e}")))?;
    Config::from_yaml(text).map_err(|e| RenderError::Invalid(e.to_string()))
}

/// A shared minimal-but-complete repo builder for tests across the crate.
/// Available to integration tests via the `test-support` feature (default-on);
/// it only constructs throwaway temp repos, so it is harmless in any build.
#[cfg(any(test, feature = "test-support"))]
pub mod testrepo {
    use std::path::PathBuf;

    /// Write a minimal valid config repo into a fresh temp dir and return its
    /// root. `env` is the fleet-pinned env value — the visible knob a "v2"
    /// change flips.
    pub fn write(env: &str) -> PathBuf {
        write_named(&format!("gatewayctl-repo-{env}"), env)
    }

    pub fn write_named(tag: &str, env: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("fleet")).unwrap();
        std::fs::create_dir_all(root.join("routes")).unwrap();

        std::fs::write(
            root.join("providers.yaml"),
            "openai-main:\n  kind: openai\n  upstream: { host: 127.0.0.1, port: 6190 }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("fleet").join("base.chain.yaml"),
            format!("attribution:\n  required_keys: [team]\n  pinned: {{ env: {env} }}\n"),
        )
        .unwrap();
        std::fs::write(
            root.join("routes").join("00-openai.route.yaml"),
            "prefix: /openai\nprovider: openai-main\n",
        )
        .unwrap();
        std::fs::write(
            root.join("rejections.yaml"),
            concat!(
                "missing_attribution:\n",
                "  status: 428\n",
                "  content_type: application/json\n",
                "  body: '{\"error\":\"missing {{key}} on {{route}}\"}'\n",
                "unknown_route:\n",
                "  status: 404\n",
                "  content_type: application/json\n",
                "  body: '{\"error\":\"no route for {{route}}\"}'\n",
            ),
        )
        .unwrap();
        root
    }

    /// Write a repo whose flat config FAILS validation (a route references a
    /// provider that does not exist). Assembly succeeds; the render-time
    /// `Config::from_yaml` gate rejects it — exactly the case a node NACKs.
    pub fn write_invalid() -> PathBuf {
        let root = write_named("gatewayctl-repo-invalid", "prod");
        std::fs::write(
            root.join("routes").join("00-openai.route.yaml"),
            "prefix: /openai\nprovider: does-not-exist\n",
        )
        .unwrap();
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_repo_renders_the_same_hash_deterministically() {
        let root = testrepo::write("prod");
        let a = render_repo(&root).unwrap();
        let b = render_repo(&root).unwrap();
        assert_eq!(a.render_hash, b.render_hash, "render is a pure function");
        assert_eq!(a.source_commit, b.source_commit);
        assert_eq!(a.config_bytes, b.config_bytes);
        assert_eq!(a.render_hash.len(), 64);
    }

    #[test]
    fn different_content_renders_a_different_hash() {
        let prod = render_repo(&testrepo::write("prod")).unwrap();
        let canary = render_repo(&testrepo::write("canary")).unwrap();
        assert_ne!(prod.render_hash, canary.render_hash);
    }

    #[test]
    fn rendered_bytes_parse_and_validate_as_a_config() {
        let r = render_repo(&testrepo::write("prod")).unwrap();
        let cfg = parse_rendered(&r.config_bytes).unwrap();
        assert_eq!(cfg.routes[0].policy().pinned["env"], "prod");
        assert_eq!(cfg.routes[0].policy().required_keys, vec!["team"]);
    }

    #[test]
    fn an_invalid_repo_fails_at_render_time_not_at_the_node() {
        let err = render_repo(&testrepo::write_invalid()).unwrap_err();
        assert!(matches!(err, RenderError::Invalid(_)), "{err:?}");
    }

    #[test]
    fn key_order_in_fragments_does_not_change_the_hash() {
        // Two repos identical except provider-map key order inside a fragment
        // must hash the same — canonicalization sorts keys.
        let a = testrepo::write_named("order-a", "prod");
        let b = testrepo::write_named("order-b", "prod");
        // Rewrite b's providers with an extra provider inserted "out of order";
        // then rewrite a with the same two providers in the other order.
        let two_ordered = "aaa-main:\n  kind: openai\n  upstream: { host: 127.0.0.1, port: 6190 }\nopenai-main:\n  kind: openai\n  upstream: { host: 127.0.0.1, port: 6190 }\n";
        let two_reversed = "openai-main:\n  kind: openai\n  upstream: { host: 127.0.0.1, port: 6190 }\naaa-main:\n  kind: openai\n  upstream: { host: 127.0.0.1, port: 6190 }\n";
        std::fs::write(a.join("providers.yaml"), two_ordered).unwrap();
        std::fs::write(b.join("providers.yaml"), two_reversed).unwrap();
        let ra = render_repo(&a).unwrap();
        let rb = render_repo(&b).unwrap();
        assert_eq!(ra.render_hash, rb.render_hash);
    }
}

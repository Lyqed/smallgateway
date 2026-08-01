//! Rendered-manifest compilation: a resolved config repo -> per-node
//! `RenderedSnapshot` (docs/07-control-plane.md, "Truth in Git" +
//! "Rendered-manifest compilation").
//!
//! Rendering is `compose + validate + stamp version + hash`, and it reuses
//! gateway-core's scope composition and validation verbatim: the control plane
//! produces exactly the flat `Config` a standalone data plane would have read
//! from one hand-written file, so the node validates and serves it through the
//! unchanged `Reloader::reload` path.
//!
//! ## The repo layout
//!
//! The directory structure mirrors the four scopes (docs/07, "Repo layout
//! mirrors the four scopes"). The same layout is read from a loose directory
//! ([`crate::source::DirectorySource`]) or from a Git commit
//! ([`crate::source::GitSource`]) — both resolve to a
//! [`crate::source::ResolvedRepo`], and rendering is defined over THAT, never
//! over a live filesystem. So the milestone-1 directory path and the
//! milestone-2 Git path feed byte-identical inputs into byte-identical
//! assembly.
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
//! Compilation is a pure function of the resolved bytes. Files are read in a
//! fixed (sorted) order, assembled into an intermediate, and serialized
//! canonically (serde_yaml over sorted keys). No wall-clock, no external
//! lookups, no randomness. Same content -> same flat bytes -> same
//! `render_hash`, forever (docs/07, "Reproducible from a commit hash"). The
//! `RenderedSnapshot` records the `source_commit` the resolved repo came from —
//! a real Git SHA under the Git source — so re-rendering that recorded commit
//! reproduces the identical bytes and hash. `compiled_at` is stamped OUTSIDE
//! the hashed bytes so it never perturbs the hash.

use std::path::Path;

use gateway_core::config::Config;
use gateway_core::snapshot::content_hash;
use gateway_proto::RenderedSnapshot;

use crate::source::{ConfigSource, DirectorySource, ResolvedRepo, SourceError};

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

impl From<SourceError> for RenderError {
    fn from(e: SourceError) -> RenderError {
        RenderError::Io(e.to_string())
    }
}

/// The immutable result of rendering a repo: the canonical flat config bytes,
/// its hash, and the source commit — everything a `RenderedSnapshot` needs
/// except the per-node `fleet_version` (assigned at delivery, per node).
#[derive(Debug, Clone)]
pub struct Rendered {
    /// Canonical flat `Config` YAML bytes — what ships in
    /// `RenderedSnapshot.config` and what the node re-parses with
    /// `Config::from_yaml`.
    pub config_bytes: Vec<u8>,
    /// SHA-256 (lowercase hex) of `config_bytes` — the RENDERED bytes, not the
    /// source fragments (docs/07's stated distinction).
    pub render_hash: String,
    /// The exact source state this render is reproducible from: a real Git
    /// commit SHA (Git source) or a content-derived id (directory source).
    pub source_commit: String,
}

impl Rendered {
    /// Build the wire snapshot for one node at one delivered version. The
    /// bytes and hash are node-independent here (selectors are a Phase 5 stub,
    /// so every node gets the same rendered config); `node_id` and
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

/// Render a config source: resolve it, then compile the resolved bytes. This is
/// the source-agnostic entry point — the Git-sourced rollout and the directory
/// demo both call it, so the reproducibility property holds across sources.
pub fn render_source(source: &dyn ConfigSource) -> Result<Rendered, RenderError> {
    let resolved = source.resolve()?;
    render_resolved(&resolved)
}

/// Render an already-resolved repo. Compilation is a pure function of the
/// resolved bytes and the recorded `source_commit`.
pub fn render_resolved(resolved: &ResolvedRepo) -> Result<Rendered, RenderError> {
    let flat = assemble_flat_yaml(resolved)?;

    // Render-time validation gate: the exact gateway-core composition +
    // validation the node will re-run. If it fails here, the operator's repo is
    // broken and nothing is delivered.
    Config::from_yaml(&flat).map_err(|e| RenderError::Invalid(e.to_string()))?;

    let render_hash = content_hash(&flat);
    Ok(Rendered {
        config_bytes: flat.into_bytes(),
        render_hash,
        source_commit: resolved.source_commit.clone(),
    })
}

/// Render a loose directory (the milestone-1 convenience path). Thin wrapper
/// over [`render_source`] with a [`DirectorySource`], kept because the demo and
/// many tests address a directory directly.
pub fn render_repo(root: &Path) -> Result<Rendered, RenderError> {
    render_source(&DirectorySource::new(root))
}

/// Read the resolved fragments and assemble them into one canonical flat
/// `Config` YAML string. Pure over the resolved bytes: fixed read order (the
/// ResolvedRepo is pre-sorted), sorted keys.
fn assemble_flat_yaml(repo: &ResolvedRepo) -> Result<String, RenderError> {
    // A serde_yaml::Value tree, built deterministically, then serialized. Using
    // Value (not string concatenation) keeps the output canonical regardless of
    // fragment formatting — we insert in a fixed order and recursively sort keys.
    let mut doc = serde_yaml::Mapping::new();

    // providers: (required)
    let providers = read_yaml_mapping(repo, "providers.yaml")?
        .ok_or_else(|| RenderError::Io("config repo has no providers.yaml".to_string()))?;
    doc.insert("providers".into(), sorted_value(providers));

    // fleet: (optional scope)
    if let Some(fleet) = read_yaml_mapping(repo, "fleet/base.chain.yaml")? {
        doc.insert("fleet".into(), sorted_value(fleet));
    }

    // projects: (optional; one directory per project)
    let projects = read_projects(repo)?;
    if !projects.is_empty() {
        let mut pm = serde_yaml::Mapping::new();
        for (name, val) in projects {
            pm.insert(name.into(), val);
        }
        doc.insert("projects".into(), serde_yaml::Value::Mapping(pm));
    }

    // routes: (required; one file per route, sorted by filename for order)
    let routes = read_routes(repo)?;
    if routes.is_empty() {
        return Err(RenderError::Io(
            "config repo has no routes/*.route.yaml files".to_string(),
        ));
    }
    doc.insert("routes".into(), serde_yaml::Value::Sequence(routes));

    // apps: (optional)
    if let Some(apps) = read_yaml_mapping(repo, "apps.yaml")? {
        doc.insert("apps".into(), sorted_value(apps));
    }

    // rejections: (required GB-4 block)
    let rejections = read_yaml_mapping(repo, "rejections.yaml")?
        .ok_or_else(|| RenderError::Io("config repo has no rejections.yaml".to_string()))?;
    doc.insert("rejections".into(), sorted_value(rejections));

    // auth: (optional)
    if let Some(auth) = read_yaml_mapping(repo, "auth.yaml")? {
        doc.insert("auth".into(), sorted_value(auth));
    }

    serde_yaml::to_string(&serde_yaml::Value::Mapping(doc))
        .map_err(|e| RenderError::Io(format!("serializing flat config: {e}")))
}

/// One project directory -> its scope Value. Each `projects/<name>/`
/// contributes a `base.chain.yaml` (the project scope). Read sorted by name.
fn read_projects(repo: &ResolvedRepo) -> Result<Vec<(String, serde_yaml::Value)>, RenderError> {
    // Collect the distinct project names from `projects/<name>/...` paths.
    let mut names: Vec<String> = Vec::new();
    for (rel, _) in repo.entries_under("projects") {
        if let Some((name, _)) = rel.split_once('/') {
            if !names.contains(&name.to_string()) {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let chain_path = format!("projects/{name}/base.chain.yaml");
        if let Some(scope) = read_yaml_mapping(repo, &chain_path)? {
            out.push((name, sorted_value(scope)));
        }
    }
    Ok(out)
}

/// One `routes/*.route.yaml` file -> one `routes:` sequence entry. Sorted by
/// filename so route order is deterministic and reviewable.
fn read_routes(repo: &ResolvedRepo) -> Result<Vec<serde_yaml::Value>, RenderError> {
    let mut files: Vec<(String, &[u8])> = repo
        .entries_under("routes")
        .into_iter()
        .filter(|(name, _)| name.ends_with(".route.yaml"))
        .collect();
    files.sort_by(|(a, _), (b, _)| a.cmp(b));
    let mut out = Vec::new();
    for (name, bytes) in files {
        let val = parse_yaml_value(bytes, &format!("routes/{name}"))?;
        out.push(sorted_value_any(val));
    }
    Ok(out)
}

/// Parse a repo file's bytes as a YAML Value; `None` if the file is absent.
fn read_yaml_mapping(
    repo: &ResolvedRepo,
    rel: &str,
) -> Result<Option<serde_yaml::Mapping>, RenderError> {
    let Some(bytes) = repo.get(rel) else {
        return Ok(None);
    };
    match parse_yaml_value(bytes, rel)? {
        serde_yaml::Value::Mapping(m) => Ok(Some(m)),
        other => Err(RenderError::Io(format!(
            "{rel}: expected a mapping, got {other:?}"
        ))),
    }
}

fn parse_yaml_value(bytes: &[u8], what: &str) -> Result<serde_yaml::Value, RenderError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| RenderError::Io(format!("{what}: not utf-8: {e}")))?;
    serde_yaml::from_str(text).map_err(|e| RenderError::Io(format!("{what}: parse: {e}")))
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
        write_files(&root, env);
        root
    }

    /// Write the minimal-valid repo fragments into an existing directory. Used
    /// by the directory builder above and by the Git-source tests (which then
    /// `git add && git commit` the directory).
    pub fn write_files(root: &std::path::Path, env: &str) {
        let _ = std::fs::create_dir_all(root.join("fleet"));
        let _ = std::fs::create_dir_all(root.join("routes"));
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
        let two_ordered = "aaa-main:\n  kind: openai\n  upstream: { host: 127.0.0.1, port: 6190 }\nopenai-main:\n  kind: openai\n  upstream: { host: 127.0.0.1, port: 6190 }\n";
        let two_reversed = "openai-main:\n  kind: openai\n  upstream: { host: 127.0.0.1, port: 6190 }\naaa-main:\n  kind: openai\n  upstream: { host: 127.0.0.1, port: 6190 }\n";
        std::fs::write(a.join("providers.yaml"), two_ordered).unwrap();
        std::fs::write(b.join("providers.yaml"), two_reversed).unwrap();
        let ra = render_repo(&a).unwrap();
        let rb = render_repo(&b).unwrap();
        assert_eq!(ra.render_hash, rb.render_hash);
    }
}

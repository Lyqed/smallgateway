//! Versioned config snapshots (Phase 1, milestone 2: hot swap).
//!
//! Rendering = load + validate + stamp: a [`Snapshot`] wraps a validated
//! [`Config`] with a monotonically increasing version, the source path, and
//! a content hash. Validation stays fail-fast — a file that does not
//! validate never becomes a snapshot, and never consumes a version number,
//! so the version sequence of *accepted* snapshots has no gaps.
//!
//! The hash is the identity of the rendered content: a reload whose bytes
//! hash to the active snapshot's hash is a no-op by definition, and the
//! hash in every swap log line ties a running version back to the exact
//! file content that produced it (docs/03-hot-swap.md: reviewable
//! snapshots, bounded staleness stated, never hidden).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use crate::config::{Config, ConfigError};

/// How many hash characters the log lines carry; the full digest stays on
/// the snapshot for exact comparison.
const SHORT_HASH_LEN: usize = 12;

/// One immutable, validated rendering of the config file. Everything a
/// request needs lives behind one `Arc<Snapshot>`: binding the Arc once at
/// request start makes a torn read (two versions seen by one request)
/// impossible by construction.
#[derive(Debug)]
pub struct Snapshot {
    pub config: Config,
    /// Monotonically increasing, stamped only on successful validation.
    pub version: u64,
    /// The file this snapshot was rendered from.
    pub source: PathBuf,
    /// SHA-256 of the source bytes, lowercase hex.
    pub content_hash: String,
}

impl Snapshot {
    /// Log-friendly hash prefix (the full hash is `content_hash`).
    pub fn short_hash(&self) -> &str {
        &self.content_hash[..SHORT_HASH_LEN.min(self.content_hash.len())]
    }
}

/// SHA-256 of the config text, lowercase hex — the no-op check and the
/// audit link between a log line and the bytes that produced it.
pub fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Owns the monotonic version counter. One renderer per process: every
/// snapshot it accepts gets the next version, and a rejected render leaves
/// the counter untouched.
#[derive(Debug, Default)]
pub struct Renderer {
    next: AtomicU64,
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer::default()
    }

    /// Load + validate + stamp, from disk.
    pub fn render_file(&self, path: &Path) -> Result<Snapshot, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Io(format!("{}: {e}", path.display())))?;
        self.render_text(&text, path)
    }

    /// Validate + stamp from already-read text (the reload path reads the
    /// file once for its hash check and renders from the same bytes).
    pub fn render_text(&self, text: &str, source: &Path) -> Result<Snapshot, ConfigError> {
        // Fail-fast BEFORE stamping: an invalid config consumes no version.
        let config = Config::from_yaml(text)?;
        let version = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(Snapshot {
            config,
            version,
            source: source.to_path_buf(),
            content_hash: content_hash(text),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_yaml(env: &str) -> String {
        format!(
            r#"
providers:
  openai-main:
    kind: openai
    upstream: {{ host: 127.0.0.1, port: 6190 }}
routes:
  - prefix: /openai
    provider: openai-main
    attribution:
      pinned: {{ env: {env} }}
rejections:
  missing_attribution:
    status: 428
    content_type: application/json
    body: '{{"error":"missing {{{{key}}}} on {{{{route}}}}"}}'
  unknown_route:
    status: 404
    content_type: application/json
    body: '{{"error":"no route for {{{{route}}}}"}}'
"#
        )
    }

    #[test]
    fn rendering_stamps_monotonically_increasing_versions() {
        let renderer = Renderer::new();
        let path = Path::new("gateway.yaml");
        let a = renderer.render_text(&valid_yaml("prod"), path).unwrap();
        let b = renderer.render_text(&valid_yaml("canary"), path).unwrap();
        assert_eq!(a.version, 1);
        assert_eq!(b.version, 2);
        assert_eq!(a.source, PathBuf::from("gateway.yaml"));
    }

    #[test]
    fn rejected_render_consumes_no_version() {
        let renderer = Renderer::new();
        let path = Path::new("gateway.yaml");
        let bad = valid_yaml("prod").replace("provider: openai-main", "provider: nope");
        assert!(matches!(
            renderer.render_text(&bad, path),
            Err(ConfigError::Invalid(_))
        ));
        // The failure above burned nothing: the next accepted snapshot is v1.
        let ok = renderer.render_text(&valid_yaml("prod"), path).unwrap();
        assert_eq!(ok.version, 1);
    }

    #[test]
    fn content_hash_is_stable_for_identical_text_and_differs_otherwise() {
        let a = content_hash(&valid_yaml("prod"));
        let b = content_hash(&valid_yaml("prod"));
        let c = content_hash(&valid_yaml("canary"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64); // full sha256, lowercase hex
        assert!(a.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn snapshot_records_the_hash_of_its_own_source_text() {
        let renderer = Renderer::new();
        let text = valid_yaml("prod");
        let snap = renderer.render_text(&text, Path::new("g.yaml")).unwrap();
        assert_eq!(snap.content_hash, content_hash(&text));
        assert_eq!(snap.short_hash(), &snap.content_hash[..12]);
    }

    #[test]
    fn render_file_missing_path_is_an_io_error() {
        let renderer = Renderer::new();
        let err = renderer
            .render_file(Path::new("/nonexistent/gateway.yaml"))
            .unwrap_err();
        assert!(matches!(err, ConfigError::Io(_)), "{err:?}");
    }
}

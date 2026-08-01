//! The config source abstraction (docs/07-control-plane.md, "Truth in Git").
//!
//! Milestone 1 read the desired config from a loose directory. Milestone 2
//! makes **Git** the config truth while keeping the directory source intact,
//! behind one [`ConfigSource`] trait with two variants:
//!
//! - [`DirectorySource`]: the M1 path, a plain directory on disk. Its
//!   `source_commit` is a content-derived id (`dir-<hash-prefix>`), stable for
//!   identical content — the milestone-1 demo path is unchanged.
//! - [`GitSource`]: reads the four-scope repo at a specific **ref or commit**
//!   (`HEAD`, a branch, a tag, a full/short SHA) out of a real Git repository
//!   using the pure-Rust `gix` crate. Its `source_commit` is the resolved
//!   40-hex commit SHA, so a `RenderedSnapshot` records the exact commit it was
//!   rendered from — the six-month rule made mechanical (docs/07,
//!   "Reproducible from a commit hash").
//!
//! ## Why a resolved file-set, not a filesystem path, is the render input
//!
//! Both variants resolve to the SAME intermediate: a [`ResolvedRepo`] — a
//! deterministically-sorted set of `(relative-path, bytes)` plus a
//! `source_commit`. `render.rs` then assembles + validates + canonicalizes over
//! that byte map, never touching the filesystem itself. This is load-bearing
//! for the reproducibility property: the directory source and the Git source
//! feed byte-identical inputs into byte-identical assembly, so rendering the
//! same content two ways yields the same `render_hash`. It also means the Git
//! source reads a *historical* commit's bytes (via the object database) without
//! checking anything out to disk — resolving "what was `edge-fra-2` running at
//! 03:14" is a pure re-render of a recorded commit (docs/07).
//!
//! Only the local-repo read path is used: `gix::open` + `rev_parse` + a tree
//! walk over the object database. No network transport is compiled in (the
//! `gix` features are trimmed to `max-performance-safe` + `revision`), so this
//! adds no C toolchain and stays inside the two-binaries budget's spirit — one
//! new dependency family, pure Rust.

use std::path::{Path, PathBuf};

use gateway_core::snapshot::content_hash;

/// A resolved config repo: every config file's bytes, keyed by its
/// repo-relative path (forward-slash separated), plus the id of the exact
/// source state it came from. The render pipeline consumes only this — it is
/// source-agnostic by construction.
#[derive(Debug, Clone)]
pub struct ResolvedRepo {
    /// `(relative-path, bytes)` for every file, sorted by path. Sorting here
    /// (not at read time) makes iteration order identical across sources.
    files: Vec<(String, Vec<u8>)>,
    /// The exact source state: a real commit SHA for Git, a content-derived id
    /// for a directory. Recorded in `RenderedSnapshot.source_commit`.
    pub source_commit: String,
}

impl ResolvedRepo {
    /// Build from an unsorted `(path, bytes)` set and an explicit commit id.
    /// Paths are normalized to forward slashes and sorted so the assembled
    /// render is deterministic regardless of the source's own ordering.
    pub fn new(mut files: Vec<(String, Vec<u8>)>, source_commit: String) -> ResolvedRepo {
        for (p, _) in files.iter_mut() {
            *p = p.replace('\\', "/");
        }
        files.sort_by(|(a, _), (b, _)| a.cmp(b));
        files.dedup_by(|(a, _), (b, _)| a == b);
        ResolvedRepo {
            files,
            source_commit,
        }
    }

    /// The bytes of one repo-relative file, if present.
    pub fn get(&self, rel: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(p, _)| p == rel)
            .map(|(_, b)| b.as_slice())
    }

    /// Whether a file exists at this repo-relative path.
    pub fn contains(&self, rel: &str) -> bool {
        self.files.iter().any(|(p, _)| p == rel)
    }

    /// Every file whose path is directly under `dir/` (one level), returning
    /// `(child-name, bytes)` sorted. Used to enumerate `projects/<p>/…` and
    /// `routes/*.route.yaml` without a live filesystem.
    pub fn entries_under(&self, dir: &str) -> Vec<(String, &[u8])> {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        let mut out = Vec::new();
        for (p, b) in &self.files {
            if let Some(rest) = p.strip_prefix(&prefix) {
                if !rest.is_empty() {
                    out.push((rest.to_string(), b.as_slice()));
                }
            }
        }
        out
    }

    /// A content-derived id over the whole resolved file-set: every path and
    /// its bytes, in sorted order. The directory source uses this as its
    /// `source_commit` so identical directory content maps to a stable id.
    pub fn content_id(files: &[(String, Vec<u8>)]) -> String {
        let mut hasher_input = String::new();
        let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
        sorted.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (p, b) in sorted {
            hasher_input.push_str(p);
            hasher_input.push('\0');
            hasher_input.push_str(&content_hash(&String::from_utf8_lossy(b)));
            hasher_input.push('\n');
        }
        content_hash(&hasher_input)
    }
}

/// A failure resolving a config source into a [`ResolvedRepo`].
#[derive(Debug)]
pub enum SourceError {
    Io(String),
    /// A Git-specific failure: repo not found, ref not resolvable, object read.
    Git(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::Io(e) => write!(f, "config source IO error: {e}"),
            SourceError::Git(e) => write!(f, "config source Git error: {e}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// The config source abstraction. A source resolves the desired config into a
/// [`ResolvedRepo`]; the render pipeline is identical thereafter. `resolve` is
/// the only method a new source kind must implement.
pub trait ConfigSource: Send + Sync {
    /// Resolve the current desired config bytes + the id they came from.
    fn resolve(&self) -> Result<ResolvedRepo, SourceError>;

    /// A short, human-readable description for logs (e.g. `dir:/path` or
    /// `git:/repo@main`).
    fn describe(&self) -> String;
}

/// The M1 directory source: read every file under a directory tree.
#[derive(Debug, Clone)]
pub struct DirectorySource {
    root: PathBuf,
}

impl DirectorySource {
    pub fn new(root: impl Into<PathBuf>) -> DirectorySource {
        DirectorySource { root: root.into() }
    }
}

impl ConfigSource for DirectorySource {
    fn resolve(&self) -> Result<ResolvedRepo, SourceError> {
        if !self.root.is_dir() {
            return Err(SourceError::Io(format!(
                "config repo {} is not a directory",
                self.root.display()
            )));
        }
        let mut files = Vec::new();
        collect_dir(&self.root, &self.root, &mut files)?;
        let commit = format!("dir-{}", &ResolvedRepo::content_id(&files)[..16]);
        Ok(ResolvedRepo::new(files, commit))
    }

    fn describe(&self) -> String {
        format!("dir:{}", self.root.display())
    }
}

/// Recursively collect `(repo-relative-path, bytes)` for every regular file
/// under `dir`, relative to `base`. Directories are descended in sorted order
/// for determinism; the final sort in [`ResolvedRepo::new`] is authoritative.
fn collect_dir(
    base: &Path,
    dir: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), SourceError> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| SourceError::Io(format!("{}: {e}", dir.display())))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()
        .map_err(|e| SourceError::Io(format!("{}: {e}", dir.display())))?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            // Skip a nested .git if the directory happens to be a work tree.
            if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                continue;
            }
            collect_dir(base, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .map_err(|e| SourceError::Io(format!("relativize {}: {e}", path.display())))?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes =
                std::fs::read(&path).map_err(|e| SourceError::Io(format!("{}: {e}", path.display())))?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

/// The Git source: read the config repo at a specific ref/commit from a real
/// Git repository, out of the object database (no checkout to disk).
#[derive(Debug, Clone)]
pub struct GitSource {
    repo: PathBuf,
    /// The ref or commit to render: `HEAD`, a branch/tag name, or a SHA.
    reference: String,
}

impl GitSource {
    pub fn new(repo: impl Into<PathBuf>, reference: impl Into<String>) -> GitSource {
        GitSource {
            repo: repo.into(),
            reference: reference.into(),
        }
    }

    /// The ref/commit this source renders (for logs and CLI echo).
    pub fn reference(&self) -> &str {
        &self.reference
    }
}

impl ConfigSource for GitSource {
    fn resolve(&self) -> Result<ResolvedRepo, SourceError> {
        let repo = gix::open(&self.repo)
            .map_err(|e| SourceError::Git(format!("open {}: {e}", self.repo.display())))?;
        // Resolve the ref/commit to a single object. `rev_parse_single` accepts
        // HEAD, branch/tag names, and full/short SHAs — the full "commit hash or
        // ref" surface docs/07 requires.
        let object = repo
            .rev_parse_single(self.reference.as_str())
            .map_err(|e| SourceError::Git(format!("resolve {:?}: {e}", self.reference)))?
            .object()
            .map_err(|e| SourceError::Git(format!("read object for {:?}: {e}", self.reference)))?;
        let commit = object
            .try_into_commit()
            .map_err(|e| SourceError::Git(format!("{:?} is not a commit: {e}", self.reference)))?;
        // The resolved 40-hex SHA — the exact state this render is reproducible
        // from. Recorded verbatim in RenderedSnapshot.source_commit.
        let source_commit = commit.id().to_hex().to_string();
        let tree = commit
            .tree()
            .map_err(|e| SourceError::Git(format!("tree of {source_commit}: {e}")))?;

        // Walk the tree breadth-first and pull every blob's bytes from the ODB.
        let mut recorder = gix::traverse::tree::Recorder::default();
        tree.traverse()
            .breadthfirst(&mut recorder)
            .map_err(|e| SourceError::Git(format!("walk tree of {source_commit}: {e}")))?;

        let mut files = Vec::new();
        for entry in recorder.records {
            if !entry.mode.is_blob() {
                continue;
            }
            let path = entry
                .filepath
                .to_string()
                .replace('\\', "/");
            let obj = repo
                .find_object(entry.oid)
                .map_err(|e| SourceError::Git(format!("read blob {path}: {e}")))?;
            files.push((path, obj.data.clone()));
        }
        Ok(ResolvedRepo::new(files, source_commit))
    }

    fn describe(&self) -> String {
        format!("git:{}@{}", self.repo.display(), self.reference)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_repo_sorts_and_enumerates_children() {
        let files = vec![
            ("routes/01-b.route.yaml".to_string(), b"b".to_vec()),
            ("routes/00-a.route.yaml".to_string(), b"a".to_vec()),
            ("providers.yaml".to_string(), b"p".to_vec()),
            ("projects/x/base.chain.yaml".to_string(), b"x".to_vec()),
        ];
        let repo = ResolvedRepo::new(files, "test".to_string());
        assert_eq!(repo.get("providers.yaml"), Some(b"p".as_slice()));
        assert!(repo.contains("routes/00-a.route.yaml"));
        // entries_under enumerates one level under a dir.
        let routes = repo.entries_under("routes");
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].0, "00-a.route.yaml", "sorted");
        let projects = repo.entries_under("projects");
        assert_eq!(projects[0].0, "x/base.chain.yaml");
    }

    #[test]
    fn content_id_is_stable_and_order_independent() {
        let a = vec![
            ("a".to_string(), b"1".to_vec()),
            ("b".to_string(), b"2".to_vec()),
        ];
        let b = vec![
            ("b".to_string(), b"2".to_vec()),
            ("a".to_string(), b"1".to_vec()),
        ];
        assert_eq!(ResolvedRepo::content_id(&a), ResolvedRepo::content_id(&b));
        let c = vec![
            ("a".to_string(), b"1".to_vec()),
            ("b".to_string(), b"changed".to_vec()),
        ];
        assert_ne!(ResolvedRepo::content_id(&a), ResolvedRepo::content_id(&c));
    }
}

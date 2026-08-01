//! Integration tests for the Git-backed config source (Phase 2, milestone 2).
//!
//! These drive the REAL `gix`-backed [`GitSource`] against a real Git
//! repository initialized with the system `git` CLI, proving the properties
//! docs/07 makes binding for "Truth in Git":
//!
//! - **The snapshot carries the source commit SHA.** A render sourced from a
//!   commit records that commit's 40-hex SHA in `RenderedSnapshot.source_commit`.
//! - **Commit-hash render determinism.** Rendering the same commit twice yields
//!   the identical `render_hash` and bytes — a pure function of `(commit,
//!   labels)` (docs/07, "Reproducible from a commit hash").
//! - **Restart reproducibility.** A control-plane "restart" (a fresh
//!   `GitSource` over the same repo, re-resolving the same ref) re-derives the
//!   exact same snapshot — the six-month rule made mechanical.
//! - **A historical commit re-renders its own bytes.** Rendering an EARLIER
//!   commit reproduces that commit's config, not HEAD's — the audit path.
//! - **Directory and Git sources agree.** The same content rendered from a
//!   loose directory and from a Git commit produce the identical `render_hash`,
//!   because both feed a byte-identical resolved file-set into byte-identical
//!   assembly.

use std::path::{Path, PathBuf};
use std::process::Command;

use gatewayctl::render::{render_repo, render_source, testrepo};
use gatewayctl::source::{ConfigSource, GitSource};

/// A throwaway Git repo seeded with the minimal-valid config repo.
struct GitRepo {
    dir: PathBuf,
}

impl GitRepo {
    /// Init a repo and commit the minimal-valid config with fleet env=`env`.
    fn init(env: &str) -> GitRepo {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gatewayctl-git-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = GitRepo { dir };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.email", "t@t"]);
        repo.git(&["config", "user.name", "t"]);
        testrepo::write_files(&repo.dir, env);
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-q", "-m", &format!("config env={env}")]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    /// Edit the fleet env pin and commit it — a "config PR merged".
    fn commit_env_change(&self, new_env: &str) -> String {
        testrepo::write_files(&self.dir, new_env);
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", &format!("flip env -> {new_env}")]);
        self.head()
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for GitRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Is the system `git` available? The tests need it to build a real repo; if
/// it's absent we skip rather than fail (the gix READ path is what's under
/// test, git is only the fixture builder).
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn the_snapshot_records_the_real_source_commit_sha() {
    if !git_available() {
        eprintln!("skipping: system git not available");
        return;
    }
    let repo = GitRepo::init("prod");
    let head = repo.head();
    assert_eq!(head.len(), 40, "a real 40-hex commit sha");

    let src = GitSource::new(repo.path(), "HEAD");
    let rendered = render_source(&src).unwrap();
    assert_eq!(
        rendered.source_commit, head,
        "the render records the exact commit it came from (docs/07 six-month rule)"
    );
    assert_eq!(rendered.render_hash.len(), 64);
}

#[test]
fn rendering_the_same_commit_twice_yields_the_identical_render_hash() {
    if !git_available() {
        return;
    }
    let repo = GitRepo::init("prod");
    let head = repo.head();

    // Render the SAME commit twice, addressing it by ref AND by full SHA.
    let by_ref = render_source(&GitSource::new(repo.path(), "HEAD")).unwrap();
    let by_sha = render_source(&GitSource::new(repo.path(), &head)).unwrap();

    assert_eq!(by_ref.source_commit, by_sha.source_commit);
    assert_eq!(
        by_ref.render_hash, by_sha.render_hash,
        "render is a pure function of (commit, labels)"
    );
    assert_eq!(by_ref.config_bytes, by_sha.config_bytes);
}

#[test]
fn a_control_plane_restart_re_derives_the_exact_same_snapshot() {
    if !git_available() {
        return;
    }
    let repo = GitRepo::init("prod");

    // "Before the restart": one GitSource resolves + renders HEAD.
    let before = render_source(&GitSource::new(repo.path(), "HEAD")).unwrap();

    // "After the restart": a brand-new GitSource over the same repo (no shared
    // state) re-resolves the same ref and re-renders. Bytes and hash identical.
    let after = render_source(&GitSource::new(repo.path(), "HEAD")).unwrap();

    assert_eq!(before.source_commit, after.source_commit);
    assert_eq!(
        before.render_hash, after.render_hash,
        "a restart re-derives the exact same snapshot from the same commit"
    );
    assert_eq!(before.config_bytes, after.config_bytes);
}

#[test]
fn a_historical_commit_re_renders_its_own_bytes_not_head() {
    if !git_available() {
        return;
    }
    let repo = GitRepo::init("prod");
    let old_commit = repo.head();
    let old_render = render_source(&GitSource::new(repo.path(), &old_commit)).unwrap();

    // Merge a config change: env prod -> canary. HEAD moves.
    let new_commit = repo.commit_env_change("canary");
    assert_ne!(old_commit, new_commit);

    // HEAD now renders the NEW config.
    let head_render = render_source(&GitSource::new(repo.path(), "HEAD")).unwrap();
    assert_eq!(head_render.source_commit, new_commit);
    assert_ne!(
        head_render.render_hash, old_render.render_hash,
        "HEAD changed, so the render hash changed"
    );

    // Re-rendering the OLD commit still reproduces the OLD bytes — the audit
    // path: "what was this node running at commit X" is a pure re-render of X.
    let replay = render_source(&GitSource::new(repo.path(), &old_commit)).unwrap();
    assert_eq!(replay.source_commit, old_commit);
    assert_eq!(
        replay.render_hash, old_render.render_hash,
        "a recorded commit re-renders its own identical bytes, forever"
    );
    assert_eq!(replay.config_bytes, old_render.config_bytes);
}

#[test]
fn the_directory_source_and_the_git_source_render_identical_bytes() {
    if !git_available() {
        return;
    }
    // Same content, two sources: a loose directory and a Git commit of that
    // same directory. The render_hash must match — the sources feed identical
    // resolved bytes into identical assembly.
    let repo = GitRepo::init("prod");
    let git_render = render_source(&GitSource::new(repo.path(), "HEAD")).unwrap();

    // The directory source over the SAME working tree (which git committed).
    let dir_render = render_repo(repo.path()).unwrap();

    assert_eq!(
        git_render.render_hash, dir_render.render_hash,
        "directory and Git sources agree on the render for identical content"
    );
    // Only the source_commit id differs in provenance (real sha vs dir-<hash>).
    assert!(dir_render.source_commit.starts_with("dir-"));
    assert_eq!(git_render.source_commit.len(), 40);
}

#[test]
fn a_bad_ref_is_a_precise_error_not_a_panic() {
    if !git_available() {
        return;
    }
    let repo = GitRepo::init("prod");
    let err = render_source(&GitSource::new(repo.path(), "no-such-ref")).unwrap_err();
    // A missing ref surfaces as an IO/render error carrying the ref name.
    assert!(
        format!("{err}").contains("no-such-ref"),
        "the error names the unresolvable ref: {err}"
    );
}

#[test]
fn describe_reports_the_source_kind_and_target() {
    let dir = GitSource::new("/tmp/repo", "main");
    assert_eq!(dir.describe(), "git:/tmp/repo@main");
    assert_eq!(dir.reference(), "main");
}

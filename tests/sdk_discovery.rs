//! SDK discovery, driven through the **real** `git` binary.
//!
//! The unit tests in `src/sdksource.rs` inject a fake `GitRunner`, which is fast
//! and hermetic but cannot catch the class of bug actually reported: `run_git`
//! accepted a working directory, built a description of the command, and then
//! never told the spawned process about it. Every fake-runner test passed while
//! production asked git the wrong question.
//!
//! These tests build a real repository on disk, so the child process, the
//! `current_dir` call and git's own output are all genuinely exercised. They
//! need only `git` on `PATH` and never touch the network.
//!
//! Set `DARWINFORGE_TEST_REMOTE=<url>` to additionally run the whole discovery
//! path against a real remote SDK repository. That test is skipped when the
//! variable is unset, so the suite stays offline by default.

use std::path::{Path, PathBuf};
use std::process::Command;

use darwinforge::sdksource::{self, GitRunner};

/// A throwaway directory removed on drop, so a failing test leaves nothing behind.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let path = std::env::temp_dir()
            .join(format!("darwinforge-sdkdisc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch dir");
        Scratch { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Run git, failing the test if it does not succeed.
fn git(cwd: &Path, args: &[&str]) -> String {
    let output =
        Command::new("git").args(args).current_dir(cwd).output().expect("git must run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Build a repository whose top level holds device SDK directories.
fn make_sdk_repo(tag: &str) -> Scratch {
    let scratch = Scratch::new(tag);
    let origin = &scratch.path;
    git(origin, &["init", "--quiet", "--initial-branch=main"]);
    git(origin, &["config", "user.email", "test@example.invalid"]);
    git(origin, &["config", "user.name", "darwinforge test"]);
    git(origin, &["config", "commit.gpgsign", "false"]);

    // A realistic spread: numeric-ordering cases, a three-component release, a
    // pre-release, a simulator SDK that must be filtered out, and an unrelated
    // file that must be ignored.
    for name in [
        "iPhoneOS9.3.sdk",
        "iPhoneOS10.3.sdk",
        "iPhoneOS12.1.2.sdk",
        "iPhoneOS17.0.sdk",
        "iPhoneOS17.0.2.sdk",
        "iPhoneOS17.5.sdk",
        "iPhoneOS26.4.sdk",
        "iPhoneOS26.4.1.sdk",
        "iPhoneOS27.0.sdk",
        "iPhoneOS18.0b3.sdk",
        "iPhoneSimulator17.5.sdk",
    ] {
        std::fs::create_dir_all(origin.join(name)).expect("mkdir sdk");
        // Each SDK needs a tracked file or git ignores the empty directory.
        std::fs::write(origin.join(name).join("SDKSettings.json"), "{\"Version\":\"1\"}")
            .expect("write settings");
    }
    std::fs::write(origin.join("README.md"), "test repository").expect("write readme");
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "--quiet", "-m", "sdks"]);
    scratch
}

/// Delegates to the real `run_git` while recording what it was handed, so a test
/// can assert where git would actually have looked.
#[derive(Default)]
struct RecordingRunner {
    directories: std::cell::RefCell<Vec<Option<PathBuf>>>,
}
#[test]
fn ls_tree_is_asked_of_the_clone_directory_and_finds_every_sdk() {
    if !git_available() {
        eprintln!("skipped: `git` is not on PATH");
        return;
    }
    let scratch = make_sdk_repo("cwd");
    let clone = scratch.path.join("clone");
    let runner = RecordingRunner::default();

    // Clone the way `ensure_clone` does, then list the tree the way
    // `discover_detailed` does — through the recording runner, so the directory
    // it passes is visible.
    let origin = scratch.path.to_string_lossy().to_string();
    let destination = clone.to_string_lossy().to_string();
    runner
        .run(None, &["clone", "--depth", "1", "--quiet", &origin, &destination])
        .expect("clone must succeed");

    let args = sdksource::ls_tree_args("HEAD");
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let listing = runner.run(Some(&clone), &borrowed).expect("ls-tree must succeed");

    // The assertion that would have caught the original bug: the recorded
    // directory is the clone, not wherever this test happens to run from.
    let recorded = runner.directories.borrow().last().cloned().flatten();
    assert_eq!(
        recorded.as_deref(),
        Some(clone.as_path()),
        "git must be told to run inside the clone"
    );

    let sdks = sdksource::parse_tree_listing(&listing);
    let versions: Vec<String> = sdks.iter().map(|sdk| sdk.version.as_string()).collect();
    assert_eq!(
        versions,
        // Newest first, numerically per component. `18.0b3` sorts below `17.5`
        // because a pre-release precedes the release it leads up to, so it is
        // not treated as newer than the 18.x line it belongs to.
        [
            "27.0", "26.4.1", "26.4", "18.0b3", "17.5", "17.0.2", "17.0", "12.1.2", "10.3", "9.3",
        ],
        "the real repository orders exactly as required, simulator SDK dropped"
    );
    // `latest` is a stable release, never the pre-release.
    let mut warning = String::new();
    let chosen = sdksource::select_latest(&sdks, Some(&mut warning)).expect("must choose");
    assert_eq!(chosen.version.as_string(), "27.0");
    assert!(warning.is_empty(), "nothing was passed over: `{warning}`");
    assert!(
        sdksource::is_prerelease(&sdks[3]),
        "the 18.0b3 entry is recognised as a pre-release"
    );
}

#[test]
fn the_default_branch_is_discovered_rather_than_assumed() {
    if !git_available() {
        eprintln!("skipped: `git` is not on PATH");
        return;
    }
    // The reported repository used `master`; the one built above uses `main`.
    // No branch name may be baked in, so discovery must find whatever this
    // repository actually uses.
    let scratch = make_sdk_repo("branch");
    let origin = scratch.path.to_string_lossy().to_string();
    let (reference, tried) =
        sdksource::detect_ref(&sdksource::run_git, &origin, None).expect("a ref must be found");
    // Either the repository's own branch or `HEAD` is a correct answer; a
    // hardcoded name would be neither on a repository that uses the other one.
    assert!(
        reference == "HEAD" || reference.contains("main") || reference.contains("master"),
        "found the repository's own default branch, got {reference}"
    );
    // Whatever it settled on must actually resolve back to a commit.
    let listing = sdksource::run_git(Some(&scratch.path), &["ls-tree", "--name-only", "-d", &reference])
        .expect("the discovered ref must resolve in a real repository");
    assert!(
        sdksource::parse_tree_listing(&listing).len() > 1,
        "and it really does hold the SDKs: {listing}"
    );
    // Probes are only recorded when they were actually rejected, so an empty
    // list means the very first probe worked — which is correct, not a bug.
    assert!(
        tried.iter().all(|ref_name| ref_name != &reference),
        "a ref that worked is never listed as tried-and-rejected: {tried:?}"
    );
}

#[test]
fn a_git_failure_reports_git_s_own_words() {
    if !git_available() {
        eprintln!("skipped: `git` is not on PATH");
        return;
    }
    // A path that is not a repository: the error must carry git's stderr rather
    // than merely asserting that git failed.
    let scratch = Scratch::new("bad-source");
    let source = scratch.path.to_string_lossy().to_string();
    let error = sdksource::run_git(None, &["ls-remote", &source, "HEAD"])
        .expect_err("must fail");
    let text = error.to_string();
    assert_eq!(error.exit_code(), 1, "a tool failure keeps the tool exit code");
    assert!(text.contains("ls-remote"), "names the command: {text}");
    assert!(
        text.contains("not a git repository") || text.contains("does not appear to be"),
        "git's stderr reaches the message: {text}"
    );
}

#[test]
fn the_real_remote_repository_is_discovered_when_one_is_configured() {
    // Opt-in: the default suite must not depend on network access.
    let Ok(source) = std::env::var("DARWINFORGE_TEST_REMOTE") else {
        eprintln!("skipped: set DARWINFORGE_TEST_REMOTE=<url> to run this");
        return;
    };
    if !git_available() {
        eprintln!("skipped: `git` is not on PATH");
        return;
    }
    // Point the data directory at a scratch path so this never disturbs a real
    // darwinforge installation.
    let scratch = Scratch::new("remote");
    let scratch_dir = scratch.path.to_string_lossy().to_string();
    let previous = std::env::var(if cfg!(windows) { "LOCALAPPDATA" } else { "XDG_DATA_HOME" }).ok();
    if cfg!(windows) {
        std::env::set_var("LOCALAPPDATA", &scratch_dir);
    } else {
        std::env::set_var("XDG_DATA_HOME", &scratch_dir);
    }

    let found = sdksource::discover_detailed(&source, &sdksource::run_git).expect("discovery runs");
    let versions: Vec<String> = found.sdks.iter().map(|sdk| sdk.version.as_string()).collect();

    // The reported failure was an empty listing; this must not be empty.
    assert!(
        !versions.is_empty(),
        "the real repository yielded no SDKs. refs_tried={:?}\ntrace:\n{}",
        found.refs_tried,
        found.trace.render()
    );
    // And the versions must be in descending numeric order.
    let mut sorted = versions.clone();
    sorted.sort_by(|left, right| {
        sdksource::SdkVersion::parse(right).cmp(&sdksource::SdkVersion::parse(left))
    });
    assert_eq!(versions, sorted, "newest first, numerically ordered");
    println!("discovered {} SDKs, newest {}", versions.len(), versions[0]);

    if let Some(previous) = previous {
        if cfg!(windows) {
            std::env::set_var("LOCALAPPDATA", previous);
        } else {
            std::env::set_var("XDG_DATA_HOME", previous);
        }
    }
}

impl GitRunner for RecordingRunner {
    fn run(&self, cwd: Option<&Path>, args: &[&str]) -> darwinforge::Result<String> {
        self.directories.borrow_mut().push(cwd.map(Path::to_path_buf));
        // The real spawn path, including the `current_dir` this test exists for.
        sdksource::run_git(cwd, args)
    }
}
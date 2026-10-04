//! Discovering and installing iPhoneOS SDKs from a git repository.
//!
//! The repository is a **configuration value** ([`crate::global_config`]'s
//! `sdk.source`, overridable per run), never a constant in the logic: a user
//! may point darwinforge at their own mirror, an internal host, or a fork that
//! carries newer SDKs. What this module deliberately does *not* contain is a
//! list of versions. The available SDKs are listed from the remote at run time,
//! so an SDK published tomorrow works without a new release of this tool.
//!
//! Everything that decides anything is a **pure function over text or over a
//! directory**, which is why the interesting tests here are tests of parsing:
//!
//! * [`parse_tree_listing`] turns `git ls-tree --name-only -d` output into
//!   [`RemoteSdk`]s, dropping simulator SDKs and unrelated directories;
//! * [`SdkVersion`] orders versions **numerically**, which is the whole point:
//!   `"17.10"` is *newer* than `"17.5"`, and string comparison gets that
//!   backwards;
//! * [`detect_symlinks`] reports whether a checkout materialised symlinks or
//!   wrote text stubs, which is the difference between a working SDK and a
//!   broken one on Windows;
//! * [`license_reminder`] and [`require_license_acknowledged`] make the
//!   licensing position explicit instead of implicit.
//!
//! The impure half — cloning, listing, checking out — goes through an injected
//! [`GitRunner`], so the unit tests drive the whole state machine with canned
//! `git` output and never touch the network.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};
use crate::paths;
use crate::sdk::Sdk;

/// The repository used when the configuration names none.
///
/// This is a *default*, and it is overridable everywhere: by `sdk.source` in
/// the global config, and by `--source` on the command line. See
/// [`resolve_source`].
pub const DEFAULT_SOURCE: &str = "https://github.com/xybp888/iOS-SDKs";

/// The directory prefix of a device SDK. Simulator, watchOS, tvOS and macOS
/// SDKs are all excluded by this prefix alone.
pub const DEVICE_PREFIX: &str = "iPhoneOS";

/// The suffix every SDK directory carries.
pub const SDK_SUFFIX: &str = ".sdk";

/// Refs tried, in order, when the remote does not say what its default branch
/// is.
///
/// This is a probe list, not a hardcoded branch: a repository is free to call it
/// anything, and [`detect_ref`] asks the remote first. `HEAD` and `origin/HEAD`
/// cover the normal cases; `main` and `master` cover servers that refuse to
/// advertise a symbolic ref.
pub const REF_PROBES: &[&str] = &["HEAD", "origin/HEAD", "main", "master"];

/// The source repository to use.
///
/// Precedence: an explicit per-run override, then the configured `sdk.source`,
/// then [`DEFAULT_SOURCE`]. An override that is present but empty falls through,
/// because an empty string is not a repository.
pub fn resolve_source(configured: Option<&str>, override_flag: Option<&str>) -> String {
    [override_flag, configured, Some(DEFAULT_SOURCE)]
        .iter()
        .filter_map(|value| value.map(str::trim))
        .find(|value| !value.is_empty())
        .unwrap_or(DEFAULT_SOURCE)
        .to_string()
}

/// An SDK version, ordered **numerically** component by component.
///
/// `17.5 < 17.10 < 18.0 < 18.1`, which string comparison gets wrong:
/// `"17.10" < "17.5"` lexicographically, so a user asking for the newest SDK
/// would be handed a five-year-old one. Deriving [`Ord`] on the component
/// vector makes that class of bug impossible rather than merely avoided.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SdkVersion {
    /// The numeric components, most significant first: `17.10` is `[17, 10]`.
    pub parts: Vec<u32>,
}

impl SdkVersion {
    /// Parse `17.5`, `17.10` or `17.5.1`.
    ///
    /// Every component must be a non-empty run of digits. Anything else —
    /// `17.x`, `latest`, an empty string — is rejected, because a version this
    /// module cannot order must never be presented as one it can.
    pub fn parse(text: &str) -> Option<SdkVersion> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        let parts: Option<Vec<u32>> = trimmed.split('.').map(parse_component).collect();
        parts.map(|parts| SdkVersion { parts })
    }

    /// The version as it appears in a directory name, e.g. `17.10`.
    pub fn as_string(&self) -> String {
        self.parts.iter().map(u32::to_string).collect::<Vec<_>>().join(".")
    }

    /// The major component, e.g. `17` for `17.10`.
    pub fn major(&self) -> u32 {
        self.parts.first().copied().unwrap_or(0)
    }
}

fn parse_component(text: &str) -> Option<u32> {
    if text.is_empty() || !text.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl std::fmt::Display for SdkVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_string())
    }
}

/// A device SDK offered by the remote repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSdk {
    /// The directory name, e.g. `iPhoneOS17.5.sdk`.
    pub name: String,
    /// Its parsed version.
    pub version: SdkVersion,
}

impl RemoteSdk {
    /// Build one from a directory name, rejecting anything that is not an
    /// iPhoneOS device SDK.
    pub fn from_name(name: &str) -> Option<RemoteSdk> {
        let stem = name.strip_prefix(DEVICE_PREFIX)?.strip_suffix(SDK_SUFFIX)?;
        let version = SdkVersion::parse(stem)?;
        Some(RemoteSdk { name: name.to_string(), version })
    }
}

/// Ordered by version, so the head of a sorted list is the newest SDK.
impl Ord for RemoteSdk {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.version.cmp(&other.version)
    }
}

impl PartialOrd for RemoteSdk {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for RemoteSdk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.version)
    }
}

/// Parse the output of `git ls-tree --name-only -d <ref>` into the device SDKs
/// it lists.
///
/// Pure: the caller supplies the text, so the whole filter — device SDKs only,
/// simulator/watchOS/tvOS/macOS dropped, unrelated directories dropped, blank
/// and whitespace-padded lines tolerated — is unit-tested against canned
/// output. The result is sorted **newest first**, because "give me the newest
/// SDK" is the overwhelmingly common request and the sort must be numeric (see
/// [`SdkVersion`]).
pub fn parse_tree_listing(stdout: &str) -> Vec<RemoteSdk> {
    let mut sdks: Vec<RemoteSdk> =
        stdout.lines().filter_map(|line| RemoteSdk::from_name(line.trim())).collect();
    // Newest first, by an explicit descending comparison rather than by
    // reversing an ascending sort: the ordering rule is the interesting part of
    // this function and is spelled out where it is applied.
    sdks.sort_by(|left, right| right.cmp(left));
    sdks
}

/// The newest SDK in a listing, or `None` when the remote offers none.
pub fn newest(sdks: &[RemoteSdk]) -> Option<&RemoteSdk> {
    sdks.iter().max_by(|left, right| left.cmp(right))
}

/// Choose an SDK from a listing: the requested version if it is present, else
/// the newest, else an error that lists what is actually on offer.
pub fn select(sdks: &[RemoteSdk], requested: Option<&str>) -> Result<RemoteSdk> {
    if let Some(wanted) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        return sdks
            .iter()
            .find(|sdk| sdk.version.as_string() == wanted || sdk.name == wanted)
            .cloned()
            .ok_or_else(|| unknown_version_error(sdks, wanted));
    }
    newest(sdks).cloned().ok_or_else(|| no_sdks_error(&[]))
}

/// The error for a request that names a version the remote does not have.
pub fn unknown_version_error(sdks: &[RemoteSdk], wanted: &str) -> Error {
    Error::Prereq {
        what: format!("the configured SDK source offers no version {wanted}"),
        fix: format!(
            "available device SDKs: {}. Change `sdk.source` in the global config, or \
             pass --source <url>, to use a repository that carries it",
            summarise(sdks)
        ),
    }
}

/// The error for a source that carries no device SDK at all.
pub fn no_sdks_error(tried: &[&str]) -> Error {
    Error::Prereq {
        what: "the configured SDK source contains no iPhoneOS device SDK".to_string(),
        fix: format!(
            "check `sdk.source` (or --source <url>): a usable repository has top-level \
             directories named like iPhoneOS17.5.sdk. Refs tried: {}",
            if tried.is_empty() { "none".to_string() } else { tried.join(", ") }
        ),
    }
}

/// A short, readable summary of a listing, for an error message.
fn summarise(sdks: &[RemoteSdk]) -> String {
    if sdks.is_empty() {
        return "none".to_string();
    }
    let names: Vec<String> = sdks.iter().take(12).map(|sdk| sdk.version.to_string()).collect();
    if sdks.len() > names.len() {
        format!("{} (and {} more)", names.join(", "), sdks.len() - names.len())
    } else {
        names.join(", ")
    }
}
/// Parse `git ls-remote --symref <url> HEAD` output into the ref it advertises.
///
/// The first line of that output is `ref: refs/heads/main<TAB>HEAD`, which is
/// how a remote names its default branch. Returns `None` when the server does
/// not advertise one — normal, not an error: [`detect_ref`] then probes
/// [`REF_PROBES`] instead.
pub fn parse_symref(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("ref:") else { continue };
        let mut fields = rest.split_whitespace();
        let reference = fields.next()?;
        if reference.is_empty() {
            continue;
        }
        return Some(reference.to_string());
    }
    None
}

/// Every ref named in `git ls-remote` output.
///
/// `git` prints `<object><TAB><ref>` per line. The first field is required to
/// look like an object name — hex, at least seven characters — so a stray
/// warning line in the output cannot be mistaken for a ref and sent to
/// `ls-remote` as if it were a branch name.
pub fn parse_remote_refs(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let object = fields.next()?;
            if object.len() < 7 || !object.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            fields.next().map(str::to_string)
        })
        .collect()
}

/// One `git` invocation: the arguments, and where to run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommand {
    /// Where the command runs. `None` means "anywhere".
    pub cwd: Option<PathBuf>,
    /// The arguments, already in `git`'s own spelling.
    pub args: Vec<String>,
}

impl GitCommand {
    /// Render for an error message: `git <args>`.
    pub fn display(&self) -> String {
        format!("git {}", self.args.join(" "))
    }
}

/// Runs `git` and returns its stdout.
///
/// Injected so the discovery and install flows are testable without a network,
/// a repository or a `git` binary. Production passes [`run_git`]; every test
/// passes a closure that answers from canned output.
pub trait GitRunner {
    /// Run one command in `cwd`, returning its stdout.
    fn run(&self, cwd: Option<&Path>, args: &[&str]) -> Result<String>;
}

impl<F> GitRunner for F
where
    F: Fn(Option<&Path>, &[&str]) -> Result<String>,
{
    fn run(&self, cwd: Option<&Path>, args: &[&str]) -> Result<String> {
        self(cwd, args)
    }
}

/// The production runner: the system `git`, resolved through
/// [`crate::toolfind`] so an explicit `git` override is honoured, with stdout
/// captured and stderr inherited so git's progress and prompts reach the user.
pub fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let command = GitCommand {
        cwd: cwd.map(Path::to_path_buf),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
    };
    let program = crate::toolfind::find(&crate::toolfind::GIT, None)
        .map(|found| found.path)
        .unwrap_or_else(|| PathBuf::from("git"));
    let mut child = Command::new(&program)
        .args(&command.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                Error::Prereq {
                    what: "`git` was not found on PATH".to_string(),
                    fix: crate::toolfind::GIT.fix_hint.to_string(),
                }
            } else {
                Error::io(format!("cannot run {}", command.display()), source)
            }
        })?;
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let status = child
        .wait()
        .map_err(|source| Error::io(format!("cannot run {}", command.display()), source))?;
    if !status.success() {
        return Err(Error::ToolFailed {
            program: "git".to_string(),
            args: command.args,
            code: status.code(),
        });
    }
    Ok(stdout)
}
/// The ref to list the tree of, discovered rather than assumed.
///
/// Probed in order: whatever a previous run recorded, `HEAD`, `origin/HEAD`,
/// the ref the remote advertises through `--symref`, then [`REF_PROBES`], then
/// every ref `ls-remote` reports. The first ref that resolves wins, so no single
/// branch name is baked in and an unusual layout still works. The refs that
/// were tried and rejected come back with the answer, for the error message.
pub fn detect_ref(
    runner: &dyn GitRunner,
    source: &str,
    preferred: Option<&str>,
) -> Result<(String, Vec<String>)> {
    let mut probes: Vec<String> = Vec::new();
    let mut advertised: Option<String> = None;
    let consider = |probes: &mut Vec<String>, candidate: Option<&str>| {
        if let Some(candidate) = candidate.map(str::trim).filter(|value| !value.is_empty()) {
            if !probes.iter().any(|existing| existing == candidate) {
                probes.push(candidate.to_string());
            }
        }
    };
    consider(&mut probes, preferred);
    consider(&mut probes, Some("HEAD"));
    consider(&mut probes, Some("origin/HEAD"));

    if let Ok(listing) = runner.run(None, &["ls-remote", "--symref", source, "HEAD"]) {
        advertised = parse_symref(&listing);
        consider(&mut probes, advertised.as_deref());
    }
    for candidate in REF_PROBES {
        consider(&mut probes, Some(candidate));
    }
    if let Ok(all) = runner.run(None, &["ls-remote", source]) {
        for reference in parse_remote_refs(&all) {
            consider(&mut probes, Some(&reference));
        }
    }

    let mut tried: Vec<String> = Vec::new();
    for probe in &probes {
        match runner.run(None, &["ls-remote", source, probe]) {
            Ok(output) if !output.trim().is_empty() => return Ok((probe.clone(), tried)),
            _ => tried.push(probe.clone()),
        }
    }
    Err(Error::Prereq {
        what: format!("{source} does not look like a readable git repository"),
        fix: format!(
            "check `sdk.source` (or --source <url>): `git ls-remote` answered for none \
             of {}. Check the URL, your network, and any credentials it needs",
            if tried.is_empty() {
                advertised.as_deref().unwrap_or("any ref").to_string()
            } else {
                tried.join(", ")
            }
        ),
    })
}

/// The `git` arguments that produce a shallow, blobless, sparse checkout.
///
/// `--depth 1` keeps the history out of it, `--filter=blob:none` keeps the file
/// contents of every *other* SDK out of it, and `--sparse` leaves the working
/// tree empty until [`sparse_checkout_args`] names the one directory wanted.
/// Built here rather than inline so a test can assert the flags are still the
/// ones that keep the download small.
pub fn clone_args(source: &str, destination: &Path) -> Vec<String> {
    vec![
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--filter=blob:none".to_string(),
        "--sparse".to_string(),
        source.to_string(),
        paths::display_path(destination),
    ]
}

/// The `git` arguments that materialise exactly one SDK inside a sparse clone.
pub fn sparse_checkout_args(sdk_name: &str) -> Vec<String> {
    vec!["sparse-checkout".to_string(), "set".to_string(), sdk_name.to_string()]
}

/// The `git` arguments that list the top-level directories of a ref.
pub fn ls_tree_args(reference: &str) -> Vec<String> {
    vec![
        "ls-tree".to_string(),
        "--name-only".to_string(),
        "-d".to_string(),
        reference.to_string(),
    ]
}
/// Where the shallow clone of the source lives.
///
/// Under [`paths::cache_dir`] rather than beside the installed SDKs, because
/// the clone is a cache: it is disposable, it holds git objects rather than
/// SDK files, and losing it costs one `git clone` next time.
pub fn clone_dir() -> Result<PathBuf> {
    let dir = paths::cache_dir().map_err(|message| Error::Prereq {
        what: message,
        fix: "set HOME (Linux/macOS) or USERPROFILE (Windows) and try again".to_string(),
    })?;
    Ok(dir.join("sdk-source"))
}

/// Ensure the shallow clone exists, cloning it if it does not.
fn ensure_clone(source: &str, runner: &dyn GitRunner) -> Result<PathBuf> {
    let clone = clone_dir()?;
    if clone.join(".git").is_dir() {
        return Ok(clone);
    }
    // A partial or foreign cache must not be reused: `git clone` refuses a
    // non-empty directory anyway, and an incremental fetch would defeat the
    // point of a fresh, versionless listing.
    if clone.exists() {
        remove_dir_if_managed(&clone)?;
    }
    if let Some(parent) = clone.parent() {
        paths::ensure_dir(parent)?;
    }
    let args = clone_args(source, &clone);
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    runner.run(None, &borrowed)?;
    Ok(clone)
}

/// Remove a cache directory, refusing anything we do not own.
fn remove_dir_if_managed(dir: &Path) -> Result<()> {
    if !paths::is_managed_dir(dir) {
        return Err(Error::Prereq {
            what: format!("refusing to replace the SDK cache at {}", dir.display()),
            fix: "move or delete that directory yourself, then run the command again"
                .to_string(),
        });
    }
    std::fs::remove_dir_all(dir)
        .map_err(|source| Error::io(format!("cannot clear {}", dir.display()), source))
}

/// List the device SDKs the source repository offers.
///
/// The sequence is: find a ref ([`detect_ref`]), make sure the shallow clone
/// exists, then list that ref's top-level directories and parse them with
/// [`parse_tree_listing`]. Nothing here knows a version number — the listing is
/// whatever the remote has today.
pub fn discover(source: &str, runner: &dyn GitRunner) -> Result<Vec<RemoteSdk>> {
    let (reference, _tried) = detect_ref(runner, source, None)?;
    let clone = ensure_clone(source, runner)?;
    let args = ls_tree_args(&reference);
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let listing = runner.run(Some(&clone), &borrowed)?;
    Ok(parse_tree_listing(&listing))
}
/// How a checkout materialised the symlinks an SDK tree contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkState {
    /// Real symlinks. The tree is usable as-is.
    Real,
    /// Tiny regular files whose content is the link target. `git` does this when
    /// `core.symlinks` is false — the Windows default without Developer Mode —
    /// and an SDK laid out this way is **not** usable.
    TextStub,
    /// No symlinks at all were found. Nothing to warn about.
    Absent,
}

impl SymlinkState {
    /// One phrase, for a status line.
    pub fn label(self) -> &'static str {
        match self {
            SymlinkState::Real => "real symlinks",
            SymlinkState::TextStub => "text stubs, not symlinks",
            SymlinkState::Absent => "no symlinks",
        }
    }

    /// True when the tree cannot be trusted and the caller must say so.
    pub fn is_broken(self) -> bool {
        matches!(self, SymlinkState::TextStub)
    }
}

/// What a directory scan found, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkReport {
    /// The overall state.
    pub state: SymlinkState,
    /// How many real symlinks were seen.
    pub real: usize,
    /// How many text stubs were seen.
    pub stubs: usize,
    /// An example path, so the message can point at something concrete.
    pub example: Option<PathBuf>,
}

impl SymlinkReport {
    /// True when nothing needs to be reported.
    pub fn ok(&self) -> bool {
        !self.state.is_broken()
    }
}

/// How many filesystem entries [`detect_symlinks`] looks at before it stops.
/// Bounded so a pathological tree cannot stall an install.
pub const MAX_ENTRIES: usize = 20_000;

/// The longest a link stub may be before it is treated as ordinary content.
const MAX_STUB_LEN: u64 = 512;

/// Scan `root` (breadth-first, bounded) for how symlinks were materialised.
///
/// Detection is by content, not by name: a stub is a short regular file whose
/// whole content is a relative path. A real symlink is seen through
/// `symlink_metadata`, so it is reported even on Windows, where creating one
/// needs a privilege.
pub fn detect_symlinks(root: &Path) -> SymlinkReport {
    let mut report = SymlinkReport {
        state: SymlinkState::Absent,
        real: 0,
        stubs: 0,
        example: None,
    };
    if !root.is_dir() {
        return report;
    }
    let mut queue: std::collections::VecDeque<PathBuf> = std::collections::VecDeque::new();
    queue.push_back(root.to_path_buf());
    let mut seen = 0usize;
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                // A bounded scan that has already seen a real symlink is still
                // a true answer; one that has not is only "none found so far".
                if report.real > 0 {
                    report.state = SymlinkState::Real;
                }
                return report;
            }
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else { continue };
            if metadata.file_type().is_symlink() {
                report.real += 1;
                if report.example.is_none() {
                    report.example = Some(path);
                }
            } else if metadata.is_dir() {
                queue.push_back(path);
            } else if looks_like_link_stub(&path) {
                report.stubs += 1;
                if report.example.is_none() {
                    report.example = Some(path);
                }
            }
        }
    }
    report.state = match (report.stubs > 0, report.real > 0) {
        (true, _) => SymlinkState::TextStub,
        (false, true) => SymlinkState::Real,
        (false, false) => SymlinkState::Absent,
    };
    report
}
/// True when `path` is a small regular file whose entire content is a relative
/// path — git's substitute for a symlink.
///
/// The test is deliberately strict: one line, no NUL, starts with `./` or
/// `../`, and names something that actually exists next to it. Anything looser
/// would flag ordinary SDK text files, and a false positive here produces a
/// frightening warning about a perfectly good SDK.
fn looks_like_link_stub(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else { return false };
    if !metadata.is_file() || metadata.len() > MAX_STUB_LEN {
        return false;
    }
    let Ok(content) = std::fs::read_to_string(path) else { return false };
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\0') {
        return false;
    }
    if !(trimmed.starts_with("./") || trimmed.starts_with("../")) {
        return false;
    }
    // The target must exist, or this is just a text file beginning with "./".
    path.parent().map(|parent| parent.join(trimmed).exists()).unwrap_or(false)
}

/// The warning to show for a checkout whose symlinks are stubs.
///
/// This never fails a build by itself — it reports the truth about what was
/// checked out, because an SDK of stub files produces baffling errors much
/// later, in the middle of a compile.
pub fn symlink_warning(report: &SymlinkReport) -> Option<String> {
    if report.ok() {
        return None;
    }
    let example = report
        .example
        .as_ref()
        .map(|path| paths::display_path(path))
        .unwrap_or_else(|| "a link inside the SDK".to_string());
    Some(format!(
        "the SDK checkout contains {stubs} text stub(s) where symlinks belong (for \
         example {example}). git wrote them as plain files because core.symlinks is \
         false, which is its default on Windows without Developer Mode, so the SDK \
         will not link correctly. Enable Developer Mode (or run `git config --global \
         core.symlinks true` in an elevated prompt), delete the installed SDK \
         directory, and install it again",
        stubs = report.stubs
    ))
}

/// The licensing reminder shown before anything is fetched.
///
/// The SDK is Apple's, redistributed by a third-party repository; darwinforge
/// fetches it on the user's behalf and is responsible for saying so plainly. A
/// function returning the exact text, rather than a string in a `println!`, so
/// that a test can assert it still names Apple and the user's obligation.
pub fn license_reminder(source: &str) -> String {
    format!(
        "About this SDK: the iPhoneOS SDK you are about to download is Apple's software, \
         redistributed by the third-party git repository {source}. It is not provided \
         by, endorsed by or licensed to you by darwinforge. You are responsible for \
         complying with Apple's licence terms for the SDK, and for any other rights \
         you need. darwinforge only fetches and uses the files; it never modifies or \
         redistributes them itself."
    )
}

/// Require the licence reminder to have been acknowledged, once.
///
/// `accepted` is the caller's flag: the saved `sdk_license_accepted` from the
/// global config, or the answer to a prompt. `Ok(())` when it is set; an
/// actionable error carrying the reminder otherwise, so the text reaches the
/// user whatever the caller does with the error.
pub fn require_license_acknowledged(accepted: bool, source: &str) -> Result<()> {
    if accepted {
        return Ok(());
    }
    Err(Error::Prereq {
        what: "the SDK licence reminder has not been acknowledged".to_string(),
        fix: format!(
            "{}. Re-run with --yes (or set sdk_license_accepted = true in the global \
             config) once you have read it",
            license_reminder(source)
        ),
    })
}
/// Where a chosen SDK is installed: [`paths::sdks_dir`]/`<version>`.
///
/// Keyed by the version rather than by the repository's directory name, so two
/// sources offering "18.1" install to the same place and an upstream rename
/// does not orphan the installed copy.
pub fn install_dir(version: &SdkVersion) -> Result<PathBuf> {
    let dir = paths::sdks_dir().map_err(|message| Error::Prereq {
        what: message,
        fix: "set HOME (Linux/macOS) or USERPROFILE (Windows) and try again".to_string(),
    })?;
    Ok(dir.join(version.as_string()))
}

/// Install one SDK from `source` into [`paths::sdks_dir`].
///
/// Steps: require the licence acknowledgement, ensure the shallow clone exists,
/// sparse-checkout the one directory, scan it for stubbed symlinks, move it into
/// place, then **validate** it with [`validate_install`] — a checkout that
/// produced an unusable tree fails here with a message naming what is missing,
/// not three stages later in the middle of a compile.
///
/// Returns the validated SDK plus the symlink report, so the caller can warn
/// about a stub-based checkout with [`symlink_warning`].
pub fn install(
    source: &str,
    sdk: &RemoteSdk,
    runner: &dyn GitRunner,
    license_accepted: bool,
) -> Result<(Sdk, SymlinkReport)> {
    require_license_acknowledged(license_accepted, source)?;
    let clone = ensure_clone(source, runner)?;
    let args = sparse_checkout_args(&sdk.name);
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    runner.run(Some(&clone), &borrowed)?;

    let checked_out = clone.join(&sdk.name);
    // Report the state of the tree *before* moving it, while the paths in the
    // report still mean something to the user.
    let report = detect_symlinks(&checked_out);

    let target = install_dir(&sdk.version)?;
    if let Some(parent) = target.parent() {
        paths::ensure_dir(parent)?;
    }
    if target.exists() {
        // Only ever replace something we own.
        if !paths::is_managed_dir(&target) {
            return Err(Error::Prereq {
                what: format!("refusing to replace the SDK at {}", target.display()),
                fix: "move or delete that directory yourself, then install again".to_string(),
            });
        }
        std::fs::remove_dir_all(&target).map_err(|source| {
            Error::io(format!("cannot replace {}", target.display()), source)
        })?;
    }
    // A rename keeps the git objects in the cache, so installing a different
    // version next time is still cheap.
    std::fs::rename(&checked_out, &target).map_err(|source| {
        Error::io(format!("cannot move the SDK into {}", target.display()), source)
    })?;

    validate_install(&target)?;
    let validated = Sdk::open(&target)?;
    Ok((validated, report))
}

/// Validate an installed SDK, turning a layout failure into an actionable error
/// that names what was missing and offers the retry.
pub fn validate_install(root: &Path) -> Result<()> {
    Sdk::open(root).map(|_| ()).map_err(|error| match error {
        Error::Prereq { what, fix } => Error::Prereq {
            what: format!("the SDK checkout at {} is not usable: {what}", root.display()),
            fix: format!(
                "{fix}. The checkout may have been truncated, or the sparse checkout \
                 may not have matched; remove {} and install again",
                paths::display_path(root)
            ),
        },
        other => other,
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("darwinforge-sdksource-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn version(text: &str) -> SdkVersion {
        SdkVersion::parse(text).expect("must parse")
    }

    /// Realistic `git ls-tree --name-only -d HEAD` output from an SDK mirror:
    /// device SDKs, simulator SDKs and unrelated directories, in git's own
    /// alphabetical order.
    const REALISTIC_TREE: &str = "\
iPhoneOS11.0.sdk
iPhoneOS12.4.sdk
iPhoneOS14.5.sdk
iPhoneOS16.4.sdk
iPhoneOS17.0.sdk
iPhoneOS17.2.sdk
iPhoneOS17.4.sdk
iPhoneOS17.5.sdk
iPhoneOS17.10.sdk
iPhoneOS18.0.sdk
iPhoneOS18.1.sdk
iPhoneSimulator17.5.sdk
iPhoneSimulator18.1.sdk
MacOSX15.2.sdk
README.md
WatchOS10.2.sdk
";

    #[test]
    fn versions_order_numerically_not_as_strings() {
        // The whole point of the type. String comparison says "17.10" < "17.5",
        // which would hand a user a five-year-old SDK.
        assert!("17.10" < "17.5", "strings really do get this wrong");
        assert!(version("17.5") < version("17.10"), "numeric order: 17.10 is newer");
        assert!(version("17.10") > version("17.5"));
        assert!(version("17.10") < version("18.0"));
        assert!(version("18.0") < version("18.1"));
        assert!(version("17.4") < version("17.5"));

        let mut versions = [
            version("18.0"),
            version("17.10"),
            version("17.5"),
            version("18.1"),
            version("17.4"),
        ];
        versions.sort();
        let ordered: Vec<String> = versions.iter().map(SdkVersion::as_string).collect();
        assert_eq!(ordered, ["17.4", "17.5", "17.10", "18.0", "18.1"]);
    }

    #[test]
    fn versions_parse_or_are_rejected_outright() {
        assert_eq!(version("17.5").parts, [17, 5]);
        assert_eq!(version("17").parts, [17], "a bare major is still a version");
        assert_eq!(version("17.5.1").parts, [17, 5, 1], "three components survive");
        assert_eq!(version(" 17.5 ").as_string(), "17.5", "padding is trimmed");
        assert_eq!(version("17.10").major(), 17);
        assert_eq!(version("17.10").to_string(), "17.10");

        assert!(SdkVersion::parse("").is_none());
        assert!(SdkVersion::parse("   ").is_none());
        assert!(SdkVersion::parse("17.x").is_none(), "a non-numeric part is not a version");
        assert!(SdkVersion::parse("latest").is_none());
        assert!(SdkVersion::parse("17.").is_none(), "a trailing dot is not a version");
        assert!(SdkVersion::parse(".5").is_none());
        assert!(SdkVersion::parse("v17.5").is_none(), "a leading v is not a version");
    }

    #[test]
    fn only_device_sdk_directory_names_are_accepted() {
        assert_eq!(
            RemoteSdk::from_name("iPhoneOS17.5.sdk").map(|sdk| sdk.version.as_string()),
            Some("17.5".to_string())
        );
        assert_eq!(
            RemoteSdk::from_name("iPhoneOS18.0.sdk").map(|sdk| sdk.version.as_string()),
            Some("18.0".to_string())
        );
        // We need DEVICE SDKs: a simulator SDK has no arm64 device layout.
        assert!(RemoteSdk::from_name("iPhoneSimulator17.5.sdk").is_none());
        assert!(RemoteSdk::from_name("iPhoneSimulator18.1.sdk").is_none());
        assert!(RemoteSdk::from_name("MacOSX15.2.sdk").is_none());
        assert!(RemoteSdk::from_name("WatchOS10.2.sdk").is_none());
        assert!(RemoteSdk::from_name("README.md").is_none());
        assert!(RemoteSdk::from_name("iPhoneOS").is_none());
        assert!(RemoteSdk::from_name("iPhoneOS17.5").is_none(), "no .sdk suffix");
        assert!(RemoteSdk::from_name("iPhoneOSx.y.sdk").is_none());
        assert!(RemoteSdk::from_name("").is_none());
    }
    #[test]
    fn a_realistic_listing_yields_every_device_sdk_newest_first() {
        let sdks = parse_tree_listing(REALISTIC_TREE);
        let names: Vec<&str> = sdks.iter().map(|sdk| sdk.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "iPhoneOS18.1.sdk",
                "iPhoneOS18.0.sdk",
                // 17.10 sorts between 17.5 and 18.0 — the case a string sort
                // would place below 17.5.
                "iPhoneOS17.10.sdk",
                "iPhoneOS17.5.sdk",
                "iPhoneOS17.4.sdk",
                "iPhoneOS17.2.sdk",
                "iPhoneOS17.0.sdk",
                "iPhoneOS16.4.sdk",
                "iPhoneOS14.5.sdk",
                "iPhoneOS12.4.sdk",
                "iPhoneOS11.0.sdk",
            ],
            "numeric order, newest first, simulators and unrelated dirs dropped"
        );
        assert_eq!(newest(&sdks).map(|sdk| sdk.version.as_string()), Some("18.1".to_string()));
    }

    #[test]
    fn simulator_and_unrelated_entries_are_filtered_out() {
        let text = "\
iPhoneSimulator17.5.sdk
iPhoneSimulator18.1.sdk
MacOSX15.2.sdk
WatchOS10.2.sdk
README.md
LICENSE
.github
scripts
iPhoneOS17.5.sdk
";
        let sdks = parse_tree_listing(text);
        let names: Vec<&str> = sdks.iter().map(|sdk| sdk.name.as_str()).collect();
        assert_eq!(names, ["iPhoneOS17.5.sdk"], "only the device SDK survives");
    }

    #[test]
    fn a_listing_with_no_device_sdk_is_empty_not_an_error() {
        assert!(parse_tree_listing("").is_empty(), "an empty listing is empty");
        assert!(parse_tree_listing("\n\n").is_empty(), "blank lines are skipped");
        assert!(parse_tree_listing("README.md\nLICENSE\n").is_empty());
        assert!(newest(&parse_tree_listing("iPhoneSimulator17.5.sdk")).is_none());
    }

    #[test]
    fn messy_whitespace_and_a_trailing_slash_do_not_hide_an_sdk() {
        let text = "  iPhoneOS17.5.sdk  \r\n\t\n iPhoneOS18.0.sdk\nREADME.md\n";
        let sdks = parse_tree_listing(text);
        let names: Vec<&str> = sdks.iter().map(|sdk| sdk.name.as_str()).collect();
        assert_eq!(names, ["iPhoneOS18.0.sdk", "iPhoneOS17.5.sdk"], "18.0 is newer than 17.5");
    }

    #[test]
    fn selection_honours_a_request_and_explains_an_impossible_one() {
        let sdks = parse_tree_listing(REALISTIC_TREE);
        assert_eq!(
            select(&sdks, Some("17.10")).expect("must select").name,
            "iPhoneOS17.10.sdk",
            "a request for 17.10 gets 17.10, not 17.5"
        );
        assert_eq!(
            select(&sdks, Some("iPhoneOS16.4.sdk")).expect("must select").version.as_string(),
            "16.4",
            "a request by directory name works too"
        );
        assert_eq!(select(&sdks, None).expect("must select").name, "iPhoneOS18.1.sdk");
        assert_eq!(
            select(&sdks, Some("  ")).expect("blank means newest").name,
            "iPhoneOS18.1.sdk",
            "a blank request is not a request"
        );

        let error = select(&sdks, Some("26.0")).expect_err("must fail");
        let text = error.to_string();
        assert!(text.contains("26.0"), "names what was asked for: {text}");
        assert!(text.contains("18.1"), "lists what is available: {text}");
        assert!(text.contains("sdk.source"), "offers the override: {text}");
        assert_eq!(error.exit_code(), 4);
    }

    #[test]
    fn an_empty_listing_explains_itself_rather_than_panicking() {
        let error = select(&[], None).expect_err("must fail");
        let text = error.to_string();
        assert!(text.contains("no iPhoneOS"), "says what is wrong: {text}");
        assert!(text.contains("iPhoneOS17.5.sdk"), "shows the expected shape: {text}");
        let with_refs = no_sdks_error(&["HEAD", "main"]);
        assert!(with_refs.to_string().contains("HEAD, main"), "names the refs tried");
    }

    #[test]
    fn the_source_is_configuration_not_a_constant() {
        assert_eq!(resolve_source(None, None), DEFAULT_SOURCE);
        assert_eq!(
            resolve_source(Some("https://mirror.invalid/iOS-SDKs"), None),
            "https://mirror.invalid/iOS-SDKs",
            "sdk.source from the config wins over the default"
        );
        assert_eq!(
            resolve_source(Some("https://mirror.invalid/iOS-SDKs"), Some("https://flag.invalid/s")),
            "https://flag.invalid/s",
            "--source wins over the config"
        );
        assert_eq!(
            resolve_source(Some(""), None),
            DEFAULT_SOURCE,
            "an empty config value falls through instead of being used as a URL"
        );
        assert_eq!(resolve_source(Some("  "), Some("")), DEFAULT_SOURCE, "whitespace is empty");
        assert_eq!(
            resolve_source(Some("  https://spaced.invalid/s  "), None),
            "https://spaced.invalid/s",
            "a padded value is trimmed, not quoted into the command line"
        );
    }
    #[test]
    fn the_advertised_default_ref_is_read_from_symref_output() {
        let stdout = "ref: refs/heads/main\tHEAD\nabc1234\trefs/heads/main\n";
        assert_eq!(parse_symref(stdout).as_deref(), Some("refs/heads/main"));

        let master = "ref: refs/heads/master\tHEAD\ndef5678\trefs/heads/master\n";
        assert_eq!(parse_symref(master).as_deref(), Some("refs/heads/master"));

        // A server that advertises nothing is normal, not a failure.
        assert_eq!(parse_symref("abc1234\trefs/heads/main\n"), None);
        assert_eq!(parse_symref(""), None);
        // `ref:` with a blank target names nothing, so nothing is returned for it.
        assert_eq!(parse_symref("ref:\t\n"), None);
        assert_eq!(parse_symref("ref:   \n"), None);
    }

    #[test]
    fn remote_refs_are_parsed_from_ls_remote_output() {
        let stdout = "abc1234\trefs/heads/main\ndef5678\trefs/tags/v1\n789abcd\tHEAD\n";
        assert_eq!(
            parse_remote_refs(stdout),
            ["refs/heads/main", "refs/tags/v1", "HEAD"],
            "every ref is available as a probe"
        );
        assert!(parse_remote_refs("").is_empty());
        // A warning line mixed into git's output must not become a ref.
        assert!(parse_remote_refs("warning: could not read from remote\n").is_empty());
        assert!(parse_remote_refs("abc\trefs/heads/main\n").is_empty(), "too short to be an object");
    }

    #[test]
    fn the_ref_is_discovered_rather_than_assumed() {
        // A remote that advertises nothing and only has `trunk`: discovery must
        // still find it, because no branch name is hardcoded.
        let runner = |_: Option<&Path>, args: &[&str]| -> Result<String> {
            let asks_about = args.get(2).copied().unwrap_or("");
            Ok(match (args.first().copied(), args.get(1).copied()) {
                (Some("ls-remote"), Some("--symref")) => String::new(),
                (Some("ls-remote"), Some(_)) if args.len() == 2 => {
                    "aaaaaaa\tHEAD\nbbbbbbb\trefs/heads/trunk\n".to_string()
                }
                (Some("ls-remote"), Some(_)) if asks_about.ends_with("trunk") => {
                    "bbbbbbb\trefs/heads/trunk\n".to_string()
                }
                _ => String::new(),
            })
        };
        let (reference, tried) =
            detect_ref(&runner, "https://example.invalid/s", None).expect("a ref must be found");
        assert_eq!(reference, "refs/heads/trunk", "the only real ref wins");
        assert!(tried.contains(&"HEAD".to_string()), "HEAD was tried and rejected: {tried:?}");
    }

    #[test]
    fn a_recorded_ref_is_tried_before_anything_else() {
        let runner = |_: Option<&Path>, args: &[&str]| -> Result<String> {
            let asks_about = args.get(2).copied().unwrap_or("");
            Ok(match (args.first().copied(), args.get(1).copied()) {
                (Some("ls-remote"), Some("--symref")) => "ref: refs/heads/main\tHEAD\n".to_string(),
                (Some("ls-remote"), Some(_)) if asks_about == "my-branch" => {
                    "aaaaaaa\trefs/heads/my-branch\n".to_string()
                }
                (Some("ls-remote"), Some(_)) if args.len() == 2 => "aaaaaaa\trefs/heads/main\n".to_string(),
                _ => String::new(),
            })
        };
        let (reference, _) = detect_ref(&runner, "https://example.invalid/s", Some("my-branch"))
            .expect("a ref must be found");
        assert_eq!(reference, "my-branch", "the recorded ref is preferred");
    }

    #[test]
    fn an_unreachable_source_is_an_actionable_error() {
        let runner = |_: Option<&Path>, _args: &[&str]| -> Result<String> { Ok(String::new()) };
        let error = detect_ref(&runner, "https://example.invalid/s", None)
            .expect_err("nothing resolves, so this must fail");
        let text = error.to_string();
        assert!(text.contains("readable git repository"), "says what is wrong: {text}");
        assert!(text.contains("HEAD"), "names the refs tried: {text}");
        assert_eq!(error.exit_code(), 4);
    }

    #[test]
    fn the_listing_step_runs_against_an_injected_runner_only() {
        // The listing step of discovery, with `git` replaced by canned output.
        // Nothing here touches the network or the user's cache directory.
        let runner = |_: Option<&Path>, args: &[&str]| -> Result<String> {
            Ok(match (args.first().copied(), args.get(1).copied()) {
                (Some("ls-remote"), Some("--symref")) => "ref: refs/heads/main\tHEAD\n".to_string(),
                (Some("ls-remote"), Some(_)) => "aaaaaaa\trefs/heads/main\n".to_string(),
                (Some("ls-tree"), _) => REALISTIC_TREE.to_string(),
                _ => String::new(),
            })
        };
        let (reference, _) = detect_ref(&runner, "https://example.invalid/s", None)
            .expect("a ref must be found");
        let listing = runner
            .run(None, &["ls-tree", "--name-only", "-d", &reference])
            .expect("fake git answers");
        let sdks = parse_tree_listing(&listing);
        assert_eq!(sdks.len(), 11, "11 device SDKs in the canned tree");
        assert_eq!(newest(&sdks).map(|sdk| sdk.version.as_string()), Some("18.1".to_string()));
    }

    #[test]
    fn the_checkout_is_shallow_blobless_and_sparse() {
        let destination = Path::new("/tmp/clone");
        let args = clone_args("https://example.invalid/s", destination);
        assert_eq!(args[0], "clone");
        assert!(args.contains(&"--depth".to_string()), "history stays out: {args:?}");
        assert!(args.contains(&"1".to_string()), "depth 1: {args:?}");
        assert!(
            args.contains(&"--filter=blob:none".to_string()),
            "other SDKs' contents stay out: {args:?}"
        );
        assert!(args.contains(&"--sparse".to_string()), "one directory only: {args:?}");
        // `git clone [options] <source> <destination>`: the source must come
        // before the destination or git will not recognise it as a URL.
        let source_at = args.iter().position(|arg| arg == "https://example.invalid/s");
        let destination_at = args.iter().position(|arg| arg == "/tmp/clone");
        assert_eq!((source_at, destination_at), (Some(5), Some(6)), "wrong order: {args:?}");
        // The destination is normalised, so no `\\?\` path reaches git.
        assert_eq!(args.last().unwrap(), &paths::display_path(destination));

        assert_eq!(sparse_checkout_args("iPhoneOS17.5.sdk"), [
            "sparse-checkout",
            "set",
            "iPhoneOS17.5.sdk"
        ]);
        assert_eq!(ls_tree_args("main"), ["ls-tree", "--name-only", "-d", "main"]);
    }
    #[test]
    fn the_licence_reminder_names_apple_and_the_users_responsibility() {
        let text = license_reminder("https://example.invalid/iOS-SDKs");
        assert!(text.contains("Apple"), "names the rights holder: {text}");
        assert!(text.to_lowercase().contains("licence"), "names the licence: {text}");
        assert!(text.contains("https://example.invalid/iOS-SDKs"), "names the source: {text}");
        assert!(
            text.to_lowercase().contains("comply"),
            "says the user is responsible for compliance: {text}"
        );
        assert!(text.contains("third-party"), "says where the files come from: {text}");
    }

    #[test]
    fn the_licence_reminder_is_required_exactly_once() {
        assert!(
            require_license_acknowledged(true, DEFAULT_SOURCE).is_ok(),
            "an acknowledged reminder does not block the install"
        );
        let error =
            require_license_acknowledged(false, DEFAULT_SOURCE).expect_err("must not proceed");
        let text = error.to_string();
        assert!(text.contains("Apple"), "the reminder travels with the error: {text}");
        assert!(text.to_lowercase().contains("licence"), "so it cannot be missed: {text}");
        assert_eq!(error.exit_code(), 4);
    }

    #[test]
    fn an_install_is_refused_before_any_network_call_without_acceptance() {
        let runner = |_: Option<&Path>, _args: &[&str]| -> Result<String> {
            panic!("git must not run before the licence is acknowledged")
        };
        let sdk = RemoteSdk::from_name("iPhoneOS17.5.sdk").expect("must parse");
        let error =
            install(DEFAULT_SOURCE, &sdk, &runner, false).expect_err("must not proceed");
        assert!(error.to_string().contains("Apple"), "the reminder is the blocker");
    }

    /// Build a directory that looks like an SDK checkout whose symlinks git
    /// wrote as text stubs.
    fn stub_sdk(tag: &str) -> (PathBuf, PathBuf) {
        let dir = scratch(tag);
        let sdk = dir.join("iPhoneOS17.5.sdk");
        std::fs::create_dir_all(sdk.join("usr").join("lib")).expect("mkdir");
        std::fs::create_dir_all(sdk.join("usr").join("include")).expect("mkdir");
        std::fs::create_dir_all(sdk.join("System").join("Library").join("Frameworks"))
            .expect("mkdir");
        std::fs::write(sdk.join("usr").join("include").join("stdio.h"), b"#pragma once\n")
            .expect("write");
        std::fs::write(sdk.join("usr").join("lib").join("libSystem.B.tbd"), b"--- tbd ---")
            .expect("write");
        // A stub: a plain file whose whole content is a relative path to
        // something that exists next to it.
        std::fs::write(sdk.join("usr").join("lib").join("libSystem.tbd"), "../include/stdio.h")
            .expect("write");
        (dir, sdk)
    }

    #[test]
    fn a_text_stub_checkout_is_detected_and_reported() {
        let (dir, sdk) = stub_sdk("stubs");
        let report = detect_symlinks(&sdk);
        assert_eq!(report.state, SymlinkState::TextStub, "the stub must be seen: {report:?}");
        assert_eq!(report.stubs, 1);
        assert!(!report.ok(), "a stub checkout is not ok");
        assert!(report.state.is_broken());
        let example = report.example.as_ref().expect("an example path");
        assert!(example.ends_with("libSystem.tbd"), "points at the stub: {}", example.display());

        let warning = symlink_warning(&report).expect("a broken checkout must warn");
        assert!(warning.contains("core.symlinks"), "names the cause: {warning}");
        assert!(warning.contains("Developer Mode"), "offers the fix: {warning}");
        assert!(warning.contains("1 text stub"), "counts the stubs: {warning}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_tree_with_no_symlinks_is_not_warned_about() {
        let dir = scratch("no-symlinks");
        std::fs::create_dir_all(dir.join("usr").join("include")).expect("mkdir");
        std::fs::write(dir.join("usr").join("include").join("stdio.h"), b"#pragma once\n")
            .expect("write");
        // A text file that merely starts with "./" but names nothing must not be
        // mistaken for a stub.
        std::fs::write(dir.join("notes.txt"), "./this-does-not-exist\n").expect("write");
        let report = detect_symlinks(&dir);
        assert_eq!(report.state, SymlinkState::Absent, "{report:?}");
        assert_eq!(report.stubs, 0);
        assert!(report.ok());
        assert_eq!(symlink_warning(&report), None, "nothing to warn about");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_symlink_is_reported_as_real_where_one_can_be_made() {
        let dir = scratch("real-symlink");
        std::fs::write(dir.join("target.txt"), b"hello").expect("write");
        // Creating a symlink needs a privilege on Windows; where it is not
        // available the tree simply reads as "no symlinks", which is the honest
        // answer rather than a failure.
        #[cfg(unix)]
        std::os::unix::fs::symlink("target.txt", dir.join("link.txt")).expect("symlink");
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_file(dir.join("target.txt"), dir.join("link.txt"));

        let report = detect_symlinks(&dir);
        assert_eq!(report.stubs, 0, "a real symlink is never a stub: {report:?}");
        if report.real > 0 {
            assert_eq!(report.state, SymlinkState::Real);
            assert!(report.ok(), "real symlinks are fine");
            assert_eq!(symlink_warning(&report), None);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn scanning_a_missing_directory_is_empty_rather_than_a_panic() {
        let report = detect_symlinks(Path::new("/definitely/not/here"));
        assert_eq!(report.state, SymlinkState::Absent);
        assert_eq!(report.stubs, 0);
        assert_eq!(report.real, 0);
        assert!(report.ok());
        assert_eq!(SymlinkState::TextStub.label(), "text stubs, not symlinks");
        assert_eq!(SymlinkState::Real.label(), "real symlinks");
    }

    #[test]
    fn a_truncated_checkout_fails_validation_with_the_retry_offered() {
        let (dir, sdk) = stub_sdk("validate-bad");
        // Remove one required subdirectory so `Sdk::open` rejects the layout.
        std::fs::remove_dir_all(sdk.join("System")).expect("rm");
        let error = validate_install(&sdk).expect_err("a partial checkout must be rejected");
        let text = error.to_string();
        assert!(text.contains("System/Library/Frameworks"), "names what is missing: {text}");
        assert!(text.contains("install again"), "offers the retry: {text}");
        assert!(text.contains(&paths::display_path(&sdk)), "names the directory: {text}");
        assert_eq!(error.exit_code(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_complete_checkout_passes_validation() {
        let (dir, sdk) = stub_sdk("validate-ok");
        validate_install(&sdk).expect("a complete layout must pass");
        let opened = Sdk::open(&sdk).expect("Sdk::open must accept it");
        assert_eq!(opened.root, sdk);
        assert!(!opened.has_framework("UIKit"), "no frameworks in the fake tree");
        assert!(opened.framework_search_path().ends_with("System/Library/Frameworks"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_sdk_is_installed_under_its_version_not_its_source_name() {
        let version = version("17.10");
        let dir = install_dir(&version).expect("the sdks dir must be derivable");
        assert!(dir.ends_with(Path::new("sdks").join("17.10")), "got {}", dir.display());
        assert!(
            dir.starts_with(paths::sdks_dir().expect("sdks dir")),
            "an SDK lives under the managed sdks dir"
        );
        assert!(clone_dir().expect("cache dir").ends_with("sdk-source"));
    }

    #[test]
    fn a_git_command_renders_for_an_error_message() {
        let command = GitCommand {
            cwd: Some(PathBuf::from("/tmp/clone")),
            args: vec!["clone".to_string(), "--depth".to_string(), "1".to_string()],
        };
        assert_eq!(command.display(), "git clone --depth 1");
        assert_eq!(command.cwd.as_deref(), Some(Path::new("/tmp/clone")));
    }

    #[test]
    fn a_remote_sdk_renders_with_its_name_and_version() {
        let sdk = RemoteSdk::from_name("iPhoneOS17.10.sdk").expect("must parse");
        assert_eq!(sdk.to_string(), "iPhoneOS17.10.sdk (17.10)");
        assert!(sdk > RemoteSdk::from_name("iPhoneOS17.5.sdk").expect("must parse"));
        assert!(sdk < RemoteSdk::from_name("iPhoneOS18.0.sdk").expect("must parse"));
    }

    #[test]
    fn the_clone_never_overwrites_a_directory_it_does_not_own() {
        // The cache lives under our own data dir, so a fake one elsewhere is
        // refused rather than deleted.
        let stranger = scratch("not-ours");
        let error = remove_dir_if_managed(&stranger).expect_err("must refuse");
        assert!(error.to_string().contains("refusing"), "says why: {error}");
        assert!(stranger.is_dir(), "and leaves the directory alone");
        let _ = std::fs::remove_dir_all(&stranger);
    }
}


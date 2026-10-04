//! Finding the external tools by *logical name*, never by a fixed version or a
//! fixed path.
//!
//! A machine that can build an `.ipa` has a clang, a Mach-O linker, an `ldid`,
//! a `git` and — optionally — a `zip`. None of them ships at one canonical
//! location: LLVM installs version-suffixed binaries (`clang-21`,
//! `ld64.lld-21`) side by side, Homebrew symlinks an unversioned `clang`,
//! Windows adds `.exe`/`.cmd` to every name, and darwinforge's own `bootstrap`
//! builds `ldid` into its private `bin` directory. So resolution walks an
//! explicit order and, wherever several candidates match, takes the **newest**
//! one rather than the first found:
//!
//! 1. an explicitly configured path (global config or a `--tool` flag),
//! 2. darwinforge's own managed [`paths::bin_dir`] — only where it makes sense,
//!    which today means `ldid`, the tool bootstrap builds from source,
//! 3. the exact name on `PATH`,
//! 4. `<name>-<N>` on `PATH`, newest `N` first (see [`VERSION_SEARCH_WINDOW`]),
//! 5. the install prefixes LLVM is known to use on this OS.
//!
//! Every `PATH` lookup goes through an injected `which`-style closure, so the
//! whole search is unit-tested against a fake `PATH` directory of fake files: no
//! real toolchain, no network, and no dependence on what happens to be installed
//! on the machine running the tests.

use std::path::{Path, PathBuf};

use crate::distro::OperatingSystem;
use crate::error::Error;
use crate::exec;
use crate::paths;

/// Oldest `<name>-<N>` scanned for in step 4 of the search.
///
/// # This is a search window, not a version pin
///
/// darwinforge makes no claim about a maximum LLVM version. Anything newer than
/// [`NEWEST_VERSION_SCANNED`] falls outside the scan, and the "not found" error
/// says so explicitly, naming both the window and the environment variable that
/// overrides it — a user is never left guessing why their tool was skipped. A
/// caller that knows about a newer LLVM can widen the range with
/// [`find_in_window`].
pub const OLDEST_VERSION_SCANNED: u32 = 6;

/// Newest `<name>-<N>` scanned for in step 4. See [`OLDEST_VERSION_SCANNED`]: a
/// boundary of the scan, never a supported-version ceiling.
pub const NEWEST_VERSION_SCANNED: u32 = 64;

/// The default `(oldest, newest)` pair scanned for versioned names. Reported in
/// the "not found" error so the window is always visible.
pub const VERSION_SEARCH_WINDOW: (u32, u32) = (OLDEST_VERSION_SCANNED, NEWEST_VERSION_SCANNED);

/// What a tool is, and the binary names that satisfy it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSpec {
    /// Logical name, used in messages and as the key in the global config.
    pub name: &'static str,
    /// Binary names to try, in order, for the unversioned search.
    pub candidates: &'static [&'static str],
    /// Whether `<candidate>-<N>` names should be searched for.
    pub versioned: bool,
    /// Whether darwinforge's own managed `bin_dir()` is consulted.
    pub search_managed: bool,
    /// Whether a build cannot proceed without this tool.
    pub required: bool,
    /// The `DARWINFORGE_*` suffix an explicit override uses.
    pub env_suffix: &'static str,
    /// The fix hint carried by the "not found" error.
    pub fix_hint: &'static str,
}

/// The C compiler. `clang++` is accepted so a pure C++ project still builds.
pub const CLANG: ToolSpec = ToolSpec {
    name: "clang",
    candidates: &["clang", "clang++"],
    versioned: true,
    search_managed: false,
    required: true,
    env_suffix: "CLANG",
    fix_hint: "install LLVM/clang (`apt install clang`, `dnf install clang`) or put the \
              binary on PATH, or set DARWINFORGE_CLANG=/path/to/clang. Run `darwinforge \
              doctor` for a full report",
};

/// LLVM's Mach-O linker, the `ld` that links Mach-O. cctools `ld` is available
/// separately as [`CCTOOLS_LD`], so a caller can fall back to it only after this
/// fails (see [`crate::sdk::Toolchain`]).
pub const LINKER: ToolSpec = ToolSpec {
    name: "ld64.lld",
    candidates: &["ld64.lld"],
    versioned: true,
    search_managed: false,
    required: true,
    env_suffix: "LINKER",
    fix_hint: "install LLVM's lld (`apt install lld`, `dnf install lld`), which ships \
              ld64.lld, or set DARWINFORGE_LINKER=/path/to/ld64.lld",
};

/// The ad-hoc signer. `bootstrap` builds it from source into
/// [`paths::bin_dir`], so the managed directory is searched for this tool.
pub const LDID: ToolSpec = ToolSpec {
    name: "ldid",
    candidates: &["ldid"],
    versioned: false,
    search_managed: true,
    required: true,
    env_suffix: "LDID",
    fix_hint: "install ldid (https://github.com/ProcursusTeam/ldid or your package \
              manager), or run `darwinforge bootstrap`, which builds it into the \
              darwinforge data directory, or set DARWINFORGE_LDID=/path/to/ldid",
};

/// `git`, used to fetch SDKs from a source repository (see
/// [`crate::sdksource`]). Needed only on the SDK download path.
pub const GIT: ToolSpec = ToolSpec {
    name: "git",
    candidates: &["git"],
    versioned: false,
    search_managed: false,
    required: false,
    env_suffix: "GIT",
    fix_hint: "install git (`apt install git`, `dnf install git`), which is how SDKs are \
              fetched from the configured source repository",
};

/// Optional external packager; without it the built-in zip writer is used.
pub const ZIP: ToolSpec = ToolSpec {
    name: "zip",
    candidates: &["zip"],
    versioned: false,
    search_managed: false,
    required: false,
    env_suffix: "ZIP",
    fix_hint: "install the `zip` command (Info-ZIP), or set `zip = false` in \
              darwinforge.toml to use the built-in writer",
};

/// Every tool darwinforge knows how to look for, in reporting order.
pub fn all_specs() -> &'static [ToolSpec] {
    &[CLANG, LINKER, CCTOOLS_LD, LDID, GIT, ZIP]
}

/// Where a resolved binary came from, for `doctor` and for messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The user (or the saved config) named the path explicitly.
    Configured,
    /// darwinforge's own managed `bin_dir()` — a locally built `ldid`.
    ManagedBin,
    /// The unversioned name, found on `PATH`.
    PathName,
    /// A `<name>-<N>` name, found on `PATH`.
    PathVersion,
    /// An LLVM install prefix such as `/usr/lib/llvm-21/bin`.
    InstallPrefix,
}

impl Origin {
    /// Lowercase label for output.
    pub fn label(self) -> &'static str {
        match self {
            Origin::Configured => "configured path",
            Origin::ManagedBin => "darwinforge bin dir",
            Origin::PathName => "PATH",
            Origin::PathVersion => "PATH (versioned)",
            Origin::InstallPrefix => "LLVM install prefix",
        }
    }

    /// Tie-break weight: a more explicit origin wins when versions are equal.
    fn rank(self) -> u32 {
        match self {
            Origin::Configured => 4,
            Origin::ManagedBin => 3,
            Origin::PathVersion => 2,
            Origin::PathName => 1,
            Origin::InstallPrefix => 0,
        }
    }
}

/// A binary that satisfies a [`ToolSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundTool {
    /// The binary to invoke.
    pub path: PathBuf,
    /// [`ToolSpec::name`] of the spec that matched.
    pub spec: &'static str,
    /// Which step of the search found it.
    pub origin: Origin,
    /// The version encoded in the binary's own name, when it has one.
    pub version: Option<u32>,
}

/// Runs `<binary> --version` and returns its first line, or `None`.
///
/// Injected so tests never spawn a process, and tolerant on purpose: a binary
/// that is missing, unreadable or prints nothing is not an error, it is simply
/// "version unknown".
pub type VersionProbe<'a> = &'a dyn Fn(&Path) -> Option<String>;

impl FoundTool {
    /// The file name of the binary, e.g. `clang-21` or `ldid.exe`.
    pub fn binary_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.to_string_lossy().to_string())
    }

    /// Sort key implementing "newest wins".
    ///
    /// The version in the name comes first, so `clang-21` beats `clang-17`
    /// *and* beats an unversioned `clang` (which counts as no version). The
    /// origin only breaks ties between equally versioned candidates.
    pub fn precedence(&self) -> (u32, u32) {
        (self.version.unwrap_or(0), self.origin.rank())
    }

    /// The version string the binary reports for itself, if it will say.
    pub fn reported_version(&self, probe: VersionProbe<'_>) -> Option<String> {
        detect_version(&self.path, probe)
    }

    /// One human-readable line: name, version, origin and path.
    pub fn describe(&self) -> String {
        let version = match self.version {
            Some(version) => format!(" v{version}"),
            None => String::new(),
        };
        format!(
            "{}{version} ({}) at {}",
            self.spec,
            self.origin.label(),
            paths::display_path(&self.path)
        )
    }
}

/// cctools-port's `ld`, kept as its own spec so it can be searched for only
/// after [`LINKER`] has failed.
pub const CCTOOLS_LD: ToolSpec = ToolSpec {
    name: "ld",
    candidates: &["ld"],
    versioned: false,
    search_managed: false,
    required: false,
    env_suffix: "LINKER",
    fix_hint: "install cctools-port for Linux, which provides `ld`, or install LLVM's \
              lld for ld64.lld, or set DARWINFORGE_LINKER",
};
/// The inputs of one search: the host OS, the explicit overrides, and the
/// `PATH` lookup to use.
pub struct Lookup<'a> {
    /// Which suffix and prefix rules apply.
    pub os: OperatingSystem,
    /// An explicit path from the global config or a `--tool` flag.
    pub configured: Option<PathBuf>,
    /// darwinforge's managed `bin_dir()`, when it could be determined.
    pub managed_bin: Option<PathBuf>,
    /// The directory scanned for `llvm-<N>` installs.
    pub lib_dir: PathBuf,
    /// Install prefixes to search, before the OS conventions are applied.
///
/// `None` (the production value) means "use the OS conventions". `Some(list)`
/// replaces them **entirely**, which is what lets a test point the search at a
/// scratch directory instead of the real `C:\Program Files\LLVM` that no amount
/// of `with_lib_dir` can override.
pub extra_prefixes: Option<Vec<PathBuf>>,
    /// The `PATH` lookup: a program name in, its full path out.
    pub which: &'a dyn Fn(&str) -> Option<PathBuf>,
}

impl<'a> Lookup<'a> {
    /// A lookup with production defaults for `os`, using `which` for `PATH`.
    pub fn new(os: OperatingSystem, which: &'a dyn Fn(&str) -> Option<PathBuf>) -> Self {
        Lookup {
            os,
            configured: None,
            managed_bin: None,
            lib_dir: default_lib_dir(os),
            extra_prefixes: None,
            which,
        }
    }

    /// The explicit override for this tool, if any.
    pub fn with_configured(mut self, configured: Option<&Path>) -> Self {
        self.configured = configured.map(Path::to_path_buf);
        self
    }

    /// The managed `bin_dir()` to search for locally built tools.
    pub fn with_managed_bin(mut self, managed_bin: Option<PathBuf>) -> Self {
        self.managed_bin = managed_bin;
        self
    }

    /// The directory scanned for `llvm-<N>` installs.
    pub fn with_lib_dir(mut self, lib_dir: PathBuf) -> Self {
        self.lib_dir = lib_dir;
        self
    }

    /// Extra install prefixes to try instead of the OS conventions.
    ///
    /// Tests pass an empty list so a search cannot escape into the real
    /// `C:\Program Files\LLVM`. Production never calls it.
    pub fn with_extra_prefixes(mut self, prefixes: Vec<PathBuf>) -> Self {
        self.extra_prefixes = Some(prefixes);
        self
    }

    /// A lookup for `spec` against this machine, honouring
    /// [`ToolSpec::search_managed`].
    pub fn production(spec: &ToolSpec, configured: Option<&Path>) -> Lookup<'static> {
        let os = crate::distro::detect_os();
        let managed_bin = if spec.search_managed { paths::bin_dir().ok() } else { None };
        Lookup {
            os,
            configured: configured.map(Path::to_path_buf),
            managed_bin,
            lib_dir: default_lib_dir(os),
            extra_prefixes: None,
            which: &SYSTEM_WHICH,
        }
    }
}

/// The real `PATH` lookup, as a `'static` reference so a [`Lookup`] can hold it.
static SYSTEM_WHICH: fn(&str) -> Option<PathBuf> = system_which;

fn system_which(program: &str) -> Option<PathBuf> {
    exec::which(program)
}

/// Find a tool on this machine using production defaults.
pub fn find(spec: &ToolSpec, configured: Option<&Path>) -> Option<FoundTool> {
    find_in(&Lookup::production(spec, configured), spec)
}

/// Find a tool, or fail with an error that says exactly what to do.
///
/// `configured` is the explicit path from the global config or a `--tool` flag.
/// It is honoured when it exists; when it does not, the search continues and
/// the "not found" error names the stale path, so a deleted config value can
/// never hide behind a vague "not on PATH".
pub fn require(spec: &ToolSpec, configured: Option<&Path>) -> Result<FoundTool, Error> {
    find(spec, configured).ok_or_else(|| not_found(spec, configured))
}
/// Find `spec` using an injected `PATH` lookup. See the module docs for the
/// order the steps are tried in.
pub fn find_in(lookup: &Lookup<'_>, spec: &ToolSpec) -> Option<FoundTool> {
    find_in_window(lookup, spec, VERSION_SEARCH_WINDOW.0, VERSION_SEARCH_WINDOW.1)
}

/// Find `spec`, scanning `<name>-<N>` for `N` in `oldest..=newest`.
///
/// The window is a parameter so it is visibly a search range rather than a
/// version policy baked into the logic.
pub fn find_in_window(
    lookup: &Lookup<'_>,
    spec: &ToolSpec,
    oldest: u32,
    newest: u32,
) -> Option<FoundTool> {
    // 1. An explicitly configured path always wins.
    if let Some(configured) = &lookup.configured {
        if configured.is_file() {
            return Some(found(configured.clone(), spec, Origin::Configured, None));
        }
    }
    // 2. darwinforge's own bin dir, where `bootstrap` puts a locally built ldid.
    if spec.search_managed {
        if let Some(managed) = &lookup.managed_bin {
            if let Some(path) = first_named_in(std::slice::from_ref(managed), spec, lookup.os) {
                return Some(found(path, spec, Origin::ManagedBin, None));
            }
        }
    }
    // 3 + 4. PATH: exact names and versioned names together, newest wins.
    if let Some(best) = best_on_path(spec, oldest, newest, lookup.which) {
        return Some(best);
    }
    // 5. LLVM install prefixes, still newest-first.
    best_in_prefixes(
        spec,
        lookup.os,
        oldest,
        newest,
        &llvm_bin_dirs_in(lookup.os, &lookup.lib_dir, lookup.extra_prefixes.as_ref()),
    )
}

fn found(path: PathBuf, spec: &ToolSpec, origin: Origin, version: Option<u32>) -> FoundTool {
    let version = version.or_else(|| parse_versioned_name(&path.to_string_lossy()));
    FoundTool { path, spec: spec.name, origin, version }
}

/// Steps 3 and 4: every name the spec knows, exact and versioned, on `PATH`.
fn best_on_path(
    spec: &ToolSpec,
    oldest: u32,
    newest: u32,
    which: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<FoundTool> {
    let mut best: Option<FoundTool> = None;
    for base in spec.candidates {
        if let Some(path) = which(base) {
            keep_newest(&mut best, found(path, spec, Origin::PathName, None));
        }
        if spec.versioned {
            for number in version_numbers(oldest, newest) {
                let name = format!("{base}-{number}");
                if let Some(path) = which(&name) {
                    keep_newest(&mut best, found(path, spec, Origin::PathVersion, Some(number)));
                }
            }
        }
    }
    best
}

/// Replace `best` when `candidate` has a higher precedence, i.e. is newer.
fn keep_newest(best: &mut Option<FoundTool>, candidate: FoundTool) {
    let better = match best {
        None => true,
        Some(current) => candidate.precedence() > current.precedence(),
    };
    if better {
        *best = Some(candidate);
    }
}

/// The numbers `oldest..=newest`, newest first, so the first hit of a complete
/// `PATH` is also the best one. A reversed window is normalised rather than
/// yielding nothing.
pub fn version_numbers(oldest: u32, newest: u32) -> impl Iterator<Item = u32> {
    let low = oldest.min(newest);
    let high = oldest.max(newest);
    (low..=high).rev()
}

/// Step 5: the newest match across LLVM install prefixes, earlier prefixes
/// winning ties because they are the ones a user installed deliberately.
fn best_in_prefixes(
    spec: &ToolSpec,
    os: OperatingSystem,
    oldest: u32,
    newest: u32,
    prefixes: &[PathBuf],
) -> Option<FoundTool> {
    let mut best: Option<FoundTool> = None;
    for (index, prefix) in prefixes.iter().enumerate() {
        if let Some(path) = first_named_in(std::slice::from_ref(prefix), spec, os) {
            let tool = found(path, spec, Origin::InstallPrefix, None);
            keep_new_prefix(&mut best, tool, index);
        }
        if !spec.versioned {
            continue;
        }
        for number in version_numbers(oldest, newest) {
            for base in spec.candidates {
                let name = format!("{base}-{number}");
                if let Some(path) = which_in_dirs(std::slice::from_ref(prefix), &name, os) {
                    let tool = found(path, spec, Origin::InstallPrefix, Some(number));
                    keep_new_prefix(&mut best, tool, index);
                }
            }
        }
    }
    best
}

/// Like [`keep_newest`], but for the prefix scan: a versioned build still beats
/// an unversioned one, and at equal versions the earlier prefix wins.
fn keep_new_prefix(best: &mut Option<FoundTool>, candidate: FoundTool, index: usize) {
    let rank = |version: Option<u32>| (version.unwrap_or(0), u32::MAX - index as u32);
    let better = match best {
        None => true,
        Some(current) => {
            let candidate_rank = rank(candidate.version);
            let current_rank = rank(current.version);
            match candidate_rank.0.cmp(&current_rank.0) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Less => false,
                std::cmp::Ordering::Equal => candidate_rank.1 > current_rank.1,
            }
        }
    };
    if better {
        *best = Some(candidate);
    }
}

/// The first of the spec's unversioned names present in `dirs`.
fn first_named_in(dirs: &[PathBuf], spec: &ToolSpec, os: OperatingSystem) -> Option<PathBuf> {
    spec.candidates.iter().find_map(|name| which_in_dirs(dirs, name, os))
}
/// `which`, over an explicit list of directories instead of the process `PATH`.
///
/// Uses [`OperatingSystem::binary_suffixes`], so on Windows `clang.exe`,
/// `clang.cmd` and `clang.bat` all satisfy `clang` while on Linux only the bare
/// name does. This is what makes the whole search testable with a fake `PATH`.
pub fn which_in_dirs(dirs: &[PathBuf], name: &str, os: OperatingSystem) -> Option<PathBuf> {
    for dir in dirs {
        for suffix in os.binary_suffixes() {
            let candidate = dir.join(format!("{name}{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The directories that may hold an LLVM install, best first.
///
/// * Linux/WSL: `<lib_dir>/llvm-<N>/bin`, newest `N` first (`lib_dir` is
///   `/usr/lib`).
/// * macOS: Homebrew on Apple silicon, Homebrew on Intel, their unversioned
///   links, then MacPorts.
/// * Windows: the standard LLVM installer location, then any `llvm-<N>` trees.
///
/// The list is ordered, not filtered: a prefix that does not exist costs one
/// `is_file` check and nothing else.
///
/// `overrides` replaces the OS-specific list **entirely** when it is `Some`, and
/// prepends to it when it is `None`. That distinction exists so a test can
/// neutralise the hardcoded `C:\Program Files\LLVM\bin` without changing what
/// production searches.
pub fn llvm_bin_dirs_in(
    os: OperatingSystem,
    lib_dir: &Path,
    overrides: Option<&Vec<PathBuf>>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    match overrides {
        Some(list) => dirs.extend(list.iter().cloned()),
        None => match os {
            OperatingSystem::Linux | OperatingSystem::Wsl => {}
            OperatingSystem::MacOs => dirs.extend(
                [
                    "/opt/homebrew/opt/llvm/bin",
                    "/usr/local/opt/llvm/bin",
                    "/usr/local/bin",
                    "/opt/local/bin",
                ]
                .iter()
                .map(PathBuf::from),
            ),
            OperatingSystem::Windows => dirs.push(PathBuf::from(r"C:\Program Files\LLVM\bin")),
        },
    }
    dirs.extend(llvm_globs(lib_dir));
    dirs
}

/// The install prefixes for `os`, including the conventional ones.
pub fn llvm_bin_dirs(os: OperatingSystem, lib_dir: &Path) -> Vec<PathBuf> {
    llvm_bin_dirs_in(os, lib_dir, None)
}

/// Where LLVM installs its versioned trees on this OS, by convention.
pub fn default_lib_dir(os: OperatingSystem) -> PathBuf {
    match os {
        OperatingSystem::Linux | OperatingSystem::Wsl => PathBuf::from("/usr/lib"),
        OperatingSystem::MacOs => PathBuf::from("/usr/local/lib"),
        OperatingSystem::Windows => PathBuf::from(r"C:\Program Files\LLVM"),
    }
}

/// `<lib_dir>/llvm-<N>/bin` for each version found, newest first.
fn llvm_globs(lib_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(lib_dir) else { return Vec::new() };
    let mut found: Vec<(u32, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let version = name.strip_prefix("llvm-").and_then(trailing_number)?;
            Some((version, entry.path().join("bin")))
        })
        .collect();
    found.sort_by_key(|(version, _)| std::cmp::Reverse(*version));
    found.into_iter().map(|(_, path)| path).collect()
}

/// The numeric suffix of a versioned binary or directory name.
///
/// `clang-21` and `ld64.lld-180` yield a version; a bare `clang`, a
/// non-numeric suffix and a dotted suffix all yield `None`, so nothing is ever
/// guessed. A Windows extension (`clang-21.exe`) is stripped first, and the
/// whole remaining text after the last `-` must be digits — `clang-21.1` is
/// not a binary name this search should trust.
pub fn parse_versioned_name(name: &str) -> Option<u32> {
    let file_name = Path::new(name).file_name()?.to_string_lossy().to_string();
    let without_suffix = strip_known_suffix(&file_name);
    let (_, number) = without_suffix.rsplit_once('-')?;
    trailing_number(number)
}

/// `Some(21)` for a bare `"21"`, `None` for anything containing a non-digit.
fn trailing_number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Drop a Windows executable suffix, whichever of them is present.
fn strip_known_suffix(name: &str) -> &str {
    for suffix in OperatingSystem::Windows.binary_suffixes() {
        if !suffix.is_empty() {
            if let Some(stripped) = name.strip_suffix(suffix) {
                return stripped;
            }
        }
    }
    name
}
/// Ask a binary for its version through an injected probe.
///
/// Blank output counts as "no version": a tool that prints nothing is not a
/// failure, it is simply unknown.
pub fn detect_version(binary: &Path, probe: VersionProbe<'_>) -> Option<String> {
    let reported = probe(binary)?;
    let line = reported.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// The production version probe: `<binary> --version`, first line, tolerantly.
/// A binary that is missing or refuses to be run yields `None`.
pub fn probe_version(binary: &Path) -> Option<String> {
    let program = paths::display_path(binary);
    exec::probe(&program, &["--version".to_string()]).ok().flatten()
}

/// The environment variable a user would set to override this tool.
pub fn env_name(spec: &ToolSpec) -> String {
    format!("DARWINFORGE_{}", spec.env_suffix)
}

/// The error returned when a required tool cannot be found.
///
/// Names the search order that was tried and, crucially, the version window: a
/// user whose `clang-99` was outside it must be told that, not left guessing.
pub fn not_found(spec: &ToolSpec, configured: Option<&Path>) -> Error {
    let mut tried = vec![match configured {
        Some(path) => {
            format!("the configured path {} (which does not exist)", path.display())
        }
        None => "the configured path (none set)".to_string(),
    }];
    tried.push("the exact name on PATH".to_string());
    if spec.search_managed {
        tried.push(match paths::bin_dir() {
            Ok(dir) => format!("the darwinforge bin dir {}", dir.display()),
            Err(_) => "the darwinforge bin dir (which could not be determined)".to_string(),
        });
    }
    if spec.versioned {
        tried.push(format!(
            "{} on PATH for versions {}..={}",
            spec.name, VERSION_SEARCH_WINDOW.0, VERSION_SEARCH_WINDOW.1
        ));
        tried.push("the LLVM install prefixes".to_string());
    }
    Error::Prereq {
        what: format!("no `{}` found; tried {}", spec.name, tried.join(", ")),
        fix: format!(
            "{hint}. If your {name} is newer than version {newest} it falls outside that \
             search window: set {var}=/path/to/{name} to use it anyway.",
            hint = spec.fix_hint,
            name = spec.name,
            newest = VERSION_SEARCH_WINDOW.1,
            var = env_name(spec),
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("darwinforge-toolfind-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// Create a fake binary. Its contents never matter: nothing is executed.
    fn fake_binary(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("write fake binary");
        path
    }

    /// A `which` closure over one directory — i.e. a fake `PATH`.
    fn fake_which(dir: PathBuf, os: OperatingSystem) -> impl Fn(&str) -> Option<PathBuf> {
        move |name| which_in_dirs(std::slice::from_ref(&dir), name, os)
    }

    fn host_os() -> OperatingSystem {
        crate::distro::detect_os()
    }

    /// A `Lookup` that can only ever find things inside `tag`'s scratch space.
    ///
    /// `Lookup::new` defaults to the machine's **real** LLVM locations — on
    /// Windows that is a hardcoded `C:\Program Files\LLVM\bin`, which no amount
    /// of `with_lib_dir` can override. A test that forgets to override it
    /// therefore passes or fails depending on what the developer has installed,
    /// which is how four of these tests started failing the moment LLVM was
    /// installed on this machine. Emptying `extra_prefixes` and pointing
    /// `lib_dir` at an empty scratch directory closes that hole, so every test
    /// asserting an absolute outcome uses this and the suite stays hermetic.
    fn hermetic_lookup<'a>(
        which: &'a dyn Fn(&str) -> Option<PathBuf>,
        os: OperatingSystem,
        tag: &str,
    ) -> Lookup<'a> {
        let lib = scratch(&format!("{tag}-lib"));
        Lookup::new(os, which)
            .with_lib_dir(lib)
            .with_extra_prefixes(Vec::new())
    }

    #[test]
    fn versioned_names_yield_their_number() {
        assert_eq!(parse_versioned_name("clang-21"), Some(21));
        assert_eq!(parse_versioned_name("ld64.lld-180"), Some(180));
        assert_eq!(parse_versioned_name("clang-21.exe"), Some(21));
        assert_eq!(parse_versioned_name("ld64.lld-180.cmd"), Some(180));
        assert_eq!(parse_versioned_name("/usr/lib/llvm-21/bin/clang-19"), Some(19));
    }

    #[test]
    fn an_unversioned_name_yields_no_version() {
        assert_eq!(parse_versioned_name("clang"), None, "no suffix at all");
        assert_eq!(parse_versioned_name("clang++"), None, "'++' is not a number");
        assert_eq!(parse_versioned_name("clang-nightly"), None, "not a number");
        assert_eq!(parse_versioned_name("clang-"), None, "empty suffix");
        assert_eq!(parse_versioned_name("clang-21.1"), None, "dotted, so not a binary name");
        assert_eq!(parse_versioned_name(""), None);
    }

    #[test]
    fn the_newest_versioned_candidate_on_path_is_chosen() {
        let os = host_os();
        let dir = scratch("newest");
        let oldest = fake_binary(&dir, "clang-17");
        let newest = fake_binary(&dir, "clang-21");
        fake_binary(&dir, "clang-14");
        let which = fake_which(dir.clone(), os);
        let lookup = Lookup::new(os, &which);

        let found = find_in(&lookup, &CLANG).expect("clang must be found");
        assert_eq!(found.path, newest, "the newest version wins, not the first");
        assert_ne!(found.path, oldest);
        assert_eq!(found.version, Some(21));
        assert_eq!(found.origin, Origin::PathVersion);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_versioned_binary_outranks_an_unversioned_one_on_the_same_path() {
        let os = host_os();
        let dir = scratch("outrank");
        let bare = fake_binary(&dir, "clang");
        let numbered = fake_binary(&dir, "clang-21");
        let which = fake_which(dir.clone(), os);
        let lookup = Lookup::new(os, &which);

        let found = find_in(&lookup, &CLANG).expect("clang must be found");
        assert_eq!(found.path, numbered, "a newer numbered build beats the bare symlink");
        assert_ne!(found.path, bare);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_exact_name_is_used_when_no_versioned_sibling_exists() {
        let os = host_os();
        let dir = scratch("exact");
        let only = fake_binary(&dir, "clang");
        let which = fake_which(dir.clone(), os);
        let lookup = Lookup::new(os, &which);

        let found = find_in(&lookup, &CLANG).expect("clang must be found");
        assert_eq!(found.path, only);
        assert_eq!(found.version, None, "an unversioned name reports no version");
        assert_eq!(found.origin, Origin::PathName);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_candidate_name_is_tried_in_order() {
        let os = host_os();
        let dir = scratch("candidates");
        let plus_plus = fake_binary(&dir, "clang++");
        let which = fake_which(dir.clone(), os);
        let lookup = Lookup::new(os, &which);

        let found = find_in(&lookup, &CLANG).expect("clang++ must satisfy clang");
        assert_eq!(found.path, plus_plus, "clang++ is the second candidate");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_linker_is_found_by_its_own_versioned_name() {
        let os = host_os();
        let dir = scratch("linker");
        let expected = fake_binary(&dir, "ld64.lld-21");
        let which = fake_which(dir.clone(), os);
        let lookup = Lookup::new(os, &which);

        let found = find_in(&lookup, &LINKER).expect("ld64.lld must be found");
        assert_eq!(found.path, expected);
        assert_eq!(found.version, Some(21));
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn an_explicitly_configured_path_beats_the_path_search() {
        let os = host_os();
        let on_path = scratch("configured-path");
        fake_binary(&on_path, "clang-21");
        let elsewhere = scratch("configured");
        let configured = fake_binary(&elsewhere, "clang-15");
        let which = fake_which(on_path.clone(), os);
        let lookup = Lookup::new(os, &which).with_configured(Some(&configured));

        let found = find_in(&lookup, &CLANG).expect("the configured binary must be used");
        assert_eq!(found.path, configured, "an explicit path outranks a newer one on PATH");
        assert_eq!(found.origin, Origin::Configured);
        let _ = std::fs::remove_dir_all(&on_path);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    #[test]
    fn a_configured_path_that_does_not_exist_falls_through_and_is_reported() {
        let os = host_os();
        let dir = scratch("stale-config");
        let real = fake_binary(&dir, "clang-21");
        let stale = dir.join("clang-15-that-was-deleted");
        let which = fake_which(dir.clone(), os);
        let lookup = Lookup::new(os, &which).with_configured(Some(&stale));

        let found = find_in(&lookup, &CLANG).expect("the search must continue past a stale path");
        assert_eq!(found.path, real, "a missing override must not shadow a real tool");

        let text = not_found(&CLANG, Some(&stale)).to_string();
        assert!(text.contains("clang-15-that-was-deleted"), "names the bad path: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_locally_built_ldid_is_found_in_the_managed_bin_dir() {
        let os = host_os();
        let managed = scratch("ldid-managed");
        let expected = fake_binary(&managed, "ldid");
        let empty_path = scratch("ldid-empty");
        let which = fake_which(empty_path.clone(), os);
        let lookup = Lookup::new(os, &which).with_managed_bin(Some(managed.clone()));

        let found = find_in(&lookup, &LDID).expect("a locally built ldid must be found");
        assert_eq!(found.path, expected);
        assert_eq!(found.origin, Origin::ManagedBin);
        let _ = std::fs::remove_dir_all(&managed);
        let _ = std::fs::remove_dir_all(&empty_path);
    }

    #[test]
    fn the_managed_bin_dir_is_not_searched_for_other_tools() {
        let os = host_os();
        let managed = scratch("clang-managed");
        fake_binary(&managed, "clang");
        let empty_path = scratch("clang-empty");
        let which = fake_which(empty_path.clone(), os);
        let lookup = hermetic_lookup(&which, os, "clang-managed")
            .with_managed_bin(Some(managed.clone()));

        assert!(
            find_in(&lookup, &CLANG).is_none(),
            "only ldid is built into the managed dir; finding clang there would be a bug"
        );
        let _ = std::fs::remove_dir_all(&managed);
        let _ = std::fs::remove_dir_all(&empty_path);
    }

    #[test]
    fn a_managed_build_outranks_a_system_ldid_on_path() {
        // bootstrap deliberately produced the managed one, so it is the one the
        // user expects to be used.
        let os = host_os();
        let managed = scratch("ldid-priority");
        let built = fake_binary(&managed, "ldid");
        let on_path = scratch("ldid-system");
        fake_binary(&on_path, "ldid");
        let which = fake_which(on_path.clone(), os);
        let lookup = Lookup::new(os, &which).with_managed_bin(Some(managed.clone()));

        let found = find_in(&lookup, &LDID).expect("ldid must be found");
        assert_eq!(found.path, built);
        let _ = std::fs::remove_dir_all(&managed);
        let _ = std::fs::remove_dir_all(&on_path);
    }

    #[test]
    fn a_missing_tool_is_none_and_an_actionable_error() {
        let os = host_os();
        let empty = scratch("nothing");
        let which = fake_which(empty.clone(), os);
        // Hermetic: this asserts "nothing found", so it must not be allowed to
        // find the developer's real LLVM install.
        let lookup = hermetic_lookup(&which, os, "nothing");

        assert!(find_in(&lookup, &CLANG).is_none());
        let error = not_found(&CLANG, None);
        let text = error.to_string();
        assert!(text.contains("clang"), "names the tool: {text}");
        assert!(
            text.contains(&format!("{}", VERSION_SEARCH_WINDOW.1)),
            "discloses the version window, so an out-of-range tool is explainable: {text}"
        );
        assert!(text.contains("DARWINFORGE_CLANG"), "offers the override: {text}");
        assert_eq!(error.exit_code(), 4, "a missing tool is a prereq");
        let _ = std::fs::remove_dir_all(&empty);
    }
    #[test]
    fn the_version_window_is_a_parameter_not_a_version_pin() {
        let os = host_os();
        let dir = scratch("window");
        let twenty_one = fake_binary(&dir, "clang-21");
        let which = fake_which(dir.clone(), os);

        let narrow = hermetic_lookup(&which, os, "window");
        assert!(
            find_in_window(&narrow, &CLANG, 6, 20).is_none(),
            "clang-21 is outside a window ending at 20"
        );
        let standard = hermetic_lookup(&which, os, "window");
        assert_eq!(
            find_in_window(&standard, &CLANG, 6, 64).map(|tool| tool.path),
            Some(twenty_one.clone()),
            "the default window reaches it"
        );
        // A caller that knows about a newer LLVM widens the window and finds it,
        // rather than being told the version is unsupported.
        let widest = hermetic_lookup(&which, os, "window");
        assert_eq!(
            find_in_window(&widest, &CLANG, 6, 200).map(|tool| tool.path),
            Some(twenty_one),
            "a wider window finds a future version"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_prefixes_are_scanned_newest_first() {
        let dir = scratch("prefixes");
        let lib = dir.join("lib");
        for version in ["llvm-18", "llvm-21", "llvm-20", "llvm-notanumber"] {
            std::fs::create_dir_all(lib.join(version).join("bin")).expect("mkdir");
        }
        let older = fake_binary(&lib.join("llvm-18").join("bin"), "clang");
        let newer = fake_binary(&lib.join("llvm-21").join("bin"), "clang");
        fake_binary(&lib.join("llvm-20").join("bin"), "clang");
        fake_binary(&lib.join("llvm-notanumber").join("bin"), "clang");

        let dirs = llvm_bin_dirs(OperatingSystem::Linux, &lib);
        assert_eq!(dirs.len(), 3, "the non-numeric tree is not an LLVM version: {dirs:?}");
        let versions: Vec<&str> = dirs
            .iter()
            .filter_map(|dir| dir.parent()?.file_name()?.to_str())
            .collect();
        assert_eq!(versions, ["llvm-21", "llvm-20", "llvm-18"], "newest prefix first");
        assert!(dirs[0].ends_with("bin"), "and each points at its bin directory");

        let empty = scratch("prefixes-empty");
        let os = host_os();
        let which = fake_which(empty.clone(), os);
        // Hermetic: without neutralising the OS prefixes, a real
        // `C:\Program Files\LLVM\bin` on the developer's machine outranks this
        // fixture and the "newest prefix wins" assertion becomes machine state.
        let lookup = Lookup::new(os, &which)
            .with_lib_dir(lib.clone())
            .with_extra_prefixes(Vec::new());

        let found = find_in(&lookup, &CLANG).expect("an empty PATH must fall through");
        assert_eq!(found.path, newer, "the newest LLVM prefix wins");
        assert_ne!(found.path, older);
        assert_eq!(found.origin, Origin::InstallPrefix);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn a_versioned_binary_in_a_prefix_beats_an_unversioned_one() {
        let os = host_os();
        let dir = scratch("prefix-versioned");
        let lib = dir.join("lib");
        std::fs::create_dir_all(lib.join("llvm-21").join("bin")).expect("mkdir");
        fake_binary(&lib.join("llvm-21").join("bin"), "clang");
        let numbered = fake_binary(&lib.join("llvm-21").join("bin"), "clang-20");
        let empty = scratch("prefix-versioned-empty");
        let which = fake_which(empty.clone(), os);
        let lookup = Lookup::new(os, &which).with_lib_dir(lib);

        let found = find_in(&lookup, &CLANG).expect("the prefix must be searched");
        assert_eq!(found.path, numbered, "20 is newer than an unversioned name");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn each_platform_gets_its_own_install_prefixes() {
        let mac = llvm_bin_dirs(OperatingSystem::MacOs, Path::new("/usr/lib"));
        assert!(mac.iter().any(|dir| dir == Path::new("/opt/homebrew/opt/llvm/bin")));
        assert!(mac.iter().any(|dir| dir == Path::new("/usr/local/bin")));
        assert!(mac.iter().any(|dir| dir == Path::new("/opt/local/bin")));

        let windows = llvm_bin_dirs(OperatingSystem::Windows, Path::new(r"C:\nothing"));
        assert!(windows.iter().any(|dir| dir == Path::new(r"C:\Program Files\LLVM\bin")));

        // A missing lib dir is an empty list, never a panic.
        assert!(llvm_bin_dirs(OperatingSystem::Linux, Path::new("/no/such/lib")).is_empty());
    }
    #[test]
    fn windows_suffixes_are_honoured_when_matching() {
        let dir = scratch("suffixes");
        let exe = dir.join("clang.exe");
        std::fs::write(&exe, b"MZ").expect("write");
        let cmd = dir.join("ldid.cmd");
        std::fs::write(&cmd, b"@echo off").expect("write");
        let dirs = [dir.clone()];

        assert_eq!(
            which_in_dirs(&dirs, "clang", OperatingSystem::Windows),
            Some(exe),
            "clang.exe satisfies clang on Windows"
        );
        assert_eq!(
            which_in_dirs(&dirs, "ldid", OperatingSystem::Windows),
            Some(cmd),
            "a .cmd shim satisfies the tool too"
        );
        assert_eq!(
            which_in_dirs(&dirs, "clang", OperatingSystem::Linux),
            None,
            "on Linux a bare name does not match clang.exe"
        );
        assert_eq!(
            which_in_dirs(&dirs, "clang", OperatingSystem::Windows)
                .and_then(|path| path.extension().map(std::ffi::OsStr::to_os_string)),
            Some(std::ffi::OsString::from("exe")),
            "the suffix is carried on the resolved path"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_versioned_tool_is_found_through_its_windows_suffix() {
        let os = host_os();
        let dir = scratch("win-versioned");
        let expected = dir.join("clang-21.exe");
        std::fs::write(&expected, b"MZ").expect("write");
        let which = fake_which(dir.clone(), os);
        let lookup = Lookup::new(os, &which);

        let found = find_in(&lookup, &CLANG).expect("clang-21.exe must be found");
        assert_eq!(found.path, expected);
        assert_eq!(found.version, Some(21), "the extension does not hide the version");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_version_string_comes_from_the_injected_probe() {
        let probe = |binary: &Path| -> Option<String> {
            if binary.ends_with("clang-21") {
                Some("clang version 21.1.2".to_string())
            } else {
                None
            }
        };
        let dir = scratch("probe");
        let path = fake_binary(&dir, "clang-21");
        let tool = found(path, &CLANG, Origin::PathVersion, Some(21));

        assert_eq!(
            tool.reported_version(&probe).as_deref(),
            Some("clang version 21.1.2"),
            "the probe's output is the reported version"
        );
        let described = tool.describe();
        assert!(described.contains("v21"), "names the version: {described}");
        assert!(described.contains("versioned"), "names the origin: {described}");
        assert_eq!(tool.binary_name(), "clang-21");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_silent_probe_is_unknown_not_an_error() {
        let dir = scratch("probe-silent");
        let path = fake_binary(&dir, "ldid");
        let silent = |_: &Path| Some("   \n".to_string());
        assert_eq!(detect_version(&path, &silent), None, "blank output means unknown");

        let absent = |_: &Path| None;
        assert_eq!(detect_version(&path, &absent), None, "a missing binary means unknown");

        // The production probe must be tolerant too: a file that is not a
        // program at all has to come back as None rather than an error.
        let not_a_program = dir.join("not-a-program");
        std::fs::write(&not_a_program, b"hello").expect("write");
        assert_eq!(probe_version(&not_a_program), None, "a non-program yields no version");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_tool_has_a_spec_a_name_and_an_override() {
        let names: Vec<&str> = all_specs().iter().map(|spec| spec.name).collect();
        for expected in ["clang", "ld64.lld", "ld", "ldid", "git", "zip"] {
            assert!(names.contains(&expected), "{expected} must be findable, got {names:?}");
        }
        for spec in all_specs() {
            assert!(!spec.candidates.is_empty(), "{} needs at least one binary name", spec.name);
            assert!(!spec.fix_hint.is_empty(), "{} needs a fix hint", spec.name);
            assert!(env_name(spec).starts_with("DARWINFORGE_"), "bad override name");
        }
        assert_eq!(env_name(&CLANG), "DARWINFORGE_CLANG");
        assert_eq!(env_name(&LINKER), "DARWINFORGE_LINKER");
        assert_eq!(env_name(&LDID), "DARWINFORGE_LDID");
        assert_eq!(env_name(&ZIP), "DARWINFORGE_ZIP");
        // The required/optional split is a policy decision, so it is asserted
        // against the specs themselves rather than assumed.
        let required: Vec<&str> =
            all_specs().iter().filter(|spec| spec.required).map(|spec| spec.name).collect();
        assert_eq!(required, ["clang", "ld64.lld", "ldid"], "only these three are mandatory");
        let optional: Vec<&str> =
            all_specs().iter().filter(|spec| !spec.required).map(|spec| spec.name).collect();
        assert_eq!(
            optional,
            ["ld", "git", "zip"],
            "zip is optional (the built-in writer exists) and git only fetches an SDK"
        );
        let managed: Vec<&str> =
            all_specs().iter().filter(|spec| spec.search_managed).map(|spec| spec.name).collect();
        assert_eq!(managed, ["ldid"], "ldid is the tool bootstrap builds itself");
    }

    #[test]
    fn versions_are_ordered_by_number_not_by_appearance() {
        let mut tools = [
            found(PathBuf::from("/usr/bin/clang"), &CLANG, Origin::PathName, None),
            found(PathBuf::from("/usr/bin/clang-17"), &CLANG, Origin::PathVersion, Some(17)),
            found(PathBuf::from("/usr/bin/clang-21"), &CLANG, Origin::PathVersion, Some(21)),
        ];
        tools.sort_by_key(FoundTool::precedence);
        let names: Vec<String> = tools.iter().map(FoundTool::binary_name).collect();
        assert_eq!(names, ["clang", "clang-17", "clang-21"]);
    }

    #[test]
    fn origins_break_ties_between_equally_versioned_candidates() {
        let configured = found(PathBuf::from("/opt/clang-21"), &CLANG, Origin::Configured, None);
        let managed = found(PathBuf::from("/data/bin/clang-21"), &CLANG, Origin::ManagedBin, None);
        let path = found(PathBuf::from("/usr/bin/clang-21"), &CLANG, Origin::PathVersion, Some(21));
        assert!(
            configured.precedence() > managed.precedence(),
            "an explicit path outranks the managed dir"
        );
        assert!(managed.precedence() > path.precedence(), "the managed dir outranks PATH");
        assert_eq!(Origin::Configured.label(), "configured path");
    }

    #[test]
    fn the_search_order_is_configured_then_managed_then_path_then_prefixes() {
        let os = host_os();
        let dir = scratch("order");
        let managed = scratch("order-managed");
        let lib = dir.join("lib");
        std::fs::create_dir_all(lib.join("llvm-21").join("bin")).expect("mkdir");
        fake_binary(&lib.join("llvm-21").join("bin"), "clang");
        fake_binary(&dir, "clang-21");
        let config_dir = scratch("order-config");
        let configured = fake_binary(&config_dir, "clang-21");

        let which = fake_which(dir.clone(), os);
        let with_override = Lookup::new(os, &which)
            .with_configured(Some(&configured))
            .with_managed_bin(Some(managed.clone()))
            .with_lib_dir(lib.clone());
        assert_eq!(find_in(&with_override, &CLANG).map(|t| t.origin), Some(Origin::Configured));

        // Drop the override: PATH decides, and it beats the install prefix.
        let which = fake_which(dir.clone(), os);
        let no_override = Lookup::new(os, &which)
            .with_managed_bin(Some(managed.clone()))
            .with_lib_dir(lib.clone());
        assert_eq!(find_in(&no_override, &CLANG).map(|t| t.origin), Some(Origin::PathVersion));

        // Empty PATH: the LLVM prefix is the last resort.
        let empty = scratch("order-empty");
        let which = fake_which(empty.clone(), os);
        let prefix_only = Lookup::new(os, &which).with_lib_dir(lib.clone());
        assert_eq!(
            find_in(&prefix_only, &CLANG).map(|t| t.origin),
            Some(Origin::InstallPrefix)
        );
        for dir in [dir, managed, config_dir, empty] {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_missing_tool_reports_each_step_it_took() {
        let os = host_os();
        let empty = scratch("report-order");
        let which = fake_which(empty.clone(), os);
        let text = not_found(&LDID, None).to_string();
        assert!(text.contains("exact name on PATH"), "step 3 is named: {text}");
        assert!(text.contains("bin dir"), "the managed dir is named for ldid: {text}");
        let lookup = Lookup::new(os, &which);
        assert!(find_in(&lookup, &LDID).is_none());
        assert!(text.contains("ldid"), "names the tool: {text}");
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn version_numbers_run_newest_first_and_tolerate_a_reversed_window() {
        let numbers: Vec<u32> = version_numbers(19, 21).collect();
        assert_eq!(numbers, [21, 20, 19], "descending, so the first hit is the best");
        let reversed: Vec<u32> = version_numbers(21, 19).collect();
        assert_eq!(reversed, [21, 20, 19], "a reversed window is normalised");
        assert_eq!(version_numbers(7, 7).collect::<Vec<u32>>(), [7], "a single version is fine");
    }

    #[test]
    fn trailing_numbers_must_be_entirely_digits() {
        assert_eq!(trailing_number("21"), Some(21));
        assert_eq!(trailing_number("180"), Some(180));
        assert_eq!(trailing_number(""), None);
        assert_eq!(trailing_number("2a"), None);
        assert_eq!(trailing_number("-1"), None);
        assert_eq!(trailing_number("99999999999999999999"), None, "no silent wrap-around");
    }

    #[test]
    fn known_suffixes_are_stripped_before_parsing_a_version() {
        assert_eq!(strip_known_suffix("clang-21.exe"), "clang-21");
        assert_eq!(strip_known_suffix("ldid.bat"), "ldid");
        assert_eq!(strip_known_suffix("clang"), "clang", "no suffix, unchanged");
        assert_eq!(strip_known_suffix("clang++"), "clang++", "'++' is not a suffix");
    }
}


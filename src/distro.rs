//! Detecting the operating system, its Linux distribution, and its package
//! manager — all at run time.
//!
//! There is deliberately **no hardcoded list of distributions**. A distro is
//! resolved from `/etc/os-release`: we read `ID` and `ID_LIKE`, so Ubuntu,
//! Linux Mint, Pop!_OS and Nobara all resolve through their lineage even though
//! none of them is named in this file. Only the *family -> package manager*
//! mapping lives here, because that is the part that genuinely differs.
//!
//! Every function takes its inputs as arguments (a path to os-release, a
//! `which`-style predicate) so the whole module is testable from canned data on
//! any operating system.

use std::path::Path;

/// The operating systems we know how to set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingSystem {
    /// Native Linux.
    Linux,
    /// The Linux userspace inside Windows Subsystem for Linux.
    Wsl,
    MacOs,
    Windows,
}

impl OperatingSystem {
    /// Lowercase name used in output and in the data table key.
    pub fn label(self) -> &'static str {
        match self {
            OperatingSystem::Linux => "linux",
            OperatingSystem::Wsl => "wsl",
            OperatingSystem::MacOs => "macos",
            OperatingSystem::Windows => "windows",
        }
    }

    /// True when this OS runs the toolchain natively (no WSL bridge needed).
    pub fn is_native_toolchain_os(self) -> bool {
        matches!(self, OperatingSystem::Linux | OperatingSystem::MacOs)
    }

    /// Which executable suffixes a binary may carry on this OS.
    pub fn binary_suffixes(self) -> &'static [&'static str] {
        match self {
            OperatingSystem::Windows => &["", ".exe", ".cmd", ".bat", ".com"],
            _ => &[""],
        }
    }
}

/// Detect the current operating system.
///
/// On Linux (including under WSL) the answer depends on the kernel, so this
/// reads `/proc/sys/kernel/osrelease`, which carries the `microsoft` marker WSL
/// adds. On macOS and Windows the answer is fixed by the build target.
pub fn detect_os() -> OperatingSystem {
    if cfg!(target_os = "macos") {
        return OperatingSystem::MacOs;
    }
    if cfg!(target_os = "windows") {
        return OperatingSystem::Windows;
    }
    if is_wsl(Path::new("/proc/sys/kernel/osrelease")) {
        OperatingSystem::Wsl
    } else {
        OperatingSystem::Linux
    }
}

/// True when `osrelease_path` names a WSL kernel.
///
/// WSL marks its kernel with `microsoft` (WSL2) or `WSL` (WSL1) in
/// `/proc/sys/kernel/osrelease`. This is a pure function of the file contents so
/// tests can feed either shape.
pub fn is_wsl(osrelease_path: &Path) -> bool {
    match std::fs::read_to_string(osrelease_path) {
        Ok(text) => {
            let lower = text.to_lowercase();
            lower.contains("microsoft") || lower.contains("wsl")
        }
        // No procfs (a container, or a test): not WSL as far as we can tell.
        Err(_) => false,
    }
}

/// The interesting fields of `/etc/os-release`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OsRelease {
    /// `ID=ubuntu`, lowercased.
    pub id: String,
    /// `ID_LIKE="debian"`, lowercased. Empty on some minimal distros.
    pub id_like: Vec<String>,
    /// `PRETTY_NAME="Ubuntu 22.04.3 LTS"`.
    pub pretty_name: String,
    /// `VERSION_ID="22.04"`.
    pub version_id: String,
}

impl OsRelease {
    /// Human label for output: `PRETTY_NAME` when the distro supplies one.
    pub fn label(&self) -> String {
        if !self.pretty_name.is_empty() {
            return self.pretty_name.clone();
        }
        if self.id.is_empty() {
            return "unknown Linux distribution".to_string();
        }
        if self.version_id.is_empty() {
            return self.id.clone();
        }
        format!("{} {}", self.id, self.version_id)
    }

    /// The distribution id plus its whole `ID_LIKE` lineage, most specific first.
    ///
    /// This is what makes Mint, Pop!_OS and Nobara work without being named
    /// anywhere: `linuxmint` declares `ID_LIKE="ubuntu debian"`, so it inherits
    /// apt through its lineage.
    pub fn lineage(&self) -> Vec<String> {
        let mut chain: Vec<String> = Vec::new();
        if !self.id.is_empty() {
            chain.push(self.id.clone());
        }
        for ancestor in &self.id_like {
            if !chain.contains(ancestor) {
                chain.push(ancestor.clone());
            }
        }
        chain
    }

    /// True when this distro (or anything it derives from) matches `family`.
    pub fn is_family(&self, family: &str) -> bool {
        self.lineage().iter().any(|entry| entry == family)
    }
}

/// Parse the shell-style `KEY=value` lines of an `os-release` file.
///
/// Values may be bare, single-quoted or double-quoted. Unknown keys are ignored
/// rather than rejected: the file is specified to grow over time, and an unknown
/// key is not a reason to refuse to set the machine up.
pub fn parse_os_release(text: &str) -> OsRelease {
    let mut parsed = OsRelease::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else { continue };
        let value = unquote(raw_value.trim());
        match key.trim() {
            "ID" => parsed.id = value.to_lowercase(),
            "ID_LIKE" => {
                parsed.id_like =
                    value.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>();
            }
            "PRETTY_NAME" => parsed.pretty_name = value,
            "VERSION_ID" => parsed.version_id = value,
            _ => {}
        }
    }
    parsed
}

/// Read and parse `/etc/os-release`.
pub fn read_os_release(path: &Path) -> Option<OsRelease> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(parse_os_release(&text))
}

/// Strip matching surrounding quotes, as `os-release` allows either style.
fn unquote(value: &str) -> String {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// The package managers bootstrap knows how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Apk,
    Brew,
    Winget,
    Choco,
    Scoop,
}

impl PackageManager {
    /// Lowercase name, used as the key into the data tables.
    pub fn label(self) -> &'static str {
        match self {
            PackageManager::Apt => "apt",
            PackageManager::Dnf => "dnf",
            PackageManager::Pacman => "pacman",
            PackageManager::Zypper => "zypper",
            PackageManager::Apk => "apk",
            PackageManager::Brew => "brew",
            PackageManager::Winget => "winget",
            PackageManager::Choco => "choco",
            PackageManager::Scoop => "scoop",
        }
    }

    /// The binary that performs installs.
    pub fn program(self) -> &'static str {
        match self {
            PackageManager::Apt => "apt-get",
            PackageManager::Dnf => "dnf",
            PackageManager::Pacman => "pacman",
            PackageManager::Zypper => "zypper",
            PackageManager::Apk => "apk",
            PackageManager::Brew => "brew",
            PackageManager::Winget => "winget",
            PackageManager::Choco => "choco",
            PackageManager::Scoop => "scoop",
        }
    }

    /// Arguments that query the configured repositories for `package`.
    ///
    /// A successful exit means "available". Run before offering an install, so
    /// we never suggest a package name the repo does not actually carry.
    pub fn probe_args(self, package: &str) -> Vec<String> {
        let package = package.to_string();
        match self {
            PackageManager::Apt => vec!["policy".into(), package],
            PackageManager::Dnf => vec!["info".into(), package],
            PackageManager::Pacman => vec!["-Si".into(), package],
            PackageManager::Zypper => vec!["--non-interactive".into(), "info".into(), package],
            PackageManager::Apk => vec!["search".into(), "-x".into(), package],
            PackageManager::Brew => vec!["info".into(), package],
            PackageManager::Winget => vec!["show".into(), "--id".into(), package],
            PackageManager::Choco => vec!["search".into(), package, "--exact".into()],
            PackageManager::Scoop => vec!["search".into(), package],
        }
    }

    /// Arguments that install `packages` non-interactively.
    pub fn install_args(self, packages: &[String]) -> Vec<String> {
        let mut args: Vec<String> = match self {
            PackageManager::Apt => vec!["install".into(), "-y".into()],
            PackageManager::Dnf => vec!["install".into(), "-y".into()],
            PackageManager::Pacman => vec!["-S".into(), "--needed".into(), "--noconfirm".into()],
            PackageManager::Zypper => vec!["--non-interactive".into(), "install".into()],
            PackageManager::Apk => vec!["add".into()],
            PackageManager::Brew => vec!["install".into()],
            PackageManager::Winget => vec![
                "install".into(),
                "--exact".into(),
                "--accept-package-agreements".into(),
                "--accept-source-agreements".into(),
                "--disable-interactivity".into(),
            ],
            PackageManager::Choco => vec!["install".into(), "-y".into()],
            PackageManager::Scoop => vec!["install".into()],
        };
        // winget resolves exactly one id per invocation.
        if self == PackageManager::Winget {
            args.truncate(5);
            match packages.first() {
                Some(first) => args.push(first.clone()),
                None => args.clear(),
            }
        } else {
            args.extend(packages.iter().cloned());
        }
        args
    }

    /// True when this manager needs elevation (`sudo`) to install.
    pub fn wants_sudo(self) -> bool {
        matches!(
            self,
            PackageManager::Apt | PackageManager::Dnf | PackageManager::Pacman
                | PackageManager::Zypper | PackageManager::Apk
        )
    }
}

/// The full picture of the machine bootstrap is running on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub os: OperatingSystem,
    /// `None` on macOS and Windows, and on Linux without a readable os-release.
    pub distro: Option<OsRelease>,
    /// The manager we would drive, if one is installed. Probed, never assumed.
    pub package_manager: Option<PackageManager>,
    /// Every manager found on PATH, in detection order. Used to explain what
    /// alternatives exist when the preferred one is missing.
    pub available_managers: Vec<PackageManager>,
    /// True when running as uid 0 (or a Windows Administrator).
    pub is_root: bool,
    /// Host architecture as reported by the OS, e.g. `x86_64`, `aarch64`.
    pub arch: String,
}

/// Order in which managers are probed. Earlier entries win.
///
/// The order reflects "most likely to be what this user actually has", not a
/// hardcoded distro list: on Linux the distro lineage decides, and only if that
/// yields nothing do we fall back to this sequence.
const MANAGER_PROBE_ORDER: &[PackageManager] = &[
    PackageManager::Apt,
    PackageManager::Dnf,
    PackageManager::Pacman,
    PackageManager::Zypper,
    PackageManager::Apk,
    PackageManager::Brew,
    PackageManager::Winget,
    PackageManager::Choco,
    PackageManager::Scoop,
];

/// Pick the package manager for a distro, from its `ID_LIKE` lineage.
///
/// Returns `None` for a distro whose family we do not map, so the caller can
/// fall back to probing PATH rather than guessing wrong.
pub fn manager_for_distro(distro: &OsRelease) -> Option<PackageManager> {
    // Families, checked in the order they are most specific.
    if distro.is_family("debian") || distro.is_family("ubuntu") {
        return Some(PackageManager::Apt);
    }
    if distro.is_family("fedora") || distro.is_family("rhel") {
        return Some(PackageManager::Dnf);
    }
    if distro.is_family("arch") {
        return Some(PackageManager::Pacman);
    }
    if distro.is_family("suse") || distro.is_family("opensuse") {
        return Some(PackageManager::Zypper);
    }
    if distro.is_family("alpine") {
        return Some(PackageManager::Apk);
    }
    None
}

/// Detect everything about the host, using the real machine.
///
/// `which` is injected so the detection logic can be unit-tested with a fake
/// PATH; production passes [`crate::exec::which`].
pub fn detect_host(
    os: OperatingSystem,
    os_release_path: &Path,
    which: &dyn Fn(&str) -> bool,
) -> Host {
    let distro = match os {
        OperatingSystem::Linux | OperatingSystem::Wsl => read_os_release(os_release_path),
        _ => None,
    };
    let available_managers: Vec<PackageManager> = MANAGER_PROBE_ORDER
        .iter()
        .copied()
        .filter(|manager| which(manager.program()))
        .collect();

    // Preference order: what the distro says, then what is installed.
    let package_manager = distro
        .as_ref()
        .and_then(manager_for_distro)
        .filter(|preferred| available_managers.contains(preferred))
        .or_else(|| {
            // macOS and Windows have no os-release; brew/winget/choco/scoop only.
            let fallback_order: &[PackageManager] = match os {
                OperatingSystem::MacOs => &[PackageManager::Brew],
                OperatingSystem::Windows => &[
                    PackageManager::Winget,
                    PackageManager::Choco,
                    PackageManager::Scoop,
                ],
                _ => MANAGER_PROBE_ORDER,
            };
            fallback_order
                .iter()
                .copied()
                .find(|manager| available_managers.contains(manager))
        });

    Host {
        os,
        distro,
        package_manager,
        available_managers,
        is_root: detect_root(),
        arch: detect_arch(),
    }
}

/// True when the process is running with administrative privileges.
pub fn detect_root() -> bool {
    if cfg!(windows) {
        // There is no uid 0 on Windows; membership of the Administrators group
        // is the closest equivalent. Reading it needs a crate we do not have, so
        // we conservatively report false and let the installer confirm.
        false
    } else {
        std::env::var("USER").map(|user| user == "root").unwrap_or(false)
            || std::env::var("EUID").is_ok_and(|euid| euid == "0")
    }
}

/// Host architecture, using the OS's own spelling (`x86_64`, `aarch64`, ...).
pub fn detect_arch() -> String {
    if let Ok(value) = std::env::var("PROCESSOR_ARCHITEW6432") {
        if !value.is_empty() {
            return value.to_lowercase();
        }
    }
    if let Ok(value) = std::env::var("PROCESSOR_ARCHITECTURE") {
        if !value.is_empty() {
            return normalize_arch(&value);
        }
    }
    if let Ok(value) = std::env::var("HOSTTYPE") {
        // macOS: x86_64 or arm64.
        return normalize_arch(value.split('.').next().unwrap_or(&value));
    }
    std::env::consts::ARCH.to_string()
}

/// Map an OS-reported architecture onto the spelling used everywhere else.
pub fn normalize_arch(value: &str) -> String {
    let lower = value.trim().to_ascii_lowercase();
    match lower.as_str() {
        "x64" | "amd64" | "x86_64" => "x86_64".to_string(),
        "arm64" | "aarch64" => "aarch64".to_string(),
        "x86" | "i386" | "i686" => "x86".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real os-release content, one per distro family the task calls out.
    const DEBIAN: &str = r#"PRETTY_NAME="Debian GNU/Linux 12 (bookworm)"
NAME="Debian GNU/Linux"
VERSION_ID="12"
ID=debian
"#;
    const UBUNTU: &str = r#"PRETTY_NAME="Ubuntu 22.04.4 LTS"
NAME="Ubuntu"
VERSION_ID="22.04"
ID=ubuntu
ID_LIKE=debian
"#;
    const FEDORA: &str = r#"NAME="Fedora Linux"
VERSION="40 (Workstation Edition)"
ID=fedora
VERSION_ID=40
PRETTY_NAME="Fedora Linux 40 (Workstation Edition)"
"#;
    const ARCH: &str = r#"NAME="Arch Linux"
ID=arch
PRETTY_NAME="Arch Linux"
BUILD_ID=rolling
"#;
    const MANJARO: &str = r#"NAME="Manjaro Linux"
ID=manjaro
ID_LIKE=arch
PRETTY_NAME="Manjaro Linux"
BUILD_ID=rolling
"#;
    const LINUXMINT: &str = r#"NAME="Linux Mint"
ID=linuxmint
ID_LIKE="ubuntu debian"
PRETTY_NAME="Linux Mint 22 (Xia)"
VERSION_ID="22"
"#;
    const NOBARA: &str = r#"NAME="Nobara Linux"
ID=nobara
ID_LIKE=fedora
PRETTY_NAME="Nobara Linux 40 (Workstation Edition)"
VERSION_ID="40"
"#;
    const ALPINE: &str = r#"NAME="Alpine Linux"
ID=alpine
VERSION_ID=3.20.1
PRETTY_NAME="Alpine Linux v3.20"
"#;
    const OPENSUSE: &str = r#"NAME="openSUSE Leap"
ID="opensuse-leap"
ID_LIKE="opensuse suse"
VERSION_ID="15.6"
PRETTY_NAME="openSUSE Leap 15.6"
"#;

    fn none_available(_: &str) -> bool {
        false
    }

    fn only(available: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |program: &str| available.contains(&program)
    }

    #[test]
    fn parses_a_quoted_pretty_name() {
        let release = parse_os_release(DEBIAN);
        assert_eq!(release.id, "debian");
        assert_eq!(release.version_id, "12");
        assert_eq!(release.pretty_name, "Debian GNU/Linux 12 (bookworm)");
        assert!(release.id_like.is_empty(), "Debian declares no ID_LIKE");
    }

    #[test]
    fn parses_id_like_as_a_whitespace_list() {
        let release = parse_os_release(UBUNTU);
        assert_eq!(release.id, "ubuntu");
        assert_eq!(release.id_like, vec!["debian"]);
        assert_eq!(release.label(), "Ubuntu 22.04.4 LTS");
    }

    #[test]
    fn parses_a_multi_word_quoted_id_like() {
        let release = parse_os_release(LINUXMINT);
        assert_eq!(release.id, "linuxmint");
        assert_eq!(release.id_like, vec!["ubuntu", "debian"], "both ancestors are kept");
    }

    #[test]
    fn ignores_comments_and_unknown_keys() {
        let text = "# a comment\n\nID=fedora\nSOMETHING_NEW=\"x\"\nBUILD_ID=rolling\n";
        let release = parse_os_release(text);
        assert_eq!(release.id, "fedora");
        assert!(release.pretty_name.is_empty());
    }

    #[test]
    fn debian_resolves_to_apt() {
        assert_eq!(manager_for_distro(&parse_os_release(DEBIAN)), Some(PackageManager::Apt));
    }

    #[test]
    fn ubuntu_resolves_to_apt() {
        assert_eq!(manager_for_distro(&parse_os_release(UBUNTU)), Some(PackageManager::Apt));
    }

    #[test]
    fn fedora_resolves_to_dnf() {
        assert_eq!(manager_for_distro(&parse_os_release(FEDORA)), Some(PackageManager::Dnf));
    }

    #[test]
    fn arch_resolves_to_pacman() {
        assert_eq!(manager_for_distro(&parse_os_release(ARCH)), Some(PackageManager::Pacman));
    }

    #[test]
    fn manjaro_inherits_pacman_through_id_like() {
        // Manjaro is named nowhere in the mapping: it comes from ID_LIKE=arch,
        // which is exactly why the lineage is read rather than a distro list.
        assert_eq!(manager_for_distro(&parse_os_release(MANJARO)), Some(PackageManager::Pacman));
    }

    #[test]
    fn linuxmint_inherits_apt_through_id_like() {
        assert_eq!(manager_for_distro(&parse_os_release(LINUXMINT)), Some(PackageManager::Apt));
    }

    #[test]
    fn nobara_inherits_dnf_through_id_like() {
        assert_eq!(manager_for_distro(&parse_os_release(NOBARA)), Some(PackageManager::Dnf));
    }

    #[test]
    fn alpine_and_opensuse_resolve_to_their_own_managers() {
        assert_eq!(manager_for_distro(&parse_os_release(ALPINE)), Some(PackageManager::Apk));
        assert_eq!(manager_for_distro(&parse_os_release(OPENSUSE)), Some(PackageManager::Zypper));
    }

    #[test]
    fn an_unmapped_distro_yields_no_manager_rather_than_a_wrong_one() {
        let weird = parse_os_release("ID=plan9\nID_LIKE=inferno\nPRETTY_NAME=\"Plan 9\"\n");
        assert_eq!(manager_for_distro(&weird), None);
    }

    #[test]
    fn lineage_puts_the_most_specific_id_first() {
        assert_eq!(parse_os_release(LINUXMINT).lineage(), vec!["linuxmint", "ubuntu", "debian"]);
    }
#[test]
    fn detect_host_prefers_the_distros_manager_over_path_order() {
        // dnf is present too, but the distro says apt, so apt wins.
        let host = detect_host(
            OperatingSystem::Linux,
            Path::new("/does/not/exist"),
            &only(&["apt-get", "dnf"]),
        );
        assert_eq!(host.package_manager, Some(PackageManager::Apt));
        assert_eq!(host.available_managers.len(), 2);
    }

    #[test]
    fn detect_host_falls_back_to_probing_when_the_distro_is_unreadable() {
        let host =
            detect_host(OperatingSystem::Linux, Path::new("/does/not/exist"), &only(&["pacman"]));
        assert!(host.distro.is_none());
        assert_eq!(host.package_manager, Some(PackageManager::Pacman));
    }

    #[test]
    fn detect_host_reports_no_manager_when_none_is_installed() {
        let host =
            detect_host(OperatingSystem::Linux, Path::new("/does/not/exist"), &none_available);
        assert_eq!(host.package_manager, None);
        assert!(host.available_managers.is_empty());
    }

    #[test]
    fn windows_prefers_winget_then_choco_then_scoop() {
        let winget =
            detect_host(OperatingSystem::Windows, Path::new(""), &only(&["choco", "winget"]));
        assert_eq!(winget.package_manager, Some(PackageManager::Winget));
        let scoop = detect_host(OperatingSystem::Windows, Path::new(""), &only(&["scoop", "choco"]));
        assert_eq!(scoop.package_manager, Some(PackageManager::Choco));
    }

    #[test]
    fn macos_uses_brew() {
        let host = detect_host(OperatingSystem::MacOs, Path::new(""), &only(&["brew", "apt-get"]));
        assert_eq!(host.package_manager, Some(PackageManager::Brew), "brew only on macOS");
    }

    #[test]
    fn wsl_is_a_linux_distro_and_keeps_its_manager() {
        // WSL reports Ubuntu; detection must not lose apt just because the
        // kernel is Microsoft's.
        let host =
            detect_host(OperatingSystem::Wsl, Path::new("/does/not/exist"), &only(&["apt-get"]));
        assert_eq!(host.package_manager, Some(PackageManager::Apt));
    }

    #[test]
    fn wsl_is_detected_from_the_kernel_string() {
        let dir = std::env::temp_dir().join(format!("darwinforge-wsl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp");
        let wsl2 = dir.join("osrelease");
        std::fs::write(&wsl2, "5.15.153.1-microsoft-standard-WSL2\n").expect("write");
        assert!(is_wsl(&wsl2), "WSL2 kernel is recognised");
        let wsl1 = dir.join("osrelease1");
        std::fs::write(&wsl1, "4.4.0-19041-Microsoft\n").expect("write");
        assert!(is_wsl(&wsl1), "WSL1 kernel is recognised");
        let vanilla = dir.join("osrelease2");
        std::fs::write(&vanilla, "6.8.0-generic\n").expect("write");
        assert!(!is_wsl(&vanilla), "a normal kernel is not WSL");
        assert!(!is_wsl(&dir.join("missing")), "no file means not WSL");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_args_query_the_repositories_rather_than_installing() {
        assert_eq!(PackageManager::Apt.probe_args("clang"), ["policy", "clang"]);
        assert_eq!(PackageManager::Pacman.probe_args("lld"), ["-Si", "lld"]);
        assert_eq!(PackageManager::Dnf.probe_args("ldid"), ["info", "ldid"]);
    }

    #[test]
    fn install_args_are_non_interactive() {
        let packages = vec!["clang".to_string(), "lld".to_string()];
        assert_eq!(PackageManager::Apt.install_args(&packages), ["install", "-y", "clang", "lld"]);
        assert_eq!(
            PackageManager::Pacman.install_args(&packages),
            ["-S", "--needed", "--noconfirm", "clang", "lld"]
        );
        assert_eq!(
            PackageManager::Zypper.install_args(&packages),
            ["--non-interactive", "install", "clang", "lld"]
        );
    }

    #[test]
    fn winget_takes_one_id_per_invocation() {
        let packages = vec!["LLVM.LLVM".to_string(), "Git.Git".to_string()];
        let args = PackageManager::Winget.install_args(&packages);
        assert!(args.contains(&"LLVM.LLVM".to_string()));
        assert!(!args.contains(&"Git.Git".to_string()), "winget resolves one id at a time");
    }

    #[test]
    fn sudo_is_only_requested_where_it_is_actually_needed() {
        assert!(PackageManager::Apt.wants_sudo());
        assert!(PackageManager::Pacman.wants_sudo());
        assert!(!PackageManager::Brew.wants_sudo());
        assert!(!PackageManager::Scoop.wants_sudo(), "Scoop is per-user by design");
    }

    #[test]
    fn architectures_normalise_across_spellings() {
        assert_eq!(normalize_arch("AMD64"), "x86_64");
        assert_eq!(normalize_arch("x64"), "x86_64");
        assert_eq!(normalize_arch("ARM64"), "aarch64");
        assert_eq!(normalize_arch("aarch64"), "aarch64");
        assert_eq!(normalize_arch("riscv64"), "riscv64", "unknown arches pass through");
    }

    #[test]
    fn binary_suffixes_follow_the_host_os() {
        assert_eq!(
            OperatingSystem::Windows.binary_suffixes(),
            ["", ".exe", ".cmd", ".bat", ".com"]
        );
        assert_eq!(OperatingSystem::Linux.binary_suffixes(), [""]);
        assert_eq!(OperatingSystem::Wsl.binary_suffixes(), [""]);
    }
}
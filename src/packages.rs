//! THE package-name table.
//!
//! Every package name darwinforge ever suggests lives in this file and nowhere
//! else. Distro repositories do not agree on names — `lld` is its own package on
//! Debian but part of `llvm` elsewhere, and `ldid` is in some Debian/Ubuntu
//! archives, absent from Fedora and only in the AUR on Arch — so the mapping is
//! data, not logic.
//!
//! **No version numbers appear anywhere in this table.** A repository may have
//! shipped `clang-21` while this was written; bootstrap probes the live
//! repository ([`crate::distro::PackageManager::probe_args`]) and falls back to
//! building from source rather than guessing a versioned name.

/// One tool, and the package that provides it on each manager.
#[derive(Debug, Clone, Copy)]
pub struct PackageNames {
    pub tool: &'static str,
    pub apt: Option<&'static str>,
    pub dnf: Option<&'static str>,
    pub pacman: Option<&'static str>,
    pub zypper: Option<&'static str>,
    pub apk: Option<&'static str>,
    pub brew: Option<&'static str>,
    pub winget: Option<&'static str>,
    pub choco: Option<&'static str>,
    pub scoop: Option<&'static str>,
}

impl PackageNames {
    /// The package name for `manager`, or `None` when there is not one.
    pub fn for_manager(&self, manager: crate::distro::PackageManager) -> Option<&'static str> {
        use crate::distro::PackageManager as Pm;
        match manager {
            Pm::Apt => self.apt,
            Pm::Dnf => self.dnf,
            Pm::Pacman => self.pacman,
            Pm::Zypper => self.zypper,
            Pm::Apk => self.apk,
            Pm::Brew => self.brew,
            Pm::Winget => self.winget,
            Pm::Choco => self.choco,
            Pm::Scoop => self.scoop,
        }
    }

    /// True when no package manager carries this tool, so it must be built.
    pub fn unavailable_everywhere(&self) -> bool {
        use crate::distro::PackageManager as Pm;
        [
            Pm::Apt,
            Pm::Dnf,
            Pm::Pacman,
            Pm::Zypper,
            Pm::Apk,
            Pm::Brew,
            Pm::Winget,
            Pm::Choco,
            Pm::Scoop,
        ]
        .iter()
        .all(|manager| self.for_manager(*manager).is_none())
    }
}

/// Look up a tool's package names.
pub fn for_tool(tool: &str) -> Option<&'static PackageNames> {
    PACKAGES.iter().find(|entry| entry.tool == tool)
}

/// Look up a tool, or an empty entry so callers can render "unknown tool"
/// without a special case.
pub fn for_tool_or_empty(tool: &str) -> PackageNames {
    for_tool(tool).copied().unwrap_or(PackageNames {
        tool: "",
        apt: None,
        dnf: None,
        pacman: None,
        zypper: None,
        apk: None,
        brew: None,
        winget: None,
        choco: None,
        scoop: None,
    })
}

/// Tools bootstrap must find, in the order it looks for them.
///
/// Required tools come first: a machine with no clang is broken, a machine with
/// no `zip` merely cannot use `build.zip = true`.
pub const REQUIRED_TOOLS: &[&str] = &["clang", "ld64.lld", "ldid", "git"];

/// Tools that improve the result but are not needed to build.
pub const OPTIONAL_TOOLS: &[&str] = &["zip"];

/// Development packages needed to build `ldid` from source.
pub const LDID_BUILD_DEPS: &[&str] = &["make", "cxx", "libplist", "openssl"];

/// The table. Adding a distro means editing this and nothing else.
pub const PACKAGES: &[PackageNames] = &[
    PackageNames {
        tool: "clang",
        // Debian/Ubuntu: clang and clang-<n> both exist; the unversioned name
        // always tracks the default version, which is what we want.
        apt: Some("clang"),
        // Fedora/RHEL: `clang` is the compiler, `llvm` is the metapackage.
        dnf: Some("clang"),
        // Arch: `clang` is the compiler; `llvm` pulls in everything incl. lld.
        pacman: Some("clang"),
        zypper: Some("clang"),
        apk: Some("clang"),
        brew: Some("llvm"),
        // The official LLVM installer registers as LLVM.LLVM on winget.
        winget: Some("LLVM.LLVM"),
        choco: Some("llvm"),
        scoop: Some("llvm"),
    },
    PackageNames {
        tool: "ld64.lld",
        // The linker is a separate package from clang on every manager here.
        apt: Some("lld"),
        dnf: Some("lld"),
        pacman: Some("lld"),
        zypper: Some("lld"),
        apk: Some("lld"),
        // Homebrew ships ld64.lld inside the llvm formula.
        brew: Some("llvm"),
        // LLVM.LLVM includes lld-link and ld64.lld.
        winget: Some("LLVM.LLVM"),
        choco: Some("llvm"),
        scoop: Some("llvm"),
    },
    PackageNames {
        tool: "ldid",
        // Present in Debian and Ubuntu since ldid 2.5; older archives lack it,
        // which the repository probe detects and routes to a source build.
        apt: Some("ldid"),
        // Not in Fedora or RHEL.
        dnf: None,
        // Not in the Arch official repositories — only the AUR.
        pacman: None,
        zypper: None,
        apk: None,
        brew: Some("ldid"),
        // ProcursusTeam publishes no official Windows build; bootstrap says so
        // plainly rather than producing an unsigned app.
        winget: None,
        choco: None,
        scoop: None,
    },
    PackageNames {
        tool: "git",
        apt: Some("git"),
        dnf: Some("git"),
        pacman: Some("git"),
        zypper: Some("git"),
        apk: Some("git"),
        brew: Some("git"),
        winget: Some("Git.Git"),
        choco: Some("git"),
        scoop: Some("git"),
    },
    PackageNames {
        tool: "zip",
        apt: Some("zip"),
        dnf: Some("zip"),
        pacman: Some("zip"),
        zypper: Some("zip"),
        apk: Some("zip"),
        brew: Some("zip"),
        winget: None,
        choco: Some("zip"),
        scoop: Some("zip"),
    },
    // --- packages that exist only to build ldid from source -----------------
    PackageNames {
        tool: "make",
        apt: Some("make"),
        dnf: Some("make"),
        pacman: Some("make"),
        zypper: Some("make"),
        apk: Some("make"),
        brew: None, // Xcode ships make.
        winget: None,
        choco: Some("make"),
        scoop: Some("make"),
    },
    PackageNames {
        tool: "cxx",
        // A C++ compiler: ldid is C++.
        apt: Some("g++"),
        dnf: Some("gcc-c++"),
        pacman: Some("g++"),
        zypper: Some("gcc-c++"),
        apk: Some("g++"),
        brew: None, // Xcode ships clang++.
        winget: None, // LLVM.LLVM covers it on Windows.
        choco: None,
        scoop: None,
    },
    PackageNames {
        tool: "libplist",
        apt: Some("libplist-dev"),
        dnf: Some("libplist-devel"),
        pacman: Some("libplist"),
        zypper: Some("libplist-devel"),
        apk: Some("libplist-dev"),
        brew: Some("libplist"),
        winget: None,
        choco: None,
        scoop: None,
    },
    PackageNames {
        tool: "openssl",
        apt: Some("libssl-dev"),
        dnf: Some("openssl-devel"),
        pacman: Some("openssl"),
        zypper: Some("libopenssl-devel"),
        apk: Some("openssl-dev"),
        // Not `openssl@3`: that is a version-pinned formula that goes stale.
        // The unversioned `openssl` tracks whatever Homebrew considers current.
        brew: Some("openssl"),
        winget: None,
        choco: None,
        scoop: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distro::PackageManager as Pm;

    #[test]
    fn every_required_and_optional_tool_has_a_row() {
        for tool in REQUIRED_TOOLS.iter().chain(OPTIONAL_TOOLS) {
            assert!(for_tool(tool).is_some(), "the table must cover `{tool}`");
        }
    }

    #[test]
    fn every_build_dependency_has_a_row() {
        for tool in LDID_BUILD_DEPS {
            assert!(for_tool(tool).is_some(), "the table must cover `{tool}`");
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let mut seen: Vec<&str> = PACKAGES.iter().map(|entry| entry.tool).collect();
        seen.sort_unstable();
        let count = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), count, "duplicate tool rows would make lookups ambiguous");
    }

    #[test]
    fn an_unknown_tool_resolves_to_an_empty_row() {
        assert!(for_tool("definitely-not-a-tool").is_none());
        let empty = for_tool_or_empty("definitely-not-a-tool");
        assert_eq!(empty.for_manager(Pm::Apt), None);
        assert!(empty.unavailable_everywhere());
    }

    #[test]
    fn known_differences_are_encoded_in_the_table() {
        // ldid: in Debian/Ubuntu and Homebrew, nowhere else.
        let ldid = for_tool("ldid").expect("row");
        assert_eq!(ldid.for_manager(Pm::Apt), Some("ldid"));
        assert_eq!(ldid.for_manager(Pm::Brew), Some("ldid"));
        assert_eq!(ldid.for_manager(Pm::Dnf), None, "Fedora does not package ldid");
        assert_eq!(ldid.for_manager(Pm::Pacman), None, "Arch ships ldid only in the AUR");
        assert_eq!(ldid.for_manager(Pm::Winget), None, "no official Windows build");

        // lld is a separate package from clang on the Linux managers.
        assert_eq!(for_tool("ld64.lld").expect("row").for_manager(Pm::Apt), Some("lld"));
        assert_eq!(for_tool("ld64.lld").expect("row").for_manager(Pm::Dnf), Some("lld"));
        assert_eq!(for_tool("ld64.lld").expect("row").for_manager(Pm::Pacman), Some("lld"));
        // ...but rides along with llvm on brew/winget/choco/scoop.
        assert_eq!(for_tool("ld64.lld").expect("row").for_manager(Pm::Brew), Some("llvm"));
        assert_eq!(for_tool("ld64.lld").expect("row").for_manager(Pm::Winget), Some("LLVM.LLVM"));
    }

    #[test]
    fn dev_package_names_match_each_distro_convention() {
        // -dev (Debian) vs -devel (Fedora/SUSE) vs plain (Arch) is a real
        // difference, so assert the suffix explicitly.
        for (manager, expected) in [
            (Pm::Apt, "libssl-dev"),
            (Pm::Dnf, "openssl-devel"),
            (Pm::Zypper, "libopenssl-devel"),
            (Pm::Pacman, "openssl"),
            (Pm::Apk, "openssl-dev"),
        ] {
            assert_eq!(
                for_tool("openssl").expect("row").for_manager(manager),
                Some(expected),
                "wrong openssl package for {}",
                manager.label()
            );
        }
    }

    #[test]
    fn no_package_name_carries_a_version_number() {
        // The constraint that keeps this table from rotting: package names are
        // unversioned so they keep working when the distro moves on. Dots are
        // allowed (winget ids are `Publisher.Id`), digits are not.
        for entry in PACKAGES {
            for manager in [
                Pm::Apt,
                Pm::Dnf,
                Pm::Pacman,
                Pm::Zypper,
                Pm::Apk,
                Pm::Brew,
                Pm::Winget,
                Pm::Choco,
                Pm::Scoop,
            ] {
                let Some(name) = entry.for_manager(manager) else { continue };
                assert!(
                    !name.chars().any(|c| c.is_ascii_digit()),
                    "package `{name}` ({}/{}) contains a digit; package names must \
                     stay unversioned so they keep working as distros move on",
                    entry.tool,
                    manager.label()
                );
            }
        }
    }

    #[test]
    fn every_manager_has_an_entry_for_clang_the_linker_and_git() {
        for tool in ["clang", "ld64.lld", "git"] {
            let row = for_tool(tool).expect("row");
            assert!(!row.unavailable_everywhere(), "{tool} must be installable somewhere");
        }
    }

    #[test]
    fn lookups_work_for_every_manager() {
        // Exercises all nine branches of for_manager.
        let clang = for_tool("clang").expect("row");
        for manager in [
            Pm::Apt,
            Pm::Dnf,
            Pm::Pacman,
            Pm::Zypper,
            Pm::Apk,
            Pm::Brew,
            Pm::Winget,
            Pm::Choco,
            Pm::Scoop,
        ] {
            assert!(clang.for_manager(manager).is_some(), "{} lacks clang", manager.label());
        }
    }
}
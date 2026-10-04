//! The user-supplied iPhoneOS SDK and the external toolchain binaries.
//!
//! darwinforge never downloads an SDK: `--sdk PATH` must point at an
//! iPhoneOS*.sdk the user already has, and this module validates the layout
//! before any compiler is invoked.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::exec;

/// A validated iPhoneOS SDK.
#[derive(Debug, Clone)]
pub struct Sdk {
    pub root: PathBuf,
    /// From `SDKSettings.plist`, when present and readable.
    pub version: Option<String>,
}

/// Paths inside an SDK that the pipeline relies on.
const REQUIRED_SUBDIRS: &[&str] = &["usr/include", "usr/lib", "System/Library/Frameworks"];

impl Sdk {
    /// Validate the SDK layout, returning an actionable error when it is wrong.
    pub fn open(root: &Path) -> Result<Sdk> {
        if !root.exists() {
            return Err(Error::Prereq {
                what: format!("SDK path {} does not exist", root.display()),
                fix: "pass --sdk /path/to/iPhoneOS17.4.sdk, or set DARWINFORGE_SDK. \
                      darwinforge never downloads an SDK for you"
                    .to_string(),
            });
        }
        if !root.is_dir() {
            return Err(Error::Prereq {
                what: format!("SDK path {} is not a directory", root.display()),
                fix: "point --sdk at the unpacked .sdk directory itself"
                    .to_string(),
            });
        }
        let missing: Vec<&str> = REQUIRED_SUBDIRS
            .iter()
            .copied()
            .filter(|subdir| !root.join(subdir).is_dir())
            .collect();
        if !missing.is_empty() {
            return Err(Error::Prereq {
                what: format!(
                    "{} does not look like an iPhoneOS SDK: missing {}",
                    root.display(),
                    missing.join(", ")
                ),
                fix: "check that you unpacked a *device* SDK (iPhoneOS*.sdk). \
                      iPhoneSimulator SDKs have no `System/Library/Frameworks` layout \
                      for arm64 device builds"
                    .to_string(),
            });
        }
        let version = crate::sdkversion::read_sdk_version(root);
        Ok(Sdk { root: root.to_path_buf(), version })
    }

    /// Where `-framework X` is looked up.
    pub fn framework_search_path(&self) -> PathBuf {
        self.root.join("System/Library/Frameworks")
    }

    /// Where `-lfoo` (e.g. `libobjc.tbd`) is looked up.
    pub fn library_search_path(&self) -> PathBuf {
        self.root.join("usr/lib")
    }

    pub fn has_framework(&self, name: &str) -> bool {
        self.root
            .join("System/Library/Frameworks")
            .join(format!("{name}.framework"))
            .is_dir()
    }
}

/// The external binaries darwinforge drives.
#[derive(Debug, Clone)]
pub struct Toolchain {
    pub clang: PathBuf,
    pub linker: PathBuf,
    /// The name we invoke the linker by (`ld64.lld`, `ld`, ...), for messages.
    pub linker_kind: LinkerKind,
    pub ldid: PathBuf,
    pub zip: Option<PathBuf>,
    pub swiftc: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkerKind {
    /// LLVM's Mach-O linker.
    Lld,
    /// cctools-port's `ld`.
    Cctools,
}

impl LinkerKind {
    pub fn binary_name(&self) -> &'static str {
        match self {
            LinkerKind::Lld => "ld64.lld",
            LinkerKind::Cctools => "ld",
        }
    }
}

impl Toolchain {
    /// Locate clang, a linker and ldid.
    ///
    /// `DARWINFORGE_LINKER` may be set to an absolute path; otherwise `ld64.lld`
    /// is preferred over cctools `ld`.
    pub fn discover() -> Result<Toolchain> {
        let clang = exec::resolve("CLANG", "clang").ok_or_else(|| Error::Prereq {
            what: "no `clang` found on PATH".to_string(),
            fix: "install LLVM (e.g. `apt install clang lld` or `dnf install clang lld`), \
                  or set DARWINFORGE_CLANG=/path/to/clang. Run `darwinforge doctor` for a full report"
                .to_string(),
        })?;
        let (linker, linker_kind) = match exec::resolve("LINKER", "ld64.lld") {
            Some(path) => (path, LinkerKind::Lld),
            None => match exec::which("ld") {
                Some(path) => (path, LinkerKind::Cctools),
                None => {
                    return Err(Error::Prereq {
                        what: "no Mach-O linker found (looked for `ld64.lld`, then `ld`)"
                            .to_string(),
                        fix: "install lld (`apt install lld`) for ld64.lld, or \
                              cctools-port for Linux; or set DARWINFORGE_LINKER=/path/to/ld64.lld"
                            .to_string(),
                    })
                }
            },
        };
        let ldid = exec::resolve("LDID", "ldid").ok_or_else(|| Error::Prereq {
            what: "no `ldid` found on PATH".to_string(),
            fix: "install ldid, or set DARWINFORGE_LDID=/path/to/ldid. ldid provides the \
                  fake (ad-hoc) signature this PoC uses"
                .to_string(),
        })?;
        Ok(Toolchain {
            clang,
            linker,
            linker_kind,
            ldid,
            zip: exec::which("zip"),
            swiftc: exec::resolve("SWIFTC", "swiftc"),
        })
    }
}
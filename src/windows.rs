//! Native-Windows support: symlinks, the WSL bridge, and what bootstrap says
//! when a checkout cannot be trusted.
//!
//! Two Windows facts drive everything here:
//!
//! * **The SDK contains symlinks.** With `git`'s `core.symlinks` disabled —
//!   the default outside Developer Mode or an elevated shell — a checkout
//!   materialises each symlink as a *tiny text file containing the link target*
//!   rather than a link. The tree then looks correct but every `.tbd` and header
//!   that was a link is garbage, and the failure surfaces much later as an
//!   inscrutable compile error.
//! * **ldid has no official Windows build.** Rather than ship an unsigned app,
//!   bootstrap says so plainly and points at the WSL bridge.

use std::path::{Path, PathBuf};

use crate::plan::{Status, Step};

/// The explanation printed when symlinks cannot be created.
pub const DEVELOPER_MODE_EXPLANATION: &str = "\
Windows can only create symlinks with Developer Mode enabled or from an \
elevated (Administrator) shell. Without one of those, `git` checks the SDK out \
with each symlink replaced by a small text file containing its target. The \
resulting SDK looks complete but its .tbd stubs and headers are unreadable, so \
the build fails much later with a confusing error.

Fix, in order of preference:
  1. Use the WSL bridge: run `darwinforge bootstrap` inside WSL, where the \
     filesystem supports symlinks natively.
  2. Turn on Developer Mode: Settings > System > For developers > Developer \
     Mode, then re-run `git config --global core.symlinks true` and retry.
  3. Run this shell as Administrator once, so `git` can create the links, \
     then re-run as a normal user.";

/// WSL must be enabled after this command, which requires a reboot.
pub const WSL_INSTALL_AFTER_REBOOT: &str = "\
`wsl --install` enables the Windows Subsystem for Linux and the virtual machine \
platform it needs. It only takes effect after a REBOOT.

After restarting Windows:
  1. Open PowerShell and run `wsl --install -d Ubuntu` if prompted.
  2. Launch Ubuntu from the Start menu and let it finish initialising.
  3. Come back and re-run `darwinforge bootstrap` — it is safe to re-run and \
     will pick up where it stopped.";

/// The two ways darwinforge can build on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Windows LLVM (clang + lld-link's ld64.lld) plus a Windows ldid.
    Native,
    /// Run the whole pipeline inside WSL, translating paths with `wslpath`.
    WslBridge,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Native => "native",
            Mode::WslBridge => "wsl-bridge",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Mode::Native => {
                "native Windows: clang and ld64.lld from Windows LLVM, plus a \
                 Windows ldid if one is available"
            }
            Mode::WslBridge => {
                "WSL bridge: the pipeline runs inside WSL, where the toolchain \
                 and the SDK's symlinks both work; recommended on Windows"
            }
        }
    }
}

/// Choose the build mode for this machine.
///
/// Native is preferred only when it can actually work — which means a real
/// ldid, because there is no official Windows build of it. Otherwise the WSL
/// bridge is the honest recommendation: it is the mode that can finish a build.
pub fn choose_mode(has_ldid: bool, has_wsl: bool) -> Mode {
    if has_ldid {
        Mode::Native
    } else if has_wsl {
        Mode::WslBridge
    } else {
        // Neither toolchain is complete and there is no WSL to bridge to. Say
        // native so the failure names the missing pieces, not the mode.
        Mode::Native
    }
}

/// True when this machine can build natively, i.e. has a signer.
///
/// Without ldid there is no signature, and an unsigned `.ipa` will not install,
/// so producing one quietly would be worse than refusing.
pub fn can_build_natively(has_ldid: bool) -> bool {
    has_ldid
}

/// The step explaining that a Windows SDK checkout needs symlink support.
pub fn sdk_checkout_step() -> Step {
    Step::check("sdk", "check out an iPhoneOS SDK")
        .with_outcome(Status::Failed, DEVELOPER_MODE_EXPLANATION.to_string())
}

/// The step explaining that native mode has no signer.
pub fn no_native_ldid_step() -> Step {
    Step::check("ldid", "sign the app").with_outcome(
        Status::Failed,
        "ldid has no official Windows build, and darwinforge will not produce an \
         unsigned .ipa. Run the pipeline inside WSL, where ldid is available."
            .to_string(),
    )
}

/// The step offering to enable WSL.
pub fn offer_wsl_install_step() -> Step {
    Step::check("wsl", "enable the Windows Subsystem for Linux")
        .with_outcome(Status::Skipped, WSL_INSTALL_AFTER_REBOOT.to_string())
}

/// True when `content` looks like a symlink written out as a text file.
///
/// A materialised symlink is a short file whose contents are a path and little
/// else. A single trailing newline is tolerated because that is exactly what
/// git writes. A NUL byte, or any newline with text after it, means this is real
/// data rather than a link target.
pub fn looks_like_materialised_symlink(content: &[u8]) -> bool {
    if content.contains(&0) {
        return false;
    }
    // At most one trailing newline is allowed.
    let body = match content.strip_suffix(b"\n") {
        Some(without) => without.strip_suffix(b"\r").unwrap_or(without),
        None => content,
    };
    if body.contains(&b'\n') || body.contains(&b'\r') {
        return false;
    }
    let Ok(text) = std::str::from_utf8(body) else { return false };
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 512 {
        return false;
    }
    trimmed.contains('/') && !trimmed.contains(' ')
}

/// How many files the symlink scan will look at before giving up.
pub const MAX_SCANNED_FILES: usize = 20_000;

/// Scan `root` for files that are materialised symlinks.
pub fn find_materialised_symlinks(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut scanned = 0usize;
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            // A real symlink is fine; only regular files can be the fake ones.
            let Ok(metadata) = std::fs::symlink_metadata(&path) else { continue };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            scanned += 1;
            if scanned > MAX_SCANNED_FILES {
                return found;
            }
            if metadata.len() > 512 {
                continue;
            }
            if let Ok(content) = std::fs::read(&path) {
                if looks_like_materialised_symlink(&content) {
                    found.push(path);
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_mode_is_chosen_only_when_there_is_a_signer() {
        // No ldid: an unsigned .ipa is not installable, so the WSL bridge is the
        // mode that can actually finish a build.
        assert_eq!(choose_mode(false, true), Mode::WslBridge);
        // With a Windows ldid, native is simpler and works.
        assert_eq!(choose_mode(true, true), Mode::Native);
        assert_eq!(choose_mode(true, false), Mode::Native);
        // Neither: report native so the failure names the missing ldid.
        assert_eq!(choose_mode(false, false), Mode::Native);
    }

    #[test]
    fn native_builds_require_a_signer() {
        assert!(!can_build_natively(false), "no ldid means no signature");
        assert!(can_build_natively(true));
    }

    #[test]
    fn both_modes_explain_themselves() {
        assert!(Mode::Native.description().contains("LLVM"));
        assert!(Mode::WslBridge.description().contains("WSL"));
        assert_eq!(Mode::Native.label(), "native");
        assert_eq!(Mode::WslBridge.label(), "wsl-bridge");
    }

    #[test]
    fn the_developer_mode_text_explains_the_cause_and_the_fixes() {
        let text = DEVELOPER_MODE_EXPLANATION;
        assert!(text.contains("Developer Mode"), "names the cause: {text}");
        assert!(text.contains("git"), "names git: {text}");
        assert!(text.contains("WSL"), "offers the bridge: {text}");
        assert!(text.contains("Administrator"), "offers elevation: {text}");
    }

    #[test]
    fn the_wsl_text_says_a_reboot_is_required_and_that_rerunning_is_safe() {
        let text = WSL_INSTALL_AFTER_REBOOT;
        assert!(text.contains("REBOOT"), "must warn about the reboot: {text}");
        assert!(text.contains("safe to re-run"), "bootstrap must be resumable: {text}");
    }

    #[test]
    fn the_failing_steps_carry_their_explanations() {
        let sdk = sdk_checkout_step();
        assert_eq!(sdk.status, Status::Failed);
        assert!(sdk.detail.expect("must explain").contains("Developer Mode"));

        let ldid = no_native_ldid_step();
        assert_eq!(ldid.status, Status::Failed);
        let detail = ldid.detail.expect("must explain");
        assert!(detail.contains("unsigned"), "says why it refuses: {detail}");
        assert!(detail.contains("WSL"), "offers the alternative: {detail}");

        let wsl = offer_wsl_install_step();
        assert_eq!(wsl.status, Status::Skipped);
        assert!(wsl.detail.expect("must explain").contains("REBOOT"));
    }

    #[test]
    fn a_relative_path_in_a_tiny_file_looks_like_a_materialised_symlink() {
        // Exactly what git writes when core.symlinks is off.
        assert!(looks_like_materialised_symlink(b"../lib/libobjc.tbd"));
        assert!(looks_like_materialised_symlink(b"usr/include/stdio.h\n"));
        assert!(looks_like_materialised_symlink(b"  ./relative/target  "));
    }

    #[test]
    fn real_file_contents_are_not_mistaken_for_symlinks() {
        assert!(!looks_like_materialised_symlink(b""));
        assert!(!looks_like_materialised_symlink(b"hello world"));
        assert!(!looks_like_materialised_symlink(b"no-slash-here"));
        // Real data spanning lines, not a path.
        assert!(!looks_like_materialised_symlink(b"a/b\nc/d"));
        // Binary content.
        assert!(!looks_like_materialised_symlink(&[0x00, 0x2f, 0x62]));
        // Far too long to be a path.
        assert!(!looks_like_materialised_symlink(&vec![b'a'; 600]));
        // A path followed by more than one line is real content.
        assert!(!looks_like_materialised_symlink(b"path/to/x\n\nsecond block\n"));
    }

    #[test]
    fn scanning_a_real_directory_finds_only_the_planted_fakes() {
        let dir = std::env::temp_dir()
            .join(format!("darwinforge-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("usr/lib")).expect("mkdir");

        // A planted materialised symlink, in a subdirectory.
        std::fs::write(dir.join("usr/lib/libobjc.tbd"), "usr/include/objc.h").expect("write");
        // Genuine files that must not be flagged.
        std::fs::write(dir.join("usr/lib/real.tbd"), "not a link at all").expect("write");
        std::fs::write(dir.join("usr/lib/binary.bin"), [0u8, 1, 2, 3]).expect("write");

        let found = find_materialised_symlinks(&dir);
        assert_eq!(found.len(), 1, "exactly the planted fake: {found:?}");
        assert!(found[0].ends_with("libobjc.tbd"), "{found:?}");

        // A missing directory yields nothing rather than an error.
        assert!(find_materialised_symlinks(Path::new("/definitely/not/here")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod limit_tests {
    use super::MAX_SCANNED_FILES;

    /// The scan bound is a constant, so clippy rejects any `assert!` on it
    /// (`assertions_on_constants`). Pinning the exact number here makes a change
    /// to the bound a deliberate, visible edit rather than a silent one.
    #[test]
    fn the_scan_bound_is_explicit() {
        assert_eq!(MAX_SCANNED_FILES, 20_000);
    }
}
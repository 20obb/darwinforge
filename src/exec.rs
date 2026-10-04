//! Running external toolchain binaries.
//!
//! Child stdout/stderr are inherited, never captured-and-dropped: warnings from
//! clang or ld must reach the user verbatim.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::reporter::Reporter;

/// Hint for the "tool not found" case, per binary.
fn fix_hint(program: &str) -> String {
    let name = Path::new(program)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| program.to_string());
    match name.as_str() {
        n if n.starts_with("clang") => {
            "install LLVM/clang and put it on PATH, or set DARWINFORGE_CLANG=/path/to/clang"
                .to_string()
        }
        n if n.starts_with("ld64") => {
            "install LLVM's lld (`ld64.lld`, shipped in LLVM releases) or set \
             DARWINFORGE_LINKER=/path/to/ld64.lld"
                .to_string()
        }
        "ld" | "ld64" => {
            "install cctools-port for Linux (provides `ld`) or LLVM's lld \
             (`ld64.lld`), or set DARWINFORGE_LINKER"
                .to_string()
        }
        "swiftc" => {
            "install a Swift toolchain for Linux and point DARWINFORGE_SWIFTC at it"
                .to_string()
        }
        "ldid" => {
            "install ldid (https://github.com/ProcursusTeam/ldid or your package \
             manager) and put it on PATH, or set DARWINFORGE_LDID"
                .to_string()
        }
        "zip" => {
            "install the `zip` command (Info-ZIP), or drop the \
             `zip = false` override in darwinforge.toml to use the built-in writer"
                .to_string()
        }
        other => format!("make `{other}` available on PATH"),
    }
}

/// Run `program` with `args`, streaming its output to the terminal.
///
/// Returns `Error::Prereq` when the binary does not exist (with an actionable
/// fix), and `Error::ToolFailed` on a non-zero exit status.
pub fn run(program: &str, args: &[String], cwd: Option<&Path>, reporter: &Reporter) -> Result<()> {
    reporter.command(program, args);
    let mut command = std::process::Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let status = command.status().map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            Error::Prereq {
                what: format!("`{program}` was not found on PATH"),
                fix: fix_hint(program),
            }
        } else {
            Error::io(format!("failed to execute `{program}`"), source)
        }
    })?;
    if status.success() {
        return Ok(());
    }
    Err(Error::ToolFailed {
        program: program.to_string(),
        args: args.to_vec(),
        code: status.code(),
    })
}

/// Run a tool purely to learn its version (`clang --version`), returning
/// `Ok(None)` when the binary is missing. Used by `doctor`.
pub fn probe(program: &str, args: &[String]) -> Result<Option<String>> {
    let output = match std::process::Command::new(program).args(args).output() {
        Ok(output) => output,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::io(format!("failed to execute `{program}`"), source))
        }
    };
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().next().map(|line| line.trim().to_string()))
}

/// Look up an executable on `PATH` (plus `PATHEXT` on Windows, so the PoC is
/// usable from a native Windows shell as well as WSL).
pub fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let mut extensions: Vec<String> = vec![String::new()];
    if cfg!(windows) {
        if let Some(pathext) = std::env::var_os("PATHEXT") {
            extensions = std::env::split_paths(&pathext)
                .filter_map(|ext| {
                    let ext = ext.to_string_lossy().to_string();
                    ext.strip_prefix('.').map(|ext| format!(".{ext}"))
                })
                .collect();
            extensions.push(String::new());
        }
    }
    for dir in std::env::split_paths(&path) {
        for extension in &extensions {
            let candidate = dir.join(format!("{program}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolve a tool: the `DARWINFORGE_<suffix>` environment override first (with
/// the legacy `IPAFORGE_<suffix>` name as a fallback), then `PATH`.
pub fn resolve(suffix: &str, program: &str) -> Option<PathBuf> {
    match crate::compat::env_var(suffix) {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_file() {
                Some(path)
            } else {
                None
            }
        }
        None => which(program),
    }
}
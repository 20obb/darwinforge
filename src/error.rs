//! Error type and the exit codes the CLI maps it to.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Convenience alias used by every module in the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// Bad command line. Exit code 2, same as most Unix tools.
    Usage(String),
    /// `darwinforge.toml` could not be read or is semantically invalid.
    Config {
        path: PathBuf,
        line: Option<usize>,
        message: String,
    },
    /// An external tool (clang, ld, ldid, ...) exited non-zero.
    ToolFailed {
        program: String,
        args: Vec<String>,
        code: Option<i32>,
    },
    /// A prerequisite (tool or SDK path) is missing; carries the fix hint.
    Prereq { what: String, fix: String },
    /// Local I/O failure, annotated with what we were trying to do.
    Io { context: String, source: io::Error },
    /// Malformed or inconsistent data we produced or consumed (plist, zip, Mach-O).
    Format { what: String, message: String },
    /// A feature that this PoC deliberately does not implement.
    Unsupported { message: String },
    /// The environment is not set up: run `darwinforge bootstrap`.
    ///
    /// Exit code 5, distinct from 4 ("a prerequisite is missing") so a CI job
    /// can tell "your SDK path is wrong" apart from "this machine has never
    /// been bootstrapped".
    Setup { what: String, fix: String },
}

impl Error {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Error::Io { context: context.into(), source }
    }

    pub fn format(what: impl Into<String>, message: impl Into<String>) -> Self {
        Error::Format { what: what.into(), message: message.into() }
    }

    /// Stable, documented exit codes.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) => 2,
            Error::Config { .. } => 3,
            Error::ToolFailed { .. } | Error::Io { .. } | Error::Format { .. } => 1,
            Error::Prereq { .. } | Error::Unsupported { .. } => 4,
            Error::Setup { .. } => 5,
        }
    }

    /// Build a `Setup` error with an actionable fix.
    pub fn setup(what: impl Into<String>, fix: impl Into<String>) -> Self {
        Error::Setup { what: what.into(), fix: fix.into() }
    }

    pub fn config_at(path: &Path, line: Option<usize>, message: impl Into<String>) -> Self {
        Error::Config { path: path.to_path_buf(), line, message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usage(msg) => {
                write!(f, "{msg}\n\nRun `darwinforge --help` for usage.")
            }
            Error::Config { path, line, message } => {
                write!(f, "{}", path.display())?;
                if let Some(line) = line {
                    write!(f, ":{line}")?;
                }
                write!(f, ": {message}")
            }
            Error::ToolFailed { program, args, code } => {
                write!(f, "external tool `{}` failed", program)?;
                match code {
                    Some(code) => write!(f, " with exit code {code}")?,
                    None => write!(f, " (killed by signal)")?,
                }
                write!(f, "\n  command: {program} {}", args.join(" "))?;
                write!(
                    f,
                    "\n  The tool's own diagnostics are printed above, unfiltered."
                )
            }
            Error::Prereq { what, fix } => write!(f, "{what}\n  fix: {fix}"),
            Error::Setup { what, fix } => write!(f, "{what}\n  fix: {fix}"),
            Error::Io { context, source } => write!(f, "{context}: {source}"),
            Error::Format { what, message } => write!(f, "{what}: {message}"),
            Error::Unsupported { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
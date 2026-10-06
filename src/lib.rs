//! DarwinForge — turn iOS sources into an installable `.ipa`, on Linux, with no
//! Mac.
//!
//! The crate is split so that every pipeline stage lives in its own module with
//! a small, testable interface:
//!
//! ```text
//! config -> discovery -> compile -> link -> bundle -> sign -> package
//! ```
//!
//! `darwinforge` never implements a compiler, a linker or a code signer: those are
//! delegated to external binaries (clang, ld64.lld / cctools `ld`, ldid) that
//! are discovered at run time by [`doctor`].

pub mod bundle;
pub mod bootstrap;
pub mod clean;
pub mod cli;
pub mod compat;
pub mod completions;
pub mod compile;
pub mod config;
pub mod discovery;
pub mod distro;
pub mod doctor;
pub mod error;
pub mod exec;
pub mod global_config;
pub mod infer;
pub mod link;
pub mod mach;
pub mod package;
pub mod packages;
pub mod paths;
pub mod plan;
pub mod project;
pub mod pipeline;
pub mod plist;
pub mod prompt;
pub mod reporter;
pub mod scaffold;
pub mod sdk;
pub mod sdkcmd;
pub mod sdkpath;
pub mod sdksource;
pub mod sdkversion;
pub mod sign;
pub mod tomlite;
pub mod toolfind;
pub mod windows;

pub use error::{Error, Result};
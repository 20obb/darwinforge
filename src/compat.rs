//! Backward compatibility with the tool's former name, `ipaforge`.
//!
//! `darwinforge` keeps two deliberate legacy paths alive so an existing
//! checkout keeps working after the rename:
//!
//! 1. **Project config file** — `darwinforge.toml` is the current name; if it is
//!    missing but `ipaforge.toml` exists, that file is loaded and a one-line
//!    deprecation notice is printed.
//! 2. **Environment variables** — `DARWINFORGE_*` is read first; the old
//!    `IPAFORGE_*` names are still honoured as a fallback, with a warning, so a
//!    muscle-memory `export IPAFORGE_SDK=...` keeps working.
//!
//! The legacy global config/data *directories* are handled by the config layer
//! (see [`crate::global_config`]), which offers to migrate them in `setup`.
//!
//! Everything else was renamed outright; a case-insensitive search for
//! `ipaforge` over the tree should only hit this file, the migration code, and
//! the documentation that explains the fallbacks.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The former name of this tool, kept only for the fallbacks below.
pub const LEGACY_NAME: &str = "ipaforge";

/// The current display name.
pub const DISPLAY_NAME: &str = "DarwinForge";

/// Preferred name of the per-project configuration file.
pub const CONFIG_FILE_NAME: &str = "darwinforge.toml";

/// Former name of the per-project configuration file, still read as a fallback.
pub const LEGACY_CONFIG_FILE_NAME: &str = "ipaforge.toml";

/// Which config file a project actually used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfigFile {
    pub path: PathBuf,
    /// True when this is the legacy `ipaforge.toml`.
    pub legacy: bool,
}

/// Find the project config file: `darwinforge.toml` first, `ipaforge.toml`
/// second. The caller prints [`deprecation_notice`] when `legacy` is set.
pub fn find_project_config(project_root: &Path) -> Option<ProjectConfigFile> {
    let current = project_root.join(CONFIG_FILE_NAME);
    if current.is_file() {
        return Some(ProjectConfigFile { path: current, legacy: false });
    }
    let legacy = project_root.join(LEGACY_CONFIG_FILE_NAME);
    if legacy.is_file() {
        return Some(ProjectConfigFile { path: legacy, legacy: true });
    }
    None
}

/// The single line printed when a legacy `ipaforge.toml` is picked up.
pub fn deprecation_notice(legacy_path: &Path) -> String {
    format!(
        "warning: {} still holds the former name of this tool's config file; \
         rename it to {} (`darwinforge init` writes the new name)",
        legacy_path.display(),
        CONFIG_FILE_NAME
    )
}

/// Read an environment variable, preferring the current `DARWINFORGE_*` name
/// and falling back to the legacy `IPAFORGE_*` one with a warning.
///
/// `suffix` is the part after the prefix, e.g. `"SDK"` for `DARWINFORGE_SDK`.
pub fn env_var(suffix: &str) -> Option<OsString> {
    let current = format!("DARWINFORGE_{suffix}");
    if let Some(value) = non_empty_env(&current) {
        return Some(value);
    }
    let legacy = format!("IPAFORGE_{suffix}");
    let value = non_empty_env(&legacy)?;
    eprintln!(
        "warning: {legacy} is deprecated; use {current} instead \
         (the tool was renamed from ipaforge to darwinforge)"
    );
    Some(value)
}

/// The legacy name of a `DARWINFORGE_*` variable, for help text and docs.
pub fn legacy_env_name(suffix: &str) -> String {
    format!("IPAFORGE_{suffix}")
}

fn non_empty_env(name: &str) -> Option<OsString> {
    match std::env::var_os(name) {
        Some(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("darwinforge-compat-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn prefers_the_current_config_name() {
        let dir = scratch("current");
        std::fs::write(dir.join(CONFIG_FILE_NAME), "").expect("write");
        std::fs::write(dir.join(LEGACY_CONFIG_FILE_NAME), "").expect("write");
        let found = find_project_config(&dir).expect("must find one");
        assert!(!found.legacy, "darwinforge.toml must win");
        assert!(found.path.ends_with(CONFIG_FILE_NAME));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn falls_back_to_the_legacy_config_name() {
        let dir = scratch("legacy");
        std::fs::write(dir.join(LEGACY_CONFIG_FILE_NAME), "").expect("write");
        let found = find_project_config(&dir).expect("must fall back");
        assert!(found.legacy);
        assert!(found.path.ends_with(LEGACY_CONFIG_FILE_NAME));
        let notice = deprecation_notice(&found.path);
        assert!(notice.contains(CONFIG_FILE_NAME), "names the new file: {notice}");
        assert!(notice.contains("former name"), "says why: {notice}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_with_neither_file_is_not_a_project() {
        let dir = scratch("empty");
        assert!(find_project_config(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_env_name_is_derived_from_the_suffix() {
        assert_eq!(legacy_env_name("SDK"), "IPAFORGE_SDK");
        assert_eq!(legacy_env_name("CLANG"), "IPAFORGE_CLANG");
        assert_eq!(legacy_env_name("TEST_SDK"), "IPAFORGE_TEST_SDK");
    }

    #[test]
    fn current_env_name_is_read_first() {
        let current = "DARWINFORGE_COMPAT_TEST_CURRENT";
        std::env::set_var(current, "new");
        assert_eq!(env_var("COMPAT_TEST_CURRENT").as_deref(), Some(std::ffi::OsStr::new("new")));
        std::env::remove_var(current);
    }

    #[test]
    fn unset_env_is_none() {
        assert!(env_var("COMPAT_TEST_DEFINITELY_UNSET_9182").is_none());
    }
}
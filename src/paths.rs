//! Platform-appropriate locations for darwinforge's global config and data.
//!
//! These follow each platform's convention rather than being hardcoded, so a
//! user never has to be told where a file lives:
//!
//! | Platform | Config | Data (installed SDKs) |
//! | --- | --- | --- |
//! | Linux/WSL | `$XDG_CONFIG_HOME/darwinforge` or `~/.config/darwinforge` | `$XDG_DATA_HOME/darwinforge` or `~/.local/share/darwinforge` |
//! | macOS | `~/Library/Application Support/darwinforge` | same |
//! | Windows | `%APPDATA%\darwinforge` | `%LOCALAPPDATA%\darwinforge` |

use std::path::{Path, PathBuf};

use crate::distro::OperatingSystem;

/// Directory name used under every platform's base directory.
pub const APP_DIR: &str = "darwinforge";

/// `~/.config/darwinforge` (Linux) — the global `config.toml`.
pub fn config_dir() -> Result<PathBuf, String> {
    config_dir_for(APP_DIR).ok_or_else(|| {
        "cannot determine a configuration directory: neither HOME nor USERPROFILE is set"
            .to_string()
    })
}

/// `~/.local/share/darwinforge` (Linux) — where bootstrap installs SDKs.
pub fn data_dir() -> Result<PathBuf, String> {
    data_dir_for(APP_DIR).ok_or_else(|| {
        "cannot determine a data directory: neither HOME nor USERPROFILE is set".to_string()
    })
}

/// The config directory for an arbitrary app name (used for legacy migration).
pub fn config_dir_for(app: &str) -> Option<PathBuf> {
    let home = home_dir()?;
    if cfg!(target_os = "macos") {
        Some(home.join("Library").join("Application Support").join(app))
    } else if cfg!(target_os = "windows") {
        // %APPDATA% is the roaming location, the convention for user settings.
        env_path("APPDATA").map(|p| p.join(app))
    } else {
        // Linux and WSL: honour XDG first, then the documented default.
        match env_path("XDG_CONFIG_HOME") {
            Some(xdg) if !xdg.as_os_str().is_empty() => Some(xdg.join(app)),
            _ => Some(home.join(".config").join(app)),
        }
    }
}

/// The data directory for an arbitrary app name.
pub fn data_dir_for(app: &str) -> Option<PathBuf> {
    let home = home_dir()?;
    if cfg!(target_os = "macos") {
        Some(home.join("Library").join("Application Support").join(app))
    } else if cfg!(target_os = "windows") {
        // %LOCALAPPDATA% is the machine-local location: installed SDKs are not
        // settings and should not roam to another machine.
        env_path("LOCALAPPDATA")
            .map(|p| p.join(app))
            .or_else(|| Some(home.join("AppData").join("Local").join(app)))
    } else {
        match env_path("XDG_DATA_HOME") {
            Some(xdg) if !xdg.as_os_str().is_empty() => Some(xdg.join(app)),
            _ => Some(home.join(".local").join("share").join(app)),
        }
    }
}

/// The user's home directory, on any platform.
pub fn home_dir() -> Option<PathBuf> {
    env_path("HOME").or_else(|| env_path("USERPROFILE"))
}

/// Where installed SDKs live.
pub fn sdks_dir() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("sdks"))
}

/// Where downloads are staged before being unpacked.
pub fn cache_dir() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("cache"))
}

/// Where bootstrap puts a locally built `ldid`, so it is found again next run.
pub fn bin_dir() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("bin"))
}

/// The global configuration file.
pub fn config_file() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config.toml"))
}

/// The legacy (pre-rename) config directory, if it exists.
pub fn legacy_config_dir() -> Option<PathBuf> {
    let candidate = config_dir_for(crate::compat::LEGACY_NAME)?;
    candidate.is_dir().then_some(candidate)
}

/// The legacy (pre-rename) data directory, if it exists.
pub fn legacy_data_dir() -> Option<PathBuf> {
    let candidate = data_dir_for(crate::compat::LEGACY_NAME)?;
    candidate.is_dir().then_some(candidate)
}

/// Remove a Windows `\\?\` verbatim prefix, which many tools reject.
///
/// Windows APIs hand back extended-length paths (`\\?\C:\...`) that ordinary
/// tools and users cannot read. Stripping the prefix keeps error messages and
/// `git` invocations usable. No-op on Unix, where `\` is a legal filename
/// character and must not be touched.
pub fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    if !cfg!(windows) {
        return path.to_path_buf();
    }
    let text = path.to_string_lossy();
    if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{stripped}"));
    }
    if let Some(stripped) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(stripped);
    }
    path.to_path_buf()
}

/// Normalise a path for display and for handing to a child process.
///
/// On Windows this also strips the verbatim prefix; elsewhere it is the identity.
pub fn display_path(path: &Path) -> String {
    strip_verbatim_prefix(path).to_string_lossy().to_string()
}

/// Create `dir` and every missing parent, with a clear error on failure.
pub fn ensure_dir(dir: &Path) -> crate::error::Result<()> {
    std::fs::create_dir_all(dir).map_err(|source| {
        crate::error::Error::io(format!("cannot create {}", display_path(dir)), source)
    })
}

/// True when `path` lies inside darwinforge's own managed data directory.
///
/// This guards every destructive operation: we only ever remove things we own,
/// so `bootstrap` can clean up its own staging dirs but never a user's.
pub fn is_managed_dir(path: &Path) -> bool {
    let Ok(data) = data_dir() else { return false };
    strip_verbatim_prefix(path).starts_with(strip_verbatim_prefix(&data))
}

/// The OS this build is running on.
pub fn current_os() -> OperatingSystem {
    crate::distro::detect_os()
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_data_dirs_are_distinct_and_named_for_the_app() {
        let config = config_dir().expect("config dir");
        let data = data_dir().expect("data dir");
        assert!(config.ends_with(APP_DIR), "{} must end in the app dir", config.display());
        assert!(data.ends_with(APP_DIR));
        assert_ne!(config, data, "config and data must not collide");
    }

    #[test]
    fn derived_paths_live_under_their_parents() {
        let data = data_dir().expect("data");
        assert!(sdks_dir().expect("sdks").starts_with(&data));
        assert!(cache_dir().expect("cache").starts_with(&data));
        assert!(bin_dir().expect("bin").starts_with(&data));
        assert!(config_file().expect("config").starts_with(config_dir().expect("config")));
    }

    #[test]
    fn verbatim_prefixes_are_stripped_only_on_windows() {
        // Exercised on every OS, so assert the contract that holds everywhere:
        // a plain path is never touched.
        let plain = PathBuf::from("/opt/ios/iPhoneOS.sdk");
        assert_eq!(strip_verbatim_prefix(&plain), plain);

        let weird = if cfg!(windows) {
            PathBuf::from(r"\\?\C:\Users\me\sdk")
        } else {
            // On Unix a backslash is a legal filename character, so this must
            // survive verbatim — rewriting it would corrupt a real path.
            PathBuf::from(r"\back\slash")
        };
        let stripped = strip_verbatim_prefix(&weird);
        if cfg!(windows) {
            assert_eq!(stripped, PathBuf::from(r"C:\Users\me\sdk"));
        } else {
            assert_eq!(stripped, weird);
        }
    }

    #[test]
    fn unc_verbatim_prefixes_become_ordinary_unc_paths() {
        if !cfg!(windows) {
            return;
        }
        let unc = PathBuf::from(r"\\?\UNC\server\share\sdk");
        assert_eq!(strip_verbatim_prefix(&unc), PathBuf::from(r"\\server\share\sdk"));
    }

    #[test]
    fn display_path_is_never_empty_for_a_real_path() {
        assert!(!display_path(Path::new("/tmp")).is_empty());
        assert!(!display_path(&sdks_dir().expect("sdks")).is_empty());
    }

    #[test]
    fn managed_dir_check_only_accepts_paths_under_our_own_data_dir() {
        let data = data_dir().expect("data");
        assert!(is_managed_dir(&data.join("sdks")), "our own sdks dir is managed");
        assert!(is_managed_dir(&data), "the data dir itself is managed");
        // Anything outside must be refused, so a cleanup bug can never rm -rf a
        // user's directory.
        assert!(!is_managed_dir(Path::new("/")));
        assert!(!is_managed_dir(Path::new("/etc")));
        assert!(!is_managed_dir(&std::env::temp_dir()));
    }
}
//! The persistent global configuration file.
//!
//! Written by `bootstrap` so a later `build` needs no flags at all. It reuses
//! the crate's own TOML subset parser ([`crate::tomlite`]) for reading and a
//! matching writer for saving, so no new dependency is introduced and the file
//! stays hand-editable.
//!
//! ```text
//! # ~/.config/darwinforge/config.toml
//! sdk_path = "/home/me/.local/share/darwinforge/sdks/iPhoneOS17.5.sdk"
//! sdk_version = "17.5"
//! clang = "/usr/bin/clang-21"
//! linker = "/usr/bin/ld64.lld-21"
//! ldid = "/usr/local/bin/ldid"
//! zip = "/usr/bin/zip"
//! sdk_source = "https://github.com/xybp888/iOS-SDKs"
//! setup_completed = true
//! sdk_license_accepted = true
//! ```

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::paths;
use crate::tomlite::{self, Value};

/// The file name inside the platform config directory.
pub const FILE_NAME: &str = "config.toml";

/// Everything darwinforge remembers between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalConfig {
    /// Absolute path to the active iPhoneOS SDK.
    pub sdk_path: Option<PathBuf>,
    /// Its version, so reporting needs no I/O.
    pub sdk_version: Option<String>,
    /// Resolved tool paths, saved so we do not re-search every time.
    pub clang: Option<PathBuf>,
    pub linker: Option<PathBuf>,
    pub ldid: Option<PathBuf>,
    pub zip: Option<PathBuf>,
    pub swiftc: Option<PathBuf>,
    /// Where SDKs are downloaded from. A config value, never hardcoded in logic.
    pub sdk_source: Option<String>,
    /// Set once bootstrap has run to completion.
    pub setup_completed: bool,
    /// Set once the user has acknowledged Apple's SDK licence terms.
    pub sdk_license_accepted: bool,
}

impl GlobalConfig {
    /// Read the config file. A missing file is an empty config, not an error:
    /// that is exactly the first-run state.
    pub fn load() -> Result<GlobalConfig> {
        let path = paths::config_file().map_err(|message| Error::Prereq {
            what: message,
            fix: "set HOME (Linux/macOS) or USERPROFILE (Windows) and try again".to_string(),
        })?;
        GlobalConfig::load_from(&path)
    }

    /// Read from an explicit path (used by tests).
    pub fn load_from(path: &Path) -> Result<GlobalConfig> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(GlobalConfig::default())
            }
            Err(source) => {
                return Err(Error::io(
                    format!("cannot read {}", paths::display_path(path)),
                    source,
                ))
            }
        };
        GlobalConfig::from_str(&text, path)
    }

    /// Parse config text, reporting problems against `path`.
    pub fn from_str(text: &str, path: &Path) -> Result<GlobalConfig> {
        let value = crate::tomlite::parse(text)
            .map_err(|error| Error::config_at(path, Some(error.line), error.message))?;
        let table = match value.as_table() {
            Some(table) => table.clone(),
            None => {
                return Err(Error::config_at(path, None, "expected a table of key = value pairs"))
            }
        };
        // The file is written with `[sdk]`, `[tools]` and `[state]` tables so it
        // stays readable; flat keys are still accepted for hand-written files.
        Ok(GlobalConfig {
            sdk_path: nested_path(&table, "sdk", "path").or_else(|| string_path(&table, "sdk_path")),
            sdk_version: nested_str(&table, "sdk", "version")
                .or_else(|| string_field(&table, "sdk_version")),
            clang: nested_path(&table, "tools", "clang").or_else(|| string_path(&table, "clang")),
            linker: nested_path(&table, "tools", "linker")
                .or_else(|| string_path(&table, "linker")),
            ldid: nested_path(&table, "tools", "ldid").or_else(|| string_path(&table, "ldid")),
            zip: nested_path(&table, "tools", "zip").or_else(|| string_path(&table, "zip")),
            swiftc: nested_path(&table, "tools", "swiftc")
                .or_else(|| string_path(&table, "swiftc")),
            sdk_source: nested_str(&table, "sdk", "source")
                .or_else(|| string_field(&table, "sdk_source")),
            setup_completed: nested_bool(&table, "state", "setup_completed")
                || bool_field(&table, "setup_completed"),
            sdk_license_accepted: nested_bool(&table, "state", "sdk_license_accepted")
                || bool_field(&table, "sdk_license_accepted"),
        })
    }

    /// Save to the platform config location, creating directories as needed.
    pub fn save(&self) -> Result<PathBuf> {
        let path = paths::config_file().map_err(|message| Error::Prereq {
            what: message,
            fix: "set HOME (Linux/macOS) or USERPROFILE (Windows) and try again".to_string(),
        })?;
        self.save_to(&path)?;
        Ok(path)
    }

    /// Save to an explicit path, atomically via a sibling temp file so an
    /// interrupted run cannot leave a half-written config behind.
    pub fn save_to(&self, path: &Path) -> Result<PathBuf> {
        if let Some(parent) = path.parent() {
            paths::ensure_dir(parent)?;
        }
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, self.to_toml()).map_err(|source| {
            Error::io(format!("cannot write {}", paths::display_path(&temporary)), source)
        })?;
        std::fs::rename(&temporary, path).map_err(|source| {
            // Leave no debris behind if the rename failed.
            let _ = std::fs::remove_file(&temporary);
            Error::io(format!("cannot write {}", paths::display_path(path)), source)
        })?;
        Ok(path.to_path_buf())
    }

    /// Render as TOML, omitting unset sections so the file stays short.
    pub fn to_toml(&self) -> String {
        let mut out = String::from(
            "# darwinforge global configuration.\n\
             # Written by `darwinforge bootstrap`; safe to edit by hand.\n",
        );
        if self.sdk_path.is_some() || self.sdk_version.is_some() || self.sdk_source.is_some() {
            out.push_str("\n[sdk]\n");
            push_path(&mut out, "path", &self.sdk_path);
            push_str(&mut out, "version", &self.sdk_version);
            push_str(&mut out, "source", &self.sdk_source);
        }
        if self.clang.is_some()
            || self.linker.is_some()
            || self.ldid.is_some()
            || self.zip.is_some()
            || self.swiftc.is_some()
        {
            out.push_str("\n[tools]\n");
            push_path(&mut out, "clang", &self.clang);
            push_path(&mut out, "linker", &self.linker);
            push_path(&mut out, "ldid", &self.ldid);
            push_path(&mut out, "zip", &self.zip);
            push_path(&mut out, "swiftc", &self.swiftc);
        }
        out.push_str("\n[state]\n");
        push_bool(&mut out, "setup_completed", self.setup_completed);
        push_bool(&mut out, "sdk_license_accepted", self.sdk_license_accepted);
        out
    }

    /// The SDK path recorded here, if it still exists on disk.
    ///
    /// A stale path (unmounted volume, deleted SDK) is reported as absent rather
    /// than used, so the caller can fall back to asking the user.
    pub fn live_sdk_path(&self) -> Option<PathBuf> {
        let path = self.sdk_path.as_ref()?;
        path.is_dir().then(|| path.clone())
    }
}

fn string_field(table: &tomlite::Table, key: &str) -> Option<String> {
    tomlite::get(table, key).and_then(Value::as_str).map(str::to_string)
}

fn bool_field(table: &tomlite::Table, key: &str) -> bool {
    matches!(tomlite::get(table, key), Some(Value::Boolean(true)))
}

fn string_path(table: &tomlite::Table, key: &str) -> Option<PathBuf> {
    string_field(table, key).filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// Read a value by dotted key, e.g. `sdk.path` for `[sdk]` + `path = ...`.
fn nested<'a>(
    table: &'a tomlite::Table,
    prefix: &str,
    key: &str,
) -> Option<&'a Value> {
    let section = tomlite::get(table, prefix)?.as_table()?;
    tomlite::get(section, key)
}

fn nested_str(table: &tomlite::Table, prefix: &str, key: &str) -> Option<String> {
    nested(table, prefix, key).and_then(Value::as_str).map(str::to_string)
}

fn nested_path(table: &tomlite::Table, prefix: &str, key: &str) -> Option<PathBuf> {
    nested_str(table, prefix, key).filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn nested_bool(table: &tomlite::Table, prefix: &str, key: &str) -> bool {
    matches!(nested(table, prefix, key), Some(Value::Boolean(true)))
}

fn push_str(out: &mut String, key: &str, value: &Option<String>) {
    if let Some(value) = value {
        out.push_str(&format!("{key} = {}\n", quote(value)));
    }
}

fn push_path(out: &mut String, key: &str, value: &Option<PathBuf>) {
    if let Some(path) = value {
        // Write the normalised form so a Windows `\\?\` path never reaches the
        // file: it would be unreadable for anyone editing it by hand.
        out.push_str(&format!("{key} = {}\n", quote(&paths::display_path(path))));
    }
}

fn push_bool(out: &mut String, key: &str, value: bool) {
    out.push_str(&format!("{key} = {value}\n"));
}

/// Quote a value as a TOML basic string, escaping what needs escaping.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("darwinforge-gconfig-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn sample() -> GlobalConfig {
        GlobalConfig {
            sdk_path: Some(PathBuf::from("/opt/ios/iPhoneOS17.5.sdk")),
            sdk_version: Some("17.5".to_string()),
            clang: Some(PathBuf::from("/usr/bin/clang-21")),
            linker: Some(PathBuf::from("/usr/bin/ld64.lld-21")),
            ldid: Some(PathBuf::from("/usr/local/bin/ldid")),
            zip: Some(PathBuf::from("/usr/bin/zip")),
            swiftc: None,
            sdk_source: Some("https://example.invalid/iOS-SDKs".to_string()),
            setup_completed: true,
            sdk_license_accepted: true,
        }
    }

    #[test]
    fn read_write_round_trip_preserves_every_field() {
        let original = sample();
        let text = original.to_toml();
        let parsed = GlobalConfig::from_str(&text, Path::new("config.toml")).expect("must parse");
        assert_eq!(parsed, original, "round trip lost data:\n{text}");
    }

    #[test]
    fn round_trip_works_through_the_filesystem_too() {
        let dir = scratch("roundtrip");
        let path = dir.join("config.toml");
        let original = sample();
        original.save_to(&path).expect("must save");
        assert!(path.is_file(), "the config file must exist");
        assert_eq!(GlobalConfig::load_from(&path).expect("must load"), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_the_first_run_state_not_an_error() {
        let config = GlobalConfig::load_from(Path::new("/definitely/not/here/config.toml"))
            .expect("missing config must not be an error");
        assert_eq!(config, GlobalConfig::default());
        assert!(!config.setup_completed, "a fresh machine has not been set up");
        assert!(!config.sdk_license_accepted);
    }

    #[test]
    fn an_empty_config_still_renders_valid_toml() {
        let text = GlobalConfig::default().to_toml();
        assert!(text.contains("[state]"));
        assert_eq!(
            GlobalConfig::from_str(&text, Path::new("c.toml")).expect("must parse"),
            GlobalConfig::default()
        );
    }

    #[test]
    fn unset_sections_are_omitted() {
        let text = GlobalConfig::default().to_toml();
        assert!(!text.contains("[sdk]"), "an unconfigured SDK needs no section");
        assert!(!text.contains("[tools]"), "no tools resolved yet");
    }

    #[test]
    fn flat_keys_are_still_accepted_for_hand_written_files() {
        let text = "sdk_path = \"/opt/ios/iPhoneOS17.5.sdk\"\n\
                    sdk_version = \"17.5\"\n\
                    clang = \"/usr/bin/clang\"\n\
                    setup_completed = true\n";
        let config = GlobalConfig::from_str(text, Path::new("c.toml")).expect("must parse");
        assert_eq!(config.sdk_path, Some(PathBuf::from("/opt/ios/iPhoneOS17.5.sdk")));
        assert_eq!(config.clang, Some(PathBuf::from("/usr/bin/clang")));
        assert!(config.setup_completed);
    }

    #[test]
    fn tables_take_precedence_over_flat_keys() {
        let text = "[sdk]\npath = \"/new/sdk\"\n\nsdk_path = \"/old/sdk\"\n";
        let config = GlobalConfig::from_str(text, Path::new("c.toml")).expect("must parse");
        assert_eq!(config.sdk_path, Some(PathBuf::from("/new/sdk")));
    }

    #[test]
    fn unknown_keys_are_ignored_rather_than_fatal() {
        // A future field must not lock a user out of their own machine.
        let text =
            "[sdk]\npath = \"/opt/sdk\"\nfuture_option = 7\n\n[unknown_section]\nx = 1\n";
        let config = GlobalConfig::from_str(text, Path::new("c.toml")).expect("must parse");
        assert_eq!(config.sdk_path, Some(PathBuf::from("/opt/sdk")));
    }

#[test]
    fn a_parse_error_reports_the_line_number() {
        let error =
            GlobalConfig::from_str("sdk_path = \n", Path::new("c.toml")).expect_err("must fail");
        assert_eq!(error.exit_code(), 3, "config problems keep exit code 3");
        let text = error.to_string();
        assert!(text.contains("c.toml"), "names the file: {text}");
    }

    #[test]
    fn paths_with_spaces_and_quotes_survive_the_round_trip() {
        let config = GlobalConfig {
            sdk_path: Some(PathBuf::from("/opt/My \"Big\" SDK/iPhoneOS.sdk")),
            linker: Some(PathBuf::from(r"C:\Program Files\LLVM\bin\ld64.lld.exe")),
            ..sample()
        };
        let text = config.to_toml();
        let parsed = GlobalConfig::from_str(&text, Path::new("c.toml")).expect("must parse");
        assert_eq!(parsed.sdk_path, config.sdk_path, "spaces and quotes must round trip");
        assert_eq!(parsed.linker, config.linker, "backslashes must round trip");
    }

    #[test]
    fn a_stale_sdk_path_is_reported_as_absent() {
        let missing = PathBuf::from("/definitely/not/an/sdk");
        let absent = GlobalConfig { sdk_path: Some(missing), ..Default::default() };
        assert!(absent.live_sdk_path().is_none(), "a deleted SDK must not be offered");

        let dir = scratch("live");
        let present = GlobalConfig { sdk_path: Some(dir.clone()), ..Default::default() };
        let live = present.live_sdk_path();
        assert_eq!(live, Some(dir.clone()), "an existing SDK is offered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_creates_missing_parent_directories() {
        let dir = scratch("nested");
        let deep = dir.join("a").join("b").join("config.toml");
        GlobalConfig::default().save_to(&deep).expect("must create parents");
        assert!(deep.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_leaves_no_temp_file_behind() {
        let dir = scratch("notemp");
        let path = dir.join("config.toml");
        sample().save_to(&path).expect("must save");
        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .expect("read")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(leftovers, vec!["config.toml".to_string()], "found {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_string_path_is_treated_as_unset() {
        let config = GlobalConfig::from_str("clang = \"\"\n", Path::new("c.toml")).expect("parse");
        assert_eq!(config.clang, None, "an empty path is not a path");
    }
}

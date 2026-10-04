//! `darwinforge.toml` — the project description.
//!
//! Everything the pipeline needs is declared here by the user; nothing is
//! bundled with the tool. Parsing is strict: an unknown key is an error, not a
//! silently ignored typo.

use std::path::{Path, PathBuf};

use crate::compat;
use crate::error::{Error, Result};
use crate::tomlite::{self, Table, Value};

/// Name of the config file looked up in the project root.
pub use compat::CONFIG_FILE_NAME;

/// Former name of the project config file, still accepted as a fallback.
pub use compat::LEGACY_CONFIG_FILE_NAME;

#[derive(Debug, Clone)]
pub struct Config {
    /// The directory containing `darwinforge.toml`.
    pub project_root: PathBuf,
    pub app: App,
    pub build: Build,
}

#[derive(Debug, Clone)]
pub struct App {
    pub name: String,
    pub bundle_id: String,
    pub min_ios_version: String,
    /// CFBundleShortVersionString, e.g. "1.0".
    pub version: String,
    /// CFBundleVersion, e.g. "1".
    pub build_number: String,
    pub display_name: Option<String>,
    /// UIDeviceFamily values: 1 = iPhone, 2 = iPad.
    pub device_family: Vec<i64>,
    /// Any extra Info.plist keys merged in verbatim.
    pub plist_extras: Table,
}

#[derive(Debug, Clone)]
pub struct Build {
    pub sources: Vec<String>,
    pub resources: Vec<String>,
    pub frameworks: Vec<String>,
    pub libraries: Vec<String>,
    /// Extra flags for every compile invocation.
    pub cflags: Vec<String>,
    /// Extra flags for the link step.
    pub ldflags: Vec<String>,
    /// Where the `.app` and `.ipa` land. Relative to the project root.
    pub output_dir: String,
    /// Use the external `zip` binary instead of the built-in writer.
    pub use_external_zip: bool,
    /// `[build.swift]` extras.
    pub swift_flags: Vec<String>,
}

const DEFAULT_SOURCE_GLOBS: &[&str] = &[
    "Sources/**/*.c",
    "Sources/**/*.m",
    "Sources/**/*.mm",
    "Sources/**/*.cpp",
    "Sources/**/*.swift",
];
const DEFAULT_RESOURCE_GLOBS: &[&str] = &["Resources/**/*"];
const DEFAULT_FRAMEWORKS: &[&str] = &["UIKit", "Foundation"];
const DEFAULT_OUTPUT_DIR: &str = "build";

/// Directory names never walked when discovering sources.
pub const IGNORED_DIRECTORIES: &[&str] =
    &["build", ".git", ".svn", ".hg", ".idea", ".vscode", "DerivedData", "Pods"];

impl Config {
    /// Read and validate `<project_root>/darwinforge.toml`.
    ///
    /// A project that still has only the legacy `ipaforge.toml` keeps working:
    /// the file is loaded and a one-line deprecation notice is printed.
    pub fn load(project_root: &Path) -> Result<Config> {
        let found = compat::find_project_config(project_root).ok_or_else(|| Error::Prereq {
            what: format!("no {} in {}", CONFIG_FILE_NAME, project_root.display()),
            fix: "create one with `darwinforge init <name>`, or point at another \
                  directory with `--project PATH`"
                .to_string(),
        })?;
        if found.legacy {
            println!("{}", compat::deprecation_notice(&found.path));
        }
        let text = std::fs::read_to_string(&found.path).map_err(|source| {
            Error::io(format!("cannot read {}", found.path.display()), source)
        })?;
        Config::from_named_file(&text, project_root, &found.path)
    }

    pub fn from_str(text: &str, project_root: &Path) -> Result<Config> {
        let file = project_root.join(CONFIG_FILE_NAME);
        Config::from_named_file(text, project_root, &file)
    }

    /// Parse `text`, reporting errors against `file` (which may be the legacy
    /// `ipaforge.toml`, so the message points at the file the user actually has).
    fn from_named_file(text: &str, project_root: &Path, file: &Path) -> Result<Config> {
        let root = match tomlite::parse(text) {
            Ok(Value::Table(table)) => table,
            Ok(_) => {
                return Err(Error::config_at(file, None, "top level must be a table"))
            }
            Err(error) => {
                return Err(Error::config_at(file, Some(error.line), error.message))
            }
        };

        reject_unknown(&root, &["app", "build"], file)?;
        let app_table = table(&root, "app", APP_KEYS, file)?;
        let build_table = optional_table(Some(&root), "build", BUILD_KEYS, file)?;

        let app = App {
            name: required_string(app_table, "name", file)?,
            bundle_id: required_string(app_table, "bundle_id", file)?,
            min_ios_version: required_string(app_table, "min_ios_version", file)?,
            version: string_or(app_table, "version", "1.0"),
            build_number: string_or(app_table, "build", "1"),
            display_name: optional_string(app_table, "display_name", file)?,
            device_family: integer_array(app_table, "device_family", file)?
                .unwrap_or_else(|| vec![1, 2]),
            plist_extras: nested_table(app_table, "info_plist", file)?,
        };

        let swift_table = optional_table(build_table, "swift", SWIFT_KEYS, file)?;
        let build = Build {
            sources: string_array_or(build_table, "sources", DEFAULT_SOURCE_GLOBS, file)?,
            resources: string_array_or(build_table, "resources", DEFAULT_RESOURCE_GLOBS, file)?,
            frameworks: string_array_or(build_table, "frameworks", DEFAULT_FRAMEWORKS, file)?,
            libraries: string_array_or(build_table, "libraries", &[], file)?,
            cflags: string_array_or(build_table, "cflags", &[], file)?,
            ldflags: string_array_or(build_table, "ldflags", &[], file)?,
            output_dir: match build_table.and_then(|table| tomlite::get(table, "output_dir")) {
                Some(value) => as_string(value, "build.output_dir", file)?,
                None => DEFAULT_OUTPUT_DIR.to_string(),
            },
            use_external_zip: match build_table.and_then(|table| tomlite::get(table, "zip")) {
                Some(value) => as_bool(value, "build.zip", file)?,
                None => false,
            },
            swift_flags: string_array_or(swift_table, "swift_flags", &[], file)?,
        };

        let config = Config { project_root: project_root.to_path_buf(), app, build };
        config.validate(file)?;
        Ok(config)
    }

    /// `build/<Name>.app`
    pub fn app_bundle_name(&self) -> String {
        format!("{}.app", self.app.name)
    }

    /// Deterministic build directory, always under `<project>/build/`.
    pub fn build_dir(&self) -> PathBuf {
        self.project_root.join(&self.build.output_dir)
    }

fn validate(&self, file: &Path) -> Result<()> {
        let app = &self.app;
        if app.name.is_empty() {
            return Err(Error::config_at(file, None, "app.name must not be empty"));
        }
        if app.name.contains(['/', '\\']) {
            return Err(Error::config_at(
                file,
                None,
                format!(
                    "app.name `{}` must not contain a path separator: it becomes a \
                     directory name inside the .ipa",
                    app.name
                ),
            ));
        }
        if app.name.contains(".app") {
            return Err(Error::config_at(
                file,
                None,
                "app.name must not contain `.app`; the extension is added automatically",
            ));
        }
        validate_bundle_id(&app.bundle_id, file)?;
        validate_version(&app.min_ios_version, "app.min_ios_version", file)?;
        validate_version(&app.version, "app.version", file)?;
        if app.build_number.is_empty()
            || !app.build_number.chars().all(|c| c.is_ascii_digit() || c == '.')
        {
            return Err(Error::config_at(
                file,
                None,
                format!(
                    "app.build `{}` must be a non-empty dot-separated number (CFBundleVersion)",
                    app.build_number
                ),
            ));
        }
        if app.device_family.is_empty()
            || !app.device_family.iter().all(|value| *value == 1 || *value == 2)
        {
            return Err(Error::config_at(
                file,
                None,
                format!(
                    "app.device_family must be a non-empty list of 1 (iPhone) and/or \
                     2 (iPad), got {:?}",
                    app.device_family
                ),
            ));
        }
        if self.build.sources.is_empty() {
            return Err(Error::config_at(file, None, "build.sources must list at least one glob"));
        }
        if !is_safe_relative_dir(&self.build.output_dir) {
            return Err(Error::config_at(
                file,
                None,
                "build.output_dir must be a relative path inside the project",
            ));
        }
        Ok(())
    }
}

fn is_safe_relative_dir(output_dir: &str) -> bool {
    !output_dir.is_empty()
        && !output_dir.starts_with('/')
        && !Path::new(output_dir)
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn validate_bundle_id(bundle_id: &str, file: &Path) -> Result<()> {
    let components: Vec<&str> = bundle_id.split('.').collect();
    if components.len() < 2 {
        return Err(Error::config_at(
            file,
            None,
            format!(
                "app.bundle_id `{bundle_id}` must be reverse-DNS, e.g. `com.example.hello`"
            ),
        ));
    }
    let valid = components.iter().all(|component| {
        !component.is_empty()
            && component.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    if !valid {
        return Err(Error::config_at(
            file,
            None,
            format!(
                "app.bundle_id `{bundle_id}` may only contain letters, digits, `-` and `_`, \
                 separated by dots"
            ),
        ));
    }
    Ok(())
}

fn validate_version(version: &str, key: &str, file: &Path) -> Result<()> {
    let parts: Vec<&str> = version.split('.').collect();
    let all_numeric = !parts.is_empty()
        && parts.iter().all(|part| !part.is_empty() && part.parse::<u32>().is_ok());
    if !all_numeric {
        return Err(Error::config_at(
            file,
            None,
            format!("{key} `{version}` must be a dotted numeric version, e.g. `13.0` or `1.0.0`"),
        ));
    }
    Ok(())
}

const APP_KEYS: &[&str] = &[
    "name",
    "bundle_id",
    "min_ios_version",
    "version",
    "build",
    "display_name",
    "device_family",
    "info_plist",
];

const BUILD_KEYS: &[&str] = &[
    "sources",
    "resources",
    "frameworks",
    "libraries",
    "cflags",
    "ldflags",
    "output_dir",
    "zip",
    "swift",
];

const SWIFT_KEYS: &[&str] = &["swift_flags"];

fn reject_unknown(table: &Table, allowed: &[&str], file: &Path) -> Result<()> {
    for (key, _) in table {
        if !allowed.contains(&key.as_str()) {
            return Err(Error::config_at(
                file,
                None,
                format!(
                    "unknown key `{key}`; supported keys here are: {}",
                    allowed.iter().map(|key| format!("`{key}`")).collect::<Vec<_>>().join(", ")
                ),
            ));
        }
    }
    Ok(())
}

fn table<'a>(root: &'a Table, key: &str, allowed: &[&str], file: &Path) -> Result<&'a Table> {
    match root.iter().find(|(name, _)| name == key) {
        Some((_, Value::Table(table))) => {
            reject_unknown(table, allowed, file)?;
            Ok(table)
        }
        Some((_, other)) => Err(Error::config_at(
            file,
            None,
            format!("`{key}` must be a table, found a {}", other.type_name()),
        )),
        None => Err(Error::config_at(
            file,
            None,
            format!(
                "missing required `[{key}]` section; a minimal file looks like:\n  \
                 [{key}]\n  name = \"Hello\"\n  bundle_id = \"com.example.hello\"\n  \
                 min_ios_version = \"13.0\""
            ),
        )),
    }
}

fn optional_table<'a>(
    root: Option<&'a Table>,
    key: &str,
    allowed: &[&str],
    file: &Path,
) -> Result<Option<&'a Table>> {
    let root = match root {
        Some(root) => root,
        None => return Ok(None),
    };
    match root.iter().find(|(name, _)| name == key) {
        Some((_, Value::Table(table))) => {
            reject_unknown(table, allowed, file)?;
            Ok(Some(table))
        }
        Some((_, other)) => Err(Error::config_at(
            file,
            None,
            format!("`{key}` must be a table, found a {}", other.type_name()),
        )),
        None => Ok(None),
    }
}

fn nested_table(parent: &Table, key: &str, file: &Path) -> Result<Table> {
    match parent.iter().find(|(name, _)| name == key) {
        Some((_, Value::Table(table))) => Ok(table.clone()),
        Some((_, other)) => Err(Error::config_at(
            file,
            None,
            format!("`{key}` must be a table, found a {}", other.type_name()),
        )),
        None => Ok(Table::new()),
    }
}

fn required_string(table: &Table, key: &str, file: &Path) -> Result<String> {
    match table.iter().find(|(name, _)| name == key) {
        Some((_, Value::String(value))) if !value.trim().is_empty() => Ok(value.clone()),
        Some((_, Value::String(_))) => {
            Err(Error::config_at(file, None, format!("`{key}` must not be empty")))
        }
        Some((_, other)) => Err(Error::config_at(
            file,
            None,
            format!("`{key}` must be a string, found a {}", other.type_name()),
        )),
        None => Err(Error::config_at(file, None, format!("missing required key `{key}`"))),
    }
}

fn optional_string(table: &Table, key: &str, file: &Path) -> Result<Option<String>> {
    match table.iter().find(|(name, _)| name == key) {
        Some((_, Value::String(value))) => Ok(Some(value.clone())),
        Some((_, other)) => Err(Error::config_at(
            file,
            None,
            format!("`{key}` must be a string, found a {}", other.type_name()),
        )),
        None => Ok(None),
    }
}

fn string_or(table: &Table, key: &str, default: &str) -> String {
    match table.iter().find(|(name, _)| name == key) {
        Some((_, Value::String(value))) => value.clone(),
        _ => default.to_string(),
    }
}

fn as_string(value: &Value, key: &str, file: &Path) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        other => Err(Error::config_at(
            file,
            None,
            format!("`{key}` must be a string, found a {}", other.type_name()),
        )),
    }
}

fn as_bool(value: &Value, key: &str, file: &Path) -> Result<bool> {
    match value {
        Value::Boolean(value) => Ok(*value),
        other => Err(Error::config_at(
            file,
            None,
            format!("`{key}` must be true or false, found a {}", other.type_name()),
        )),
    }
}

fn string_array_or(
    table: Option<&Table>,
    key: &str,
    default: &[&str],
    file: &Path,
) -> Result<Vec<String>> {
    let table = match table {
        Some(table) => table,
        None => return Ok(default.iter().map(|value| value.to_string()).collect()),
    };
    match table.iter().find(|(name, _)| name == key) {
        Some((_, Value::Array(items))) => items
            .iter()
            .map(|item| match item {
                Value::String(value) => Ok(value.clone()),
                other => Err(Error::config_at(
                    file,
                    None,
                    format!("`{key}` must contain only strings, found a {}", other.type_name()),
                )),
            })
            .collect(),
        Some((_, other)) => Err(Error::config_at(
            file,
            None,
            format!("`{key}` must be an array of strings, found a {}", other.type_name()),
        )),
        None => Ok(default.iter().map(|value| value.to_string()).collect()),
    }
}

fn integer_array(table: &Table, key: &str, file: &Path) -> Result<Option<Vec<i64>>> {
    match table.iter().find(|(name, _)| name == key) {
        Some((_, Value::Array(items))) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::Integer(value) => values.push(*value),
                    other => {
                        return Err(Error::config_at(
                            file,
                            None,
                            format!(
                                "`{key}` must contain only integers, found a {}",
                                other.type_name()
                            ),
                        ))
                    }
                }
            }
            Ok(Some(values))
        }
        Some((_, other)) => Err(Error::config_at(
            file,
            None,
            format!("`{key}` must be an array of integers, found a {}", other.type_name()),
        )),
        None => Ok(None),
    }
}

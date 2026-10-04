//! Source discovery: walk the project, match the configured globs, classify
//! every file, and refuse to silently ignore things the PoC cannot build.
//!
//! Anything the PoC does not support (storyboards, `.xcassets`, `.xcdatamodeld`,
//! `.xcodeproj`, Metal, ...) produces a loud warning and is skipped. A project
//! whose *only* sources are unsupported is a hard error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{Config, IGNORED_DIRECTORIES};
use crate::error::{Error, Result};
use crate::reporter::Reporter;

/// Language of a compiled source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    C,
    ObjectiveC,
    ObjectiveCPlusPlus,
    CPlusPlus,
    Swift,
}

impl Language {
    pub fn from_path(path: &Path) -> Option<Language> {
        let extension = path.extension()?.to_string_lossy().to_lowercase();
        match extension.as_str() {
            "c" => Some(Language::C),
            "m" => Some(Language::ObjectiveC),
            "mm" => Some(Language::ObjectiveCPlusPlus),
            "cpp" | "cc" | "cxx" => Some(Language::CPlusPlus),
            "swift" => Some(Language::Swift),
            _ => None,
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Language::C => "c",
            Language::ObjectiveC => "m",
            Language::ObjectiveCPlusPlus => "mm",
            Language::CPlusPlus => "cpp",
            Language::Swift => "swift",
        }
    }

    pub fn is_swift(&self) -> bool {
        matches!(self, Language::Swift)
    }
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub language: Language,
}

#[derive(Debug, Default)]
pub struct Discovery {
    /// Compiled sources, in deterministic (sorted) order.
    pub sources: Vec<SourceFile>,
    /// Plain resources copied verbatim into the `.app`.
    pub resources: Vec<PathBuf>,
    /// Sorted unique list of unsupported things that were skipped.
    pub skipped: Vec<String>,
}

/// Why a file cannot be handled by the PoC, if it cannot.
fn unsupported_reason(path: &Path) -> Option<&'static str> {
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "storyboard" => Some("storyboard compilation needs ibtool"),
        "xib" => Some("XIB compilation needs ibtool"),
        "xcassets" => Some("asset catalogs need actool to build Assets.car"),
        "xcdatamodel" | "xcdatamodeld" => Some("Core Data models need momc"),
        "nib" => Some("compiled .nib files are not produced by darwinforge"),
        "metal" => Some("Metal shaders need metal"),
        "sks" => Some("SpriteKit scene files need skstool"),
        "framework" => Some("prebuilt .framework bundles are not supported"),
        "bundle" => Some("prebuilt .bundle resources are not supported"),
        "xcconfig" => Some(".xcconfig files are not parsed"),
        _ => {
            if file_name.ends_with(".xcassets") {
                Some("asset catalogs need actool to build Assets.car")
            } else if file_name.ends_with(".xcdatamodeld") {
                Some("Core Data models need momc")
            } else if file_name.ends_with(".xcodeproj") {
                Some(".xcodeproj parsing is out of scope; list sources in darwinforge.toml")
            } else if file_name.ends_with(".xcworkspace") {
                Some(".xcworkspace parsing is out of scope")
            } else {
                None
            }
        }
    }
}

/// True for extensions we copy into the bundle without compiling.
fn is_plain_resource(path: &Path) -> bool {
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "pdf" | "json" | "strings" | "stringsdict"
            | "txt" | "html" | "css" | "js" | "csv" | "md" | "xcstrings" | "plist" | "xml"
    )
}

/// Why a *directory* cannot be handled by the PoC, if it cannot.
///
/// Asset catalogs, Core Data models and Xcode projects are directories, so the
/// file-level check never sees them.
fn unsupported_directory_reason(name: &str) -> Option<&'static str> {
    if name.ends_with(".xcassets") {
        Some("asset catalogs need actool to build Assets.car")
    } else if name.ends_with(".xcdatamodeld") || name.ends_with(".xcdatamodel") {
        Some("Core Data models need momc")
    } else if name.ends_with(".xcodeproj") {
        Some(".xcodeproj parsing is out of scope; list sources in darwinforge.toml")
    } else if name.ends_with(".xcworkspace") {
        Some(".xcworkspace parsing is out of scope")
    } else if name.ends_with(".momd") {
        Some("compiled Core Data models need momc")
    } else {
        None
    }
}

/// Walk `directory`, collecting regular files and recording skipped directories.
///
/// `root` is the project root, used only to render skipped paths relative to it.
fn walk(
    directory: &Path,
    files: &mut Vec<PathBuf>,
    skipped: &mut BTreeMap<String, String>,
    root: &Path,
) -> Result<()> {
    let entries = std::fs::read_dir(directory)
        .map_err(|source| Error::io(format!("cannot read directory {}", directory.display()), source))?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            Error::io(format!("cannot read an entry in {}", directory.display()), source)
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = entry
            .file_type()
            .map_err(|source| Error::io(format!("cannot stat {}", path.display()), source))?;
        if file_type.is_dir() {
            if IGNORED_DIRECTORIES.contains(&name.as_str()) {
                continue;
            }
            // Some unsupported things are directories; report the directory
            // once and do not descend into it.
            if let Some(reason) = unsupported_directory_reason(&name) {
                skipped.insert(
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    reason.to_string(),
                );
                continue;
            }
            walk(&path, files, skipped, root)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

/// Match a `/`-separated relative path against a glob supporting `**`, `*` and
/// `?`. Deliberately small, but enough for `Sources/**/*.m` style patterns.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();
    match_parts(&pattern_parts, &path_parts)
}

fn match_parts(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.first() {
        None => path.is_empty(),
        // `**` matches zero or more path segments.
        Some(first) if *first == "**" => {
            (0..=path.len()).any(|skip| match_parts(&pattern[1..], &path[skip..]))
        }
        Some(first) => {
            if path.is_empty() {
                return false;
            }
            match_segment(first, path[0]) && match_parts(&pattern[1..], &path[1..])
        }
    }
}

fn match_segment(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    // Classic single-star dynamic programming; supports `*` and `?`.
    let mut table = vec![vec![false; text.len() + 1]; pattern.len() + 1];
    table[0][0] = true;
    for p in 1..=pattern.len() {
        if pattern[p - 1] == '*' {
            table[p][0] = table[p - 1][0];
        }
    }
    for p in 1..=pattern.len() {
        for t in 1..=text.len() {
            table[p][t] = match pattern[p - 1] {
                '*' => table[p - 1][t] || table[p][t - 1],
                '?' => table[p - 1][t - 1],
                literal => literal == text[t - 1] && table[p - 1][t - 1],
            };
        }
    }
    table[pattern.len()][text.len()]
}

/// Run source discovery for a build, reporting anything skipped.
pub fn discover(config: &Config, reporter: &Reporter) -> Result<Discovery> {
    let root = &config.project_root;
    let mut files = Vec::new();
    let mut directory_skips: BTreeMap<String, String> = BTreeMap::new();
    walk(root, &mut files, &mut directory_skips, root)?;
    files.sort();

    let relative = |path: &Path| -> String {
        path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
    };

    let mut sources: BTreeMap<String, SourceFile> = BTreeMap::new();
    let mut resources: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut skipped: BTreeMap<String, String> = directory_skips;

    for path in &files {
        let relative_path = relative(path);
        if relative_path == crate::config::CONFIG_FILE_NAME {
            continue;
        }
        let matched_source = config
            .build
            .sources
            .iter()
            .any(|pattern| glob_match(pattern, &relative_path));
        let matched_resource = config
            .build
            .resources
            .iter()
            .any(|pattern| glob_match(pattern, &relative_path));

        if matched_source {
            match Language::from_path(path) {
                Some(language) => {
                    sources
                        .insert(relative_path.clone(), SourceFile { path: path.clone(), language });
                    continue;
                }
                None => {
                    let reason = unsupported_reason(path).unwrap_or(
                        "matched build.sources but is not a C/ObjC/C++/Swift source file",
                    );
                    skipped.insert(relative_path.clone(), reason.to_string());
                    continue;
                }
            }
        }

        if matched_resource {
            match unsupported_reason(path) {
                Some(reason) => {
                    skipped.insert(relative_path.clone(), reason.to_string());
                }
                None if is_plain_resource(path) => {
                    resources.insert(relative_path.clone(), path.clone());
                }
                None => {
                    skipped.insert(
                        relative_path.clone(),
                        "matched build.resources but is not a plain resource file \
                         (supported: png, jpg, gif, pdf, json, strings, stringsdict, \
                         txt, html, plist, xml, ...)"
                            .to_string(),
                    );
                }
            }
            continue;
        }

        // Not referenced by any glob: still complain about file kinds the PoC
        // knows it cannot handle, so a half-supported project never looks fine.
        if let Some(reason) = unsupported_reason(path) {
            skipped.insert(relative_path.clone(), reason.to_string());
        }
    }

    for (path, reason) in &skipped {
        reporter.warn(format!("skipping {path} — {reason}"));
    }

    Ok(Discovery {
        sources: sources.into_values().collect(),
        resources: resources.into_values().collect(),
        skipped: skipped.into_iter().map(|(path, reason)| format!("{path}: {reason}")).collect(),
    })
}

/// Check that the discovered sources can actually be built.
pub fn check_buildable(discovery: &Discovery) -> Result<()> {
    if discovery.sources.is_empty() {
        return Err(Error::Prereq {
            what: "no compilable sources found".to_string(),
            fix: "check build.sources in darwinforge.toml; the default globs look under \
                  Sources/ for .c, .m, .mm, .cpp and .swift"
                .to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_double_star_crosses_directories() {
        assert!(glob_match("Sources/**/*.m", "Sources/main.m"));
        assert!(glob_match("Sources/**/*.m", "Sources/ui/view.m"));
        assert!(glob_match("Sources/**/*.m", "Sources/ui/deep/view.m"));
        assert!(!glob_match("Sources/**/*.m", "Sources/main.cpp"));
        assert!(!glob_match("Sources/**/*.m", "Other/main.m"));
    }

    #[test]
    fn glob_single_star_stays_in_one_segment() {
        assert!(glob_match("Resources/*.png", "Resources/logo.png"));
        assert!(!glob_match("Resources/*.png", "Resources/ui/logo.png"));
    }

    #[test]
    fn glob_resource_wildcard_matches_nested() {
        assert!(glob_match("Resources/**/*", "Resources/data/config.json"));
        assert!(glob_match("Resources/**/*", "Resources/logo.png"));
    }

    #[test]
    fn glob_question_mark() {
        assert!(glob_match("Sources/v?.m", "Sources/v1.m"));
        assert!(!glob_match("Sources/v?.m", "Sources/v10.m"));
    }

    #[test]
    fn language_detection() {
        assert_eq!(Language::from_path(Path::new("a/b.m")), Some(Language::ObjectiveC));
        assert_eq!(Language::from_path(Path::new("a/b.mm")), Some(Language::ObjectiveCPlusPlus));
        assert_eq!(Language::from_path(Path::new("a/b.cpp")), Some(Language::CPlusPlus));
        assert_eq!(Language::from_path(Path::new("a/b.swift")), Some(Language::Swift));
        assert_eq!(Language::from_path(Path::new("a/b.h")), None);
    }

    #[test]
    fn unsupported_detection() {
        assert!(unsupported_reason(Path::new("Base.lproj/Main.storyboard")).is_some());
        assert!(unsupported_reason(Path::new("Images.xcassets")).is_some());
        assert!(unsupported_reason(Path::new("Model.xcdatamodeld")).is_some());
        assert!(unsupported_reason(Path::new("App.xcodeproj")).is_some());
        assert!(unsupported_reason(Path::new("Shader.metal")).is_some());
        assert!(unsupported_reason(Path::new("main.m")).is_none());
        assert!(unsupported_reason(Path::new("logo.png")).is_none());
    }

    #[test]
    fn unsupported_directory_detection() {
        assert!(unsupported_directory_reason("Images.xcassets").is_some());
        assert!(unsupported_directory_reason("Model.xcdatamodeld").is_some());
        assert!(unsupported_directory_reason("App.xcodeproj").is_some());
        assert!(unsupported_directory_reason("en.lproj").is_none());
        assert!(unsupported_directory_reason("Sources").is_none());
    }

    #[test]
    fn plain_resources_recognised() {
        assert!(is_plain_resource(Path::new("Resources/logo.png")));
        assert!(is_plain_resource(Path::new("Resources/Localizable.strings")));
        assert!(is_plain_resource(Path::new("Resources/data.json")));
        assert!(!is_plain_resource(Path::new("Resources/main.m")));
    }
}
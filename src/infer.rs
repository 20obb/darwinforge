//! Zero-configuration inference: deriving a whole project description from
//! nothing but a directory.
//!
//! The first-run experience the tool aims for is `darwinforge build ~/MyApp` and
//! a built `.ipa`, with no config file to write and nothing to read first. This
//! module is what makes that possible: it takes a path and returns an
//! [`Inference`], which [`to_config`] turns into exactly the `Config` the
//! pipeline would have loaded from `darwinforge.toml`.
//!
//! Every inference is reported back as a line of prose ([`Inference::describe`]),
//! so nothing is ever guessed *silently* — the user is told what was assumed, and
//! `darwinforge init --here` writes the same values to a file, at which point
//! nothing is being inferred at all.
//!
//! The rules, all deliberately conservative:
//!
//! | What | How |
//! | --- | --- |
//! | name | the last path component of the directory, sanitized |
//! | bundle id | `com.example.<name>`, sanitized — the same rule as `init` |
//! | sources | every `.c`/`.m`/`.mm`/`.cpp`/`.swift` outside skipped directories |
//! | frameworks | `#import <X/…>` and `@import X;` |
//! | min iOS | the resolved SDK's version, else a fixed floor |
//!
//! Framework inference reads what the *source asks for*, not a hardcoded list.
//! A file that imports `CoreGraphics/CoreGraphics.h` is asking for
//! `CoreGraphics` — a fact about the file, not an opinion about iOS.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config::{Config, IGNORED_DIRECTORIES};
use crate::discovery::Language;
use crate::error::{Error, Result};
use crate::scaffold;

/// Deployment target used when no SDK version could be read.
///
/// A fixed floor rather than "whatever the newest SDK claims": this value goes
/// into `-target arm64-apple-iosX.Y`, so an SDK-independent default means a
/// project built on one machine targets the same version on every other.
const DEFAULT_MIN_IOS: &str = "13.0";

/// Directories never walked when scanning for sources.
///
/// [`IGNORED_DIRECTORIES`] covers the general cases (`build`, `.git`, `Pods`);
/// these are the ones that appear specifically when a user points the tool at an
/// arbitrary directory with no config file.
const EXTRA_SKIP: &[&str] = &["target", "node_modules", "Carthage", ".venv", "venv"];

/// What zero-config mode worked out, and where each answer came from.
#[derive(Debug, Clone)]
pub struct Inference {
    /// The directory the description was derived from.
    pub root: PathBuf,
    /// App name; the last path component, sanitized.
    pub name: String,
    /// `com.example.<name>` style reverse-DNS id.
    pub bundle_id: String,
    /// Exact source paths, sorted, one entry per file.
    pub source_paths: Vec<PathBuf>,
    /// Frameworks implied by the sources' own imports.
    pub frameworks: Vec<String>,
    /// How many source files were actually read for imports.
    pub scanned_files: usize,
    /// Deployment target, taken from the SDK when one could be read.
    pub min_ios_version: String,
    /// One phrase saying where `min_ios_version` came from.
    pub min_ios_origin: String,
}

impl Inference {
    /// One line per inference, printed before a zero-config build.
    ///
    /// Short on purpose: this appears on every run until a config file exists,
    /// so a wall of text here would undo the point of a quiet default.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "inferred: name = \"{}\", bundle_id = \"{}\" (no darwinforge.toml)",
            self.name, self.bundle_id
        )];
        lines.push(format!(
            "  {} source file(s), {} framework(s), min iOS {} ({})",
            self.source_paths.len(),
            self.frameworks.len(),
            self.min_ios_version,
            self.min_ios_origin,
        ));
        lines.push(
            "  run `darwinforge init --here` to write this to darwinforge.toml and stop \
             inferring it"
                .to_string(),
        );
        lines
    }

    /// The frameworks joined for a config line.
    pub fn frameworks_display(&self) -> String {
        self.frameworks
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Derive a project description from `root`.
///
/// `sdk_version` is passed in because only the caller knows which SDK is in use;
/// `None` means no SDK was resolvable yet and [`DEFAULT_MIN_IOS`] is used
/// instead, which [`Inference::min_ios_origin`] states either way.
pub fn infer(root: &Path, sdk_version: Option<&str>) -> Result<Inference> {
    let root = normalise_root(root)?;
    let name = scaffold::derive_app_name(&root.to_string_lossy());
    let bundle_id = scaffold::default_bundle_id(&name);

    let sources = scan_sources(&root);
    if sources.is_empty() {
        return Err(Error::Prereq {
            what: format!("no compilable sources found in {}", root.display()),
            fix: "add a .m, .c, .mm or .cpp file, or write a darwinforge.toml naming them \
                  (`darwinforge init --here`)"
                .to_string(),
        });
    }

    let (frameworks, scanned) = infer_frameworks(&sources);
    let (min_ios_version, min_ios_origin) = match sdk_version {
        Some(version) => {
            // Never target above the SDK: clang accepts it, but the binary
            // would claim a floor higher than the SDK it was built against.
            (deployment_target(version), format!("the SDK's own version {version}"))
        }
        None => (DEFAULT_MIN_IOS.to_string(), "a fixed default, no SDK readable".to_string()),
    };

    Ok(Inference {
        root,
        name,
        bundle_id,
        source_paths: sources,
        frameworks,
        scanned_files: scanned,
        min_ios_version,
        min_ios_origin,
    })
}

/// Resolve `root` to an existing directory.
///
/// A path that is a *file* is turned into its parent, so `darwinforge build
/// ~/myapp/main.m` and `darwinforge build ~/myapp` describe the same project
/// rather than two different ones.
fn normalise_root(root: &Path) -> Result<PathBuf> {
    if root.is_dir() {
        return Ok(root.to_path_buf());
    }
    if root.is_file() {
        if let Some(parent) = root.parent() {
            if !parent.as_os_str().is_empty() && parent.is_dir() {
                return Ok(parent.to_path_buf());
            }
        }
    }
    Err(Error::Prereq {
        what: format!("{} does not exist", root.display()),
        fix: "pass a project directory, a darwinforge.toml, or a single .m/.c file"
            .to_string(),
    })
}

/// Turn an SDK version into a `major.minor` deployment target.
///
/// `17.5` → `17.5` and `17` → `17.0`, through the same function the pipeline
/// uses, so inference and compilation cannot disagree. Anything unparsable falls
/// back to [`DEFAULT_MIN_IOS`] rather than handing clang a malformed version.
fn deployment_target(sdk_version: &str) -> String {
    match crate::sdksource::SdkVersion::parse(sdk_version) {
        Some(version) if !version.parts.is_empty() => {
            crate::compile::normalize_ios_version(&version.as_string())
        }
        _ => DEFAULT_MIN_IOS.to_string(),
    }
}


/// Walk `root` and collect every compilable source.
///
/// Sorted by path so two runs over the same tree always compile in the same
/// order — deterministic output depends on a stable input order, since the
/// filesystem does not promise one.
pub fn scan_sources(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if IGNORED_DIRECTORIES.contains(&name.as_str())
                    || EXTRA_SKIP.contains(&name.as_str())
                    || name.starts_with('.')
                {
                    continue;
                }
                stack.push(path);
            } else if Language::from_path(&path).is_some() {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Frameworks the sources ask for, read from their own import statements.
///
/// Returns the sorted unique framework names and the number of files read. A
/// file that cannot be read is skipped rather than fatal: no build has started
/// yet, and refusing to infer because one file is unreadable would be a poor
/// first impression.
pub fn infer_frameworks(sources: &[PathBuf]) -> (Vec<String>, usize) {
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut scanned = 0usize;
    for source in sources {
        let Ok(text) = std::fs::read_to_string(source) else { continue };
        scanned += 1;
        found.extend(frameworks_in(&text));
    }
    (found.into_iter().collect(), scanned)
}

/// Extract framework names from the text of one source file.
///
/// Pure, so the rules are testable without touching a filesystem.
///
/// ```text
/// #import <UIKit/UIKit.h>         -> UIKit
/// #import <CoreGraphics/CGBase.h> -> CoreGraphics
/// #import <stdio.h>               -> ignored (a header, not a framework)
/// #import "MyHeader.h"            -> ignored (quoted form names a local file)
/// @import SwiftUI;                -> SwiftUI
/// ```
///
/// Only the first path component is taken, so a nested include never becomes a
/// framework called `A/B`.
pub fn frameworks_in(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let candidate = if let Some(rest) = line.strip_prefix("#import") {
            let rest = rest.trim_start();
            rest.strip_prefix('<')
                .and_then(|r| r.split('>').next())
                .and_then(angle_framework)
        } else if let Some(rest) = line.strip_prefix("@import") {
            // `@import UIKit;` and `@import UIKit.Swift;` both name UIKit.
            let rest = rest.trim_start().trim_end_matches(';').trim();
            let name = rest.split('.').next().unwrap_or_default().trim();
            is_framework_name(name).then(|| name.to_string())
        } else {
            None
        };
        if let Some(name) = candidate {
            if !found.contains(&name) {
                found.push(name);
            }
        }
    }
    found
}

/// The framework an `#import <...>` target names, if it names one.
///
/// Requires exactly `Name/Header.h`: `UIKit/UIKit.h` is a framework import, but
/// `<stdio.h>` has no slash and `<A/B/C.h>` is a nested path — neither names a
/// framework.
fn angle_framework(target: &str) -> Option<String> {
    let (name, rest) = target.split_once('/')?;
    if rest.contains('/') {
        // Two components after the name: a subdirectory, not a framework.
        return None;
    }
    is_framework_name(name).then(|| name.to_string())
}

/// True when `name` looks like a framework rather than a path or a header.
///
/// Frameworks are alphanumeric with underscores — `UIKit`, `CoreGraphics`,
/// `SwiftUI` — so `stdio.h` (which contains a dot) and `../foo` are rejected.
fn is_framework_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}


/// Render an [`Inference`] as TOML.
///
/// Public because it is exactly what `init --here` writes: the file a user ends
/// up with and the configuration the tool inferred must be the same thing, so
/// they are produced by one function rather than two that could drift.
pub fn render(inference: &Inference) -> String {
    let sources = inference
        .source_paths
        .iter()
        .map(|path| {
            let relative = path.strip_prefix(&inference.root).unwrap_or(path);
            format!("\"{}\"", relative.to_string_lossy().replace('\\', "/"))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"# Written by `darwinforge init --here` — inferred once, never inferred again.
# Edit freely: from now on darwinforge reads this file and stops guessing.

[app]
name = "{name}"
bundle_id = "{bundle_id}"
min_ios_version = "{min_ios_version}"
version = "1.0"
build = "1"
device_family = [1, 2]

[build]
sources = [{sources}]
resources = ["Resources/**/*"]
frameworks = [{frameworks}]
"#,
        name = inference.name,
        bundle_id = inference.bundle_id,
        min_ios_version = inference.min_ios_version,
        frameworks = inference.frameworks_display(),
    )
}

/// Turn an [`Inference`] into a config the pipeline accepts.
///
/// The globs produced here are literal relative paths with no wildcards, so
/// they are valid patterns by construction and the two representations cannot
/// drift apart.
pub fn to_config(inference: &Inference) -> Result<Config> {
    let text = render(inference);
    Config::from_str(&text, &inference.root).map_err(|error| match error {
        // `Config` errors point at a file that does not exist in zero-config
        // mode, so the message is reframed around the directory instead.
        Error::Config { message, .. } => Error::Prereq {
            what: format!("zero-config inference produced an unusable description: {message}"),
            fix: "run `darwinforge init --here` to write a config you can edit, or add a \
                  darwinforge.toml by hand"
                .to_string(),
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("darwinforge-infer-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write(root: &Path, relative: &str, contents: &str) -> PathBuf {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, contents).expect("write");
        path
    }

    /// A sample directory shaped like a real project a user might point at.
    fn sample(tag: &str) -> PathBuf {
        let root = scratch(tag).join(format!("sample-{tag}"));
        std::fs::create_dir_all(&root).expect("temp project");
        write(
            &root,
            "main.m",
            "#import <UIKit/UIKit.h>\n#import <CoreGraphics/CGBase.h>\n\
             @import Foundation;\n#include <stdio.h>\nint main(void){return 0;}\n",
        );
        write(&root, "Helpers/util.c", "int helper(void) { return 1; }\n");
        write(&root, "README.md", "not a source\n");
        // Must not be walked: ignored by name, and by leading dot.
        write(&root, "build/stale.m", "int stale(void) { return 0; }\n");
        write(&root, ".hidden/secret.m", "int secret(void) { return 0; }\n");
        root
    }

    #[test]
    fn the_sample_directory_infers_every_field() {
        let root = sample("fields");
        let inferred = infer(&root, Some("17.5")).expect("must infer");

        assert_eq!(inferred.name, "sample-fields", "named after the directory");
        assert_eq!(inferred.bundle_id, "com.example.sample-fields");
        assert!(
            inferred
                .bundle_id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'),
            "bundle id is [a-z0-9.-]: {}",
            inferred.bundle_id
        );

        let sources: Vec<String> = inferred
            .source_paths
            .iter()
            .map(|path| path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            sources,
            ["Helpers/util.c", "main.m"],
            "both sources found and sorted; build/ and the hidden dir skipped, README ignored"
        );

        assert_eq!(inferred.frameworks, ["CoreGraphics", "Foundation", "UIKit"]);
        assert_eq!(inferred.scanned_files, 2, "both sources were read");
        assert_eq!(inferred.min_ios_version, "17.5");
        assert_eq!(inferred.min_ios_origin, "the SDK's own version 17.5");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_sdk_falls_back_to_a_fixed_floor_and_says_so() {
        let root = sample("nosdk");
        let inferred = infer(&root, None).expect("must infer");
        assert_eq!(inferred.min_ios_version, "13.0");
        assert!(
            inferred.min_ios_origin.contains("no SDK"),
            "the origin must be honest about why: {}",
            inferred.min_ios_origin
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_sdk_version_becomes_a_major_minor_deployment_target() {
        // `17` must not reach clang as `-target arm64-apple-ios17`.
        assert_eq!(deployment_target("17"), "17.0");
        assert_eq!(deployment_target("17.5"), "17.5");
        assert_eq!(deployment_target("12.1.2"), "12.1", "the patch component is dropped");
        // Anything unparsable falls back rather than being passed through.
        assert_eq!(deployment_target("latest"), "13.0");
        assert_eq!(deployment_target(""), "13.0");
    }

    #[test]
    fn pointing_at_a_single_source_file_builds_its_directory() {
        // `darwinforge build ~/MyApp/main.m` and `darwinforge build ~/MyApp`
        // must describe the same project rather than two different ones.
        let root = sample("singlefile");
        let file = root.join("main.m");
        let inferred = infer(&file, None).expect("must infer");
        assert_eq!(inferred.root, root, "the parent directory is the project");
        assert_eq!(inferred.name, "sample-singlefile");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_with_no_sources_explains_what_to_add() {
        let root = sample("empty");
        std::fs::remove_file(root.join("main.m")).expect("remove");
        std::fs::remove_file(root.join("Helpers/util.c")).expect("remove");
        let error = infer(&root, None).expect_err("must fail");
        let text = error.to_string();
        assert!(text.contains("no compilable sources"), "{text}");
        assert!(text.contains("init --here"), "offers the fix: {text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_path_that_does_not_exist_is_reported_with_the_three_accepted_shapes() {
        let error = infer(Path::new("/definitely/not/here/12345"), None).expect_err("must fail");
        let text = error.to_string();
        assert!(text.contains("does not exist"), "{text}");
        assert!(text.contains("darwinforge.toml"), "names the accepted inputs: {text}");
    }


    // ---- framework inference, from the source's own imports -----------------

    #[test]
    fn angle_imports_yield_the_framework_component_only() {
        assert_eq!(frameworks_in("#import <UIKit/UIKit.h>"), ["UIKit"]);
        assert_eq!(frameworks_in("#import <CoreGraphics/CGBase.h>"), ["CoreGraphics"]);
        // No slash at all: a header, not a framework.
        assert_eq!(frameworks_in("#import <stdio.h>"), [] as [&str; 0]);
        // More than two components: a nested path, not a framework.
        assert_eq!(frameworks_in("#import <A/B/C.h>"), [] as [&str; 0]);
        // The quoted form names a local header by definition.
        assert_eq!(frameworks_in("#import \"MyHeader.h\""), [] as [&str; 0]);
        // `#include` is the C spelling, and is not treated as a framework ask.
        assert_eq!(frameworks_in("#include <UIKit/UIKit.h>"), [] as [&str; 0]);
    }

    #[test]
    fn module_imports_yield_the_framework() {
        assert_eq!(frameworks_in("@import UIKit;"), ["UIKit"]);
        assert_eq!(frameworks_in("@import CoreGraphics;"), ["CoreGraphics"]);
        // A module qualifier still names the framework itself.
        assert_eq!(frameworks_in("@import UIKit.Swift;"), ["UIKit"]);
        assert_eq!(frameworks_in("@import  Foundation ;"), ["Foundation"]);
        assert_eq!(frameworks_in("@import;"), [] as [&str; 0]);
    }

    #[test]
    fn duplicates_collapse_in_first_seen_order() {
        let text = "#import <UIKit/UIKit.h>\n@import Foundation;\n#import <UIKit/UIView.h>\n";
        assert_eq!(frameworks_in(text), ["UIKit", "Foundation"]);
    }

    #[test]
    fn non_framework_lookalikes_are_rejected() {
        // The rules are conservative: a name must be alphanumeric to count.
        assert!(!is_framework_name(""));
        assert!(!is_framework_name("stdio.h"));
        assert!(!is_framework_name("../foo"));
        assert!(!is_framework_name("a/b"));
        assert!(is_framework_name("UIKit"));
        assert!(is_framework_name("CoreGraphics"));
        assert!(is_framework_name("_Private"));
    }

    #[test]
    fn text_with_no_imports_yields_nothing() {
        assert!(frameworks_in("int main(void){return 0;}").is_empty());
        assert!(frameworks_in("").is_empty());
        assert!(frameworks_in("\n\n\n").is_empty());
    }

    #[test]
    fn scanning_a_directory_only_reads_real_sources() {
        // A README must not be opened, and a source in an ignored directory
        // must not be counted — otherwise `scanned_files` would overstate how
        // much of the project was actually looked at.
        let root = sample("scanned");
        let sources = scan_sources(&root);
        assert_eq!(sources.len(), 2, "{sources:?}");
        let (_, scanned) = infer_frameworks(&sources);
        assert_eq!(scanned, 2, "exactly the two sources were read");
        let _ = std::fs::remove_dir_all(&root);
    }


    // ---- render, and the round trip that keeps `init --here` honest ---------

    #[test]
    fn the_rendered_config_parses_back_into_the_same_description() {
        // `init --here` writes this text and a later build reads it back, so
        // the two must agree; a round trip is the only way to guarantee it.
        let root = sample("roundtrip");
        let inferred = infer(&root, Some("17.5")).expect("infer");
        let config = to_config(&inferred).expect("rendered config must parse");
        assert_eq!(config.app.name, inferred.name);
        assert_eq!(config.app.bundle_id, inferred.bundle_id);
        assert_eq!(config.app.min_ios_version, inferred.min_ios_version);
        assert_eq!(config.build.frameworks, inferred.frameworks);
        assert_eq!(
            config.build.sources,
            ["Helpers/util.c", "main.m"],
            "literal relative paths, one per source"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_description_names_the_command_that_stops_the_inference() {
        // The escape hatch must appear in *every* zero-config run, or the user
        // has no way to learn there is one.
        let root = sample("describe");
        let inferred = infer(&root, Some("17.5")).expect("infer");
        let lines = inferred.describe();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("inferred:"), "{}", lines[0]);
        assert!(lines[0].contains(&inferred.name), "{}", lines[0]);
        assert!(lines[1].contains("framework(s)"), "{}", lines[1]);
        assert!(lines[2].contains("init --here"), "{}", lines[2]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rendered_paths_always_use_forward_slashes() {
        // Discovery yields backslashes on Windows; a glob with them would never
        // match on any platform, so the config must be normalised on the way out.
        let root = sample("slashes");
        let inferred = infer(&root, None).expect("infer");
        let text = render(&inferred);
        assert!(
            !text.contains('\\'),
            "no backslashes may reach the config, whatever the host OS:\n{text}"
        );
        assert!(text.contains("\"Helpers/util.c\""), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_rendered_config_carries_the_file_a_reader_would_need() {
        // Sanity: what `init --here` writes must contain every key `Config`
        // requires, or the file is unusable the moment it exists.
        let root = sample("complete");
        let inferred = infer(&root, Some("17.5")).expect("infer");
        let text = render(&inferred);
        for key in ["[app]", "name =", "bundle_id =", "min_ios_version =", "[build]", "sources ="] {
            assert!(text.contains(key), "the rendered config is missing `{key}`:\n{text}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

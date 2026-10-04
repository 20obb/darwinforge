//! Stage 1: compile each source to an object file with clang.
//!
//! All flag construction lives in pure functions so the exact command line can
//! be asserted in tests without a compiler present. Swift is handled only if a
//! usable `swiftc` was detected; otherwise the build fails with a clear
//! "not supported yet" message.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::discovery::{Language, SourceFile};
use crate::error::{Error, Result};
use crate::exec;
use crate::reporter::Reporter;
use crate::sdk::Sdk;

/// The `-target` triple, e.g. `arm64-apple-ios13.0`.
pub fn target_triple(arch: &str, min_ios_version: &str) -> String {
    format!("{arch}-apple-ios{}", normalize_ios_version(min_ios_version))
}

/// iOS wants at least a major.minor version for the deployment target.
pub fn normalize_ios_version(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or("13");
    let minor = parts.next().unwrap_or("0");
    format!("{major}.{minor}")
}

/// Flags that depend on the source language.
fn language_flags(language: Language) -> Vec<String> {
    match language {
        Language::C => vec!["-std=gnu11".to_string()],
        Language::ObjectiveC => vec!["-fobjc-runtime=ios-13.0".to_string()],
        Language::ObjectiveCPlusPlus | Language::CPlusPlus => {
            vec!["-std=gnu++17".to_string()]
        }
        Language::Swift => Vec::new(),
    }
}

/// Build the full clang argument list for one source file.
pub fn clang_arguments(
    config: &Config,
    sdk: &Sdk,
    source: &SourceFile,
    object: &Path,
    arch: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-c".to_string(),
        source.path.to_string_lossy().to_string(),
        "-o".to_string(),
        object.to_string_lossy().to_string(),
        "-target".to_string(),
        target_triple(arch, &config.app.min_ios_version),
        "-isysroot".to_string(),
        sdk.root.to_string_lossy().to_string(),
        "-arch".to_string(),
        arch.to_string(),
    ];
    args.extend(language_flags(source.language));
    args.extend(config.build.cflags.iter().cloned());
    args
}

fn swiftc_arguments(
    config: &Config,
    sdk: &Sdk,
    source: &Path,
    object: &Path,
    arch: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-emit-object".to_string(),
        "-c".to_string(),
        source.to_string_lossy().to_string(),
        "-o".to_string(),
        object.to_string_lossy().to_string(),
        "-target".to_string(),
        target_triple(arch, &config.app.min_ios_version),
        "-sdk".to_string(),
        sdk.root.to_string_lossy().to_string(),
    ];
    args.extend(config.build.swift_flags.iter().cloned());
    args
}

/// Compile every source; returns the object files in deterministic order.
// The pipeline stages pass config + sdk + binaries + inputs straight through;
// grouping them into a context struct would only move the same fields around.
#[allow(clippy::too_many_arguments)]
pub fn compile_all(
    config: &Config,
    sdk: &Sdk,
    clang: &Path,
    swiftc: Option<&Path>,
    sources: &[SourceFile],
    object_dir: &Path,
    arch: &str,
    reporter: &Reporter,
) -> Result<Vec<PathBuf>> {
    let uses_swift = sources.iter().any(|source| source.language.is_swift());
    let swiftc = match (uses_swift, swiftc) {
        (true, Some(path)) => Some(path.to_path_buf()),
        (true, None) => {
            return Err(Error::Unsupported {
                message: "this project contains Swift sources, but no Swift toolchain \
                          for Darwin was found.\n  \
                          darwinforge needs a `swiftc` that can target arm64-apple-ios plus \
                          an SDK that ships the matching Swift modules.\n  \
                          Fix: install a Swift toolchain and put `swiftc` on PATH, or set \
                          DARWINFORGE_SWIFTC=/path/to/swiftc.\n  \
                          Swift is optional in this PoC: a project with only C/ObjC/C++ \
                          sources builds fine without it."
                    .to_string(),
            })
        }
        (false, _) => None,
    };

    std::fs::create_dir_all(object_dir)
        .map_err(|source| Error::io(format!("cannot create {}", object_dir.display()), source))?;

    let mut objects = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        // Object names are derived from the position in the sorted list, so the
        // layout under ./build/ is stable across machines.
        let stem = source
            .path
            .file_stem()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("source{index}"));
        let object = object_dir.join(format!("{index:03}-{stem}.o"));
        let program = if source.language.is_swift() {
            swiftc.as_deref().expect("swiftc resolved above")
        } else {
            clang
        };
        let args = if source.language.is_swift() {
            swiftc_arguments(config, sdk, &source.path, &object, arch)
        } else {
            clang_arguments(config, sdk, source, &object, arch)
        };
        reporter.info(&format!(
            "  {} [{}]",
            relative_name(&source.path, config),
            source.language.extension()
        ));
        exec::run(&program.to_string_lossy(), &args, None, reporter)?;
        if !object.is_file() {
            return Err(Error::format(
                "compile",
                format!(
                    "{} reported success but did not create {}",
                    program.display(),
                    object.display()
                ),
            ));
        }
        objects.push(object);
    }
    Ok(objects)
}

fn relative_name(path: &Path, config: &Config) -> String {
    path.strip_prefix(&config.project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let text = r#"
            [app]
            name = "Hello"
            bundle_id = "com.example.hello"
            min_ios_version = "13.0"

            [build]
            frameworks = ["UIKit"]
            cflags = ["-fobjc-arc"]
        "#;
        Config::from_str(text, Path::new("/project")).expect("valid config")
    }

    fn sdk() -> Sdk {
        Sdk { root: PathBuf::from("/sdk"), version: Some("17.4".to_string()) }
    }

    fn source(name: &str) -> SourceFile {
        SourceFile {
            path: Path::new("/project").join(name),
            language: Language::from_path(Path::new(name)).expect("known extension"),
        }
    }

    fn args_for(name: &str) -> Vec<String> {
        let config = config();
        clang_arguments(
            &config,
            &sdk(),
            &source(name),
            Path::new("/project/build/main.o"),
            "arm64",
        )
    }

    #[test]
    fn target_triple_uses_min_ios_version() {
        assert_eq!(target_triple("arm64", "13.0"), "arm64-apple-ios13.0");
        assert_eq!(target_triple("arm64", "16.4"), "arm64-apple-ios16.4");
    }

    #[test]
    fn clang_command_has_sdk_and_target() {
        let args = args_for("Sources/main.m");
        let expected_source = Path::new("/project").join("Sources/main.m");
        assert_eq!(&args[0..2], ["-c", &expected_source.to_string_lossy()]);
        assert!(args.windows(2).any(|pair| pair == ["-target", "arm64-apple-ios13.0"]));
        assert!(args.windows(2).any(|pair| pair == ["-isysroot", "/sdk"]));
        assert!(args.windows(2).any(|pair| pair == ["-arch", "arm64"]));
        assert!(args.contains(&"-fobjc-arc".to_string()), "user cflags are appended");
        assert!(args.contains(&"-fobjc-runtime=ios-13.0".to_string()));
    }

    #[test]
    fn language_specific_flags() {
        assert!(args_for("Sources/a.c").contains(&"-std=gnu11".to_string()));
        assert!(args_for("Sources/a.mm").contains(&"-std=gnu++17".to_string()));
        assert!(args_for("Sources/a.cpp").contains(&"-std=gnu++17".to_string()));
        assert!(!args_for("Sources/a.m").contains(&"-std=gnu++17".to_string()));
    }

    #[test]
    fn swift_arguments_target_darwin_sdk() {
        let config = config();
        let args = swiftc_arguments(
            &config,
            &sdk(),
            Path::new("/project/Sources/main.swift"),
            Path::new("/project/build/main.o"),
            "arm64",
        );
        assert!(args.windows(2).any(|pair| pair == ["-target", "arm64-apple-ios13.0"]));
        assert!(args.windows(2).any(|pair| pair == ["-sdk", "/sdk"]));
        assert!(args.contains(&"-emit-object".to_string()));
    }

    #[test]
    fn version_normalisation_drops_patch() {
        assert_eq!(normalize_ios_version("13"), "13.0");
        assert_eq!(normalize_ios_version("15.4.1"), "15.4");
    }
}
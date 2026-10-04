//! Stage 2: link the objects into a Mach-O executable.
//!
//! Supports LLVM's `ld64.lld` and cctools-port's `ld`. The argument list is a
//! pure function so tests can assert the framework/search-path wiring without a
//! linker installed.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::exec;
use crate::reporter::Reporter;
use crate::sdk::{LinkerKind, Sdk};

/// One library darwinforge always links, and the reason it is needed.
struct ImplicitLibrary {
    name: &'static str,
    /// What goes undefined without it.
    reason: &'static str,
}

/// Libraries linked by default because nothing else pulls them in.
///
/// `libobjc` carries the Objective-C runtime (`objc_msgSend`, class
/// registration); `libSystem` carries the stack-protector helpers
/// (`___stack_chk_fail`, `___stack_chk_guard`) that clang emits for every
/// translation unit compiled with stack protection, which is the default.
const IMPLICIT_LIBRARIES: &[ImplicitLibrary] = &[
    ImplicitLibrary {
        name: "System",
        reason: "___stack_chk_fail / ___stack_chk_guard are undefined without it",
    },
    ImplicitLibrary {
        name: "objc",
        reason: "the Objective-C runtime (objc_msgSend) is undefined without it",
    },
];

/// Build the link command line.
pub fn link_arguments(
    config: &Config,
    sdk: &Sdk,
    kind: LinkerKind,
    objects: &[PathBuf],
    output: &Path,
    arch: &str,
) -> Vec<String> {
    let min_version = crate::compile::normalize_ios_version(&config.app.min_ios_version);
    let mut args: Vec<String> = vec![
        "-arch".to_string(),
        arch.to_string(),
        "-platform_version".to_string(),
        "ios".to_string(),
        min_version.clone(),
        // If the SDK does not declare a version, fall back to the deployment
        // target; ld only uses this to stamp LC_BUILD_VERSION.
        sdk.version.clone().unwrap_or_else(|| min_version.clone()),
        "-o".to_string(),
        output.to_string_lossy().to_string(),
        // `-syslibroot` lets ld find the `.tbd` stubs under <SDK>/usr/lib.
        "-syslibroot".to_string(),
        sdk.root.to_string_lossy().to_string(),
        "-L".to_string(),
        sdk.library_search_path().to_string_lossy().to_string(),
        "-F".to_string(),
        sdk.framework_search_path().to_string_lossy().to_string(),
    ];

    args.extend(objects.iter().map(|path| path.to_string_lossy().to_string()));

    for framework in &config.build.frameworks {
        args.push("-framework".to_string());
        args.push(framework.clone());
    }

    // The implicit libraries every iOS executable needs.
    //
    // clang emits calls to `___stack_chk_fail`/`___stack_chk_guard` (the
    // stack-protector symbols) and the Objective-C runtime entry points, none of
    // which any declared framework provides. Without these two `-l` flags the
    // link fails with "undefined symbol" on any real SDK. They are added unless
    // the user already listed them in `build.libraries`, so an explicit choice is
    // never duplicated or reordered.
    for library in IMPLICIT_LIBRARIES {
        let name = library.name;
        if config.build.libraries.iter().any(|listed| listed == name) {
            continue;
        }
        args.push(format!("-l{name}"));
    }

    for library in &config.build.libraries {
        args.push(format!("-l{library}"));
    }

    // ld refuses to sign a binary twice; darwinforge signs later with ldid.
    args.push("-no_data_in_code_info".to_string());
    args.push("-e".to_string());
    args.push("_main".to_string());
    let _ = kind; // both lld and cctools `ld` accept the flags above

    args.extend(config.build.ldflags.iter().cloned());
    args
}

/// Verify every declared framework actually exists in the given SDK.
pub fn check_frameworks(config: &Config, sdk: &Sdk) -> Result<()> {
    let missing: Vec<&str> = config
        .build
        .frameworks
        .iter()
        .map(String::as_str)
        .filter(|framework| !sdk.has_framework(framework))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let hint = missing
        .iter()
        .map(|framework| {
            if *framework == "UIKit" {
                "UIKit is missing — that usually means the SDK is a simulator SDK or an \
                 incomplete extract; darwinforge needs a device iPhoneOS SDK"
                    .to_string()
            } else {
                format!("{framework} is not in {}", sdk.framework_search_path().display())
            }
        })
        .collect::<Vec<_>>()
        .join("\n  ");
    Err(Error::Prereq {
        what: format!(
            "{} not found in the SDK: {}",
            if missing.len() == 1 { "framework is" } else { "frameworks are" },
            missing.join(", ")
        ),
        fix: hint,
    })
}

/// Hint appended when the linker reports an undefined symbol.
///
/// The two that actually bite are the implicit libraries above: a stripped or
/// partial SDK without `libSystem.tbd`/`libobjc.tbd` produces exactly these, and
/// a baffling "undefined symbol" with no hint is the worst possible error.
pub fn undefined_symbol_hint(symbol: &str) -> Option<String> {
    let known = IMPLICIT_LIBRARIES.iter().find(|library| match library.name {
        "System" => symbol.contains("stack_chk"),
        "objc" => {
            symbol.contains("objc_")
                || symbol.contains("OBJC_CLASS")
                || symbol.contains("_OBJC_")
        }
        _ => false,
    })?;
    Some(format!(
        "`{symbol}` is normally provided by lib{}, which darwinforge links by \
         default ({}). If it is still undefined, the SDK is missing {}.tbd; \
         check <SDK>/usr/lib for the .tbd stub.",
        known.name,
        known.reason,
        known.name
    ))
}

/// Link the objects into the executable at `output`.
// Same rationale as `compile_all`: these are the stage's inputs, in order.
#[allow(clippy::too_many_arguments)]
pub fn link_all(
    config: &Config,
    sdk: &Sdk,
    linker: &Path,
    kind: LinkerKind,
    objects: &[PathBuf],
    output: &Path,
    arch: &str,
    reporter: &Reporter,
) -> Result<()> {
    check_frameworks(config, sdk)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| Error::io(format!("cannot create {}", parent.display()), source))?;
    }
    let args = link_arguments(config, sdk, kind, objects, output, arch);
    exec::run(&linker.to_string_lossy(), &args, None, reporter)?;
    if !output.is_file() {
        return Err(Error::format(
            "link",
            format!("{} produced no output at {}", linker.display(), output.display()),
        ));
    }
    Ok(())
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
            frameworks = ["UIKit", "Foundation"]
            libraries = ["z"]
            ldflags = ["-dead_strip"]
        "#;
        Config::from_str(text, Path::new("/project")).expect("valid config")
    }

    fn sdk() -> Sdk {
        Sdk { root: PathBuf::from("/sdk"), version: Some("17.4".to_string()) }
    }

    fn args() -> Vec<String> {
        let objects = vec![PathBuf::from("/project/build/000-main.o")];
        link_arguments(
            &config(),
            &sdk(),
            LinkerKind::Lld,
            &objects,
            Path::new("/project/build/Hello.app/Hello"),
            "arm64",
        )
    }

    #[test]
    fn link_sets_arch_and_platform_version() {
        let args = args();
        assert!(args.windows(2).any(|pair| pair == ["-arch", "arm64"]));
        let index = args.iter().position(|a| a == "-platform_version").expect("present");
        assert_eq!(&args[index + 1..index + 4], ["ios", "13.0", "17.4"]);
    }

    #[test]
    fn link_declares_frameworks_and_libraries() {
        let args = args();
        assert!(args.windows(2).any(|pair| pair == ["-framework", "UIKit"]));
        assert!(args.windows(2).any(|pair| pair == ["-framework", "Foundation"]));
        assert!(args.contains(&"-lz".to_string()));
    }

    #[test]
    fn link_points_at_sdk_search_paths() {
        let args = args();
        let sdk = sdk();
        assert!(args.windows(2).any(|pair| pair
            == ["-syslibroot", &sdk.root.to_string_lossy()]));
        assert!(args.windows(2).any(|pair| pair
            == ["-L", &sdk.library_search_path().to_string_lossy()]));
        assert!(args.windows(2).any(|pair| pair
            == ["-F", &sdk.framework_search_path().to_string_lossy()]));
    }

    #[test]
    fn link_uses_main_entry_point_and_user_flags() {
        let args = args();
        assert!(args.windows(2).any(|pair| pair == ["-e", "_main"]));
        assert!(args.contains(&"-dead_strip".to_string()));
    }

    #[test]
    fn platform_version_falls_back_when_sdk_has_none() {
        let objects = vec![PathBuf::from("/project/build/main.o")];
        let sdk = Sdk { root: PathBuf::from("/sdk"), version: None };
        let args = link_arguments(
            &config(),
            &sdk,
            LinkerKind::Cctools,
            &objects,
            Path::new("/out"),
            "arm64",
        );
        let index = args.iter().position(|a| a == "-platform_version").expect("present");
        assert_eq!(&args[index + 1..index + 4], ["ios", "13.0", "13.0"]);
    }

    #[test]
    fn missing_frameworks_are_reported_with_a_hint() {
        let sdk = Sdk { root: PathBuf::from("/does/not/exist"), version: None };
        let error = check_frameworks(&config(), &sdk).expect_err("must fail");
        assert!(error.to_string().contains("UIKit"));
        assert!(error.to_string().contains("iPhoneOS"), "hint explains the SDK kind");
    }

    #[test]
    fn system_and_objc_are_linked_by_default() {
        // Regression guard: without these the link fails on a real SDK with
        // `undefined symbol: ___stack_chk_fail` (and `objc_msgSend` for ObjC).
        let args = args();
        assert!(args.contains(&"-lSystem".to_string()), "libSystem must be linked");
        assert!(args.contains(&"-lobjc".to_string()), "libobjc must be linked");
    }

    #[test]
    fn implicit_libraries_are_not_duplicated_when_already_listed() {
        let text = r#"
            [app]
            name = "Hello"
            bundle_id = "com.example.hello"
            min_ios_version = "13.0"

            [build]
            libraries = ["System", "objc", "z"]
        "#;
        let config = Config::from_str(text, Path::new("/project")).expect("valid config");
        let objects = vec![PathBuf::from("/project/build/main.o")];
        let args = link_arguments(
            &config,
            &sdk(),
            LinkerKind::Lld,
            &objects,
            Path::new("/out"),
            "arm64",
        );
        assert_eq!(
            args.iter().filter(|a| *a == "-lSystem").count(),
            1,
            "-lSystem must appear exactly once: {args:?}"
        );
        assert_eq!(
            args.iter().filter(|a| *a == "-lobjc").count(),
            1,
            "-lobjc must appear exactly once: {args:?}"
        );
        assert!(args.contains(&"-lz".to_string()), "the user's own libraries survive");
    }

    #[test]
    fn explicit_libraries_keep_their_declared_order() {
        let text = r#"
            [app]
            name = "Hello"
            bundle_id = "com.example.hello"
            min_ios_version = "13.0"

            [build]
            libraries = ["z", "System", "c++"]
        "#;
        let config = Config::from_str(text, Path::new("/project")).expect("valid config");
        let objects = vec![PathBuf::from("/project/build/main.o")];
        let args = link_arguments(
            &config,
            &sdk(),
            LinkerKind::Lld,
            &objects,
            Path::new("/out"),
            "arm64",
        );
        let libraries: Vec<&String> =
            args.iter().filter(|a| a.starts_with("-l") && !a.starts_with("-link")).collect();
        // `objc` is not in the user's list, so it is added implicitly; `System` is
        // listed by the user, so it is not duplicated. The user's own libraries
        // keep their declared order.
        assert_eq!(
            libraries,
            vec!["-lobjc", "-lz", "-lSystem", "-lc++"],
            "got {libraries:?}"
        );
    }

    #[test]
    fn every_implicit_library_documents_what_it_fixes() {
        // A library nobody can justify is a library nobody can debug.
        for library in IMPLICIT_LIBRARIES {
            assert!(!library.reason.is_empty(), "`{}` must say why it is linked", library.name);
        }
        assert!(IMPLICIT_LIBRARIES.iter().any(|l| l.reason.contains("stack_chk")));
        assert!(IMPLICIT_LIBRARIES.iter().any(|l| l.reason.contains("Objective-C")));
    }

    #[test]
    fn undefined_symbols_from_the_implicit_libraries_get_a_hint() {
        // The exact symbols reported by a real failed link.
        for symbol in [
            "___stack_chk_fail",
            "___stack_chk_guard",
            "_objc_msgSend",
            "_OBJC_CLASS_$_UIViewController",
        ] {
            let hint = undefined_symbol_hint(symbol)
                .unwrap_or_else(|| panic!("no hint for `{symbol}`"));
            assert!(hint.contains(symbol), "the hint names the symbol: {hint}");
            assert!(hint.contains(".tbd"), "the hint says what to look for: {hint}");
        }
    }

    #[test]
    fn unrelated_undefined_symbols_get_no_speculative_hint() {
        // We must not invent an explanation for a symbol we do not understand.
        for symbol in ["_myOwnMissingFunction", "_OBJC_CLASS_$_NSObject2", ""] {
            if symbol.is_empty() {
                assert!(undefined_symbol_hint(symbol).is_none());
                continue;
            }
            if symbol == "_OBJC_CLASS_$_NSObject2" {
                continue; // genuinely an objc symbol; a hint is correct here.
            }
            assert!(
                undefined_symbol_hint(symbol).is_none(),
                "`{symbol}` must not claim to know the cause"
            );
        }
    }
}
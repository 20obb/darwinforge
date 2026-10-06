//! Which iPhoneOS SDK to use, and why.
//!
//! Four places can name an SDK: `--sdk`, `$DARWINFORGE_SDK`, the global config
//! `bootstrap` writes, and the managed `sdks/` directory of installed SDKs. The
//! reported bug was that `doctor` consulted only the first two — so a machine
//! set up by `bootstrap` had a saved SDK that **neither** command would use, and
//! `doctor` reported the SDK as missing while a build would have worked.
//!
//! One [`resolve_sdk`] function is therefore shared by `build`, `doctor` and the
//! preflight. They cannot disagree about which SDK is in use, and the returned
//! [`SdkOrigin`] lets every one of them say where the answer came from.

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::paths;
use crate::sdk::Sdk;

/// The SDK darwinforge should use, and where that decision came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkResolution {
    /// The SDK root that will be used.
    pub root: PathBuf,
    /// Where the path came from, for `doctor` and `-v` output.
    pub origin: SdkOrigin,
    /// The version, as read from the SDK's own settings.
    pub version: Option<String>,
}

/// Which of the four sources supplied the SDK path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkOrigin {
    /// `--sdk PATH` on the command line.
    Flag,
    /// `$DARWINFORGE_SDK` (or the legacy `$IPAFORGE_SDK`).
    Environment,
    /// The global config written by `bootstrap`.
    Config,
    /// A directory under the managed `sdks/` directory.
    Installed,
}

impl SdkOrigin {
    /// One phrase for the `doctor` report and verbose build output.
    pub fn label(self) -> &'static str {
        match self {
            SdkOrigin::Flag => "--sdk",
            SdkOrigin::Environment => "DARWINFORGE_SDK",
            SdkOrigin::Config => "saved config",
            SdkOrigin::Installed => "installed SDK",
        }
    }
}

/// Everything [`resolve_sdk`] needs, injected so the order can be tested without
/// touching a real config file or the real environment.
#[derive(Debug, Clone, Default)]
pub struct SdkSources {
    /// `--sdk PATH`.
    pub flag: Option<PathBuf>,
    /// `$DARWINFORGE_SDK`.
    pub environment: Option<PathBuf>,
    /// `sdk_path` from the global config.
    pub config: Option<PathBuf>,
    /// SDK directories discovered under the managed directory, oldest first.
    pub installed: Vec<PathBuf>,
}

impl SdkSources {
    /// Collect the sources that need no I/O beyond a directory scan.
    ///
    /// The global config is deliberately **not** read here: a caller that must
    /// not fail on an unreadable config (a preflight, or `doctor`) can then run
    /// even when the config is malformed. Call [`SdkSources::with_config`] when
    /// the saved SDK is in scope.
    pub fn from_environment(flag: Option<PathBuf>) -> SdkSources {
        SdkSources {
            flag,
            environment: crate::compat::env_var("SDK").map(PathBuf::from),
            config: None,
            installed: installed_sdks(),
        }
    }

    /// Read the saved config and add it as a source.
    ///
    /// A config that cannot be read is ignored rather than fatal: the point of
    /// this call is to work out whether the environment is usable, and refusing
    /// to look because of a malformed config would be unhelpful.
    pub fn with_config(mut self) -> SdkSources {
        self.config =
            crate::global_config::GlobalConfig::load().ok().and_then(|config| config.sdk_path);
        self
    }
}

/// Every usable SDK darwinforge has installed, ordered oldest version first.
///
/// The directories are named by version ([`crate::sdksource::install_dir`]), so
/// they are ordered **numerically** rather than by directory name — `9.3`
/// before `10.3`. Anything that does not open as a real SDK is skipped, so an
/// interrupted install is never offered as if it were usable.
pub fn installed_sdks() -> Vec<PathBuf> {
    let Ok(dir) = paths::sdks_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut found: Vec<(crate::sdksource::SdkVersion, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let version = crate::sdksource::SdkVersion::parse(path.file_name()?.to_str()?)?;
            Sdk::open(&path).ok()?;
            Some((version, path))
        })
        .collect();
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found.into_iter().map(|(_, path)| path).collect()
}

/// Resolve the SDK to use, in one fixed order.
///
/// `--sdk`, then `$DARWINFORGE_SDK`, then the saved global config, then the
/// newest installed SDK.
///
/// The first source that names an existing directory wins. A configured path
/// that **does not exist** is an error rather than a fall-through: silently
/// building against some other SDK than the one asked for would be far worse
/// than refusing, and a named path that is missing is a mistake worth
/// surfacing. "Nothing configured anywhere" is a different error with a
/// different fix, and it says which four places were consulted.
pub fn resolve_sdk(sources: &SdkSources) -> Result<SdkResolution> {
    if let Some(flag) = &sources.flag {
        return open(flag.clone(), SdkOrigin::Flag);
    }
    if let Some(environment) = &sources.environment {
        return open(environment.clone(), SdkOrigin::Environment);
    }
    if let Some(config) = &sources.config {
        if config.is_dir() {
            return open(config.clone(), SdkOrigin::Config);
        }
        return Err(Error::setup(
            format!("the SDK saved in the global config is gone: {}", paths::display_path(config)),
            "run `darwinforge sdk install` to fetch one again, or point at another with \
             --sdk PATH"
                .to_string(),
        ));
    }
    // `installed` is oldest first, so the newest SDK is the last entry.
    match sources.installed.iter().rev().find(|path| path.is_dir()) {
        Some(newest) => open(newest.clone(), SdkOrigin::Installed),
        None => Err(no_sdk_error()),
    }
}

/// Build the resolution for a path, validating it as we go.
///
/// Validating here is what keeps `doctor` and `build` in agreement: every caller
/// gets the same answer about whether a directory is a usable SDK rather than
/// applying its own criteria.
fn open(root: PathBuf, origin: SdkOrigin) -> Result<SdkResolution> {
    let sdk = Sdk::open(&root)?;
    Ok(SdkResolution { root, origin, version: sdk.version })
}

/// The error for "no SDK has been configured anywhere".
///
/// It names every source consulted, so the user can see that `build` and
/// `doctor` looked in the same four places and found nothing in any of them.
pub fn no_sdk_error() -> Error {
    Error::setup(
        "no iPhoneOS SDK is configured",
        format!(
            "darwinforge looked at --sdk, $DARWINFORGE_SDK, the saved config and {} and \
             found none. Run `darwinforge sdk install` to fetch one, or `darwinforge sdk` \
             to choose one",
            paths::sdks_dir()
                .map(|dir| paths::display_path(&dir))
                .unwrap_or_else(|_| "the managed sdks directory".to_string())
        ),
    )
}

/// Resolve and open in one step, for the pipeline's use.
pub fn resolve_and_open(sources: &SdkSources) -> Result<(SdkResolution, Sdk)> {
    let resolution = resolve_sdk(sources)?;
    let sdk = Sdk::open(&resolution.root)?;
    Ok((resolution, sdk))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("darwinforge-resolve-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A minimal directory that passes `Sdk::open`.
    fn fake_sdk(dir: &Path, version: &str) -> PathBuf {
        let root = dir.join(version);
        for subdir in ["usr/include", "usr/lib", "System/Library/Frameworks"] {
            std::fs::create_dir_all(root.join(subdir)).expect("mkdir");
        }
        std::fs::write(root.join("SDKSettings.json"), format!("{{\"Version\":\"{version}\"}}"))
            .expect("write");
        root
    }

    #[test]
    fn the_flag_wins_over_everything_else() {
        let dir = scratch("flag");
        let flag = fake_sdk(&dir, "17.5");
        let other = fake_sdk(&dir, "18.0");
        let sources = SdkSources {
            flag: Some(flag.clone()),
            environment: Some(other.clone()),
            config: Some(other.clone()),
            installed: vec![other],
        };
        let resolved = resolve_sdk(&sources).expect("must resolve");
        assert_eq!(resolved.root, flag);
        assert_eq!(resolved.origin, SdkOrigin::Flag);
        assert_eq!(resolved.version.as_deref(), Some("17.5"), "read from the SDK itself");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_environment_is_consulted_before_the_saved_config() {
        let dir = scratch("env");
        let environment = fake_sdk(&dir, "17.5");
        let config = fake_sdk(&dir, "18.0");
        let sources =
            SdkSources { environment: Some(environment), config: Some(config), ..Default::default() };
        assert_eq!(resolve_sdk(&sources).expect("must resolve").origin, SdkOrigin::Environment);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_saved_config_is_used_when_nothing_was_flagged() {
        // The reported bug: `doctor` ignored the config `bootstrap` had written,
        // so it reported an SDK as missing that a build would happily use. Both
        // now call this function, so they cannot disagree.
        let dir = scratch("config");
        let config = fake_sdk(&dir, "17.5");
        let sources = SdkSources { config: Some(config.clone()), ..Default::default() };
        let resolved = resolve_sdk(&sources).expect("the saved SDK must be found");
        assert_eq!(resolved.root, config);
        assert_eq!(resolved.origin, SdkOrigin::Config);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_newest_installed_sdk_is_used_when_nothing_else_is_configured() {
        let dir = scratch("installed");
        let sources = SdkSources {
            // `installed_sdks` returns these oldest first, so the fixture must be
            // in that order for the test to mean anything.
            installed: vec![
                fake_sdk(&dir, "9.3"),
                fake_sdk(&dir, "10.3"),
                fake_sdk(&dir, "17.5"),
                fake_sdk(&dir, "26.4.1"),
            ],
            ..Default::default()
        };
        let resolved = resolve_sdk(&sources).expect("must resolve");
        assert_eq!(
            resolved.root.file_name().unwrap().to_string_lossy(),
            "26.4.1",
            "numeric ordering, so 26.4.1 beats 17.5 and 9.3"
        );
        assert_eq!(resolved.origin, SdkOrigin::Installed);
        let _ = std::fs::remove_dir_all(&dir);
    }
#[test]
    fn a_flag_that_does_not_exist_is_an_error_rather_than_a_fallthrough() {
        // Silently building against a different SDK than the one requested would
        // be far worse than an error.
        let dir = scratch("missing-flag");
        let sources = SdkSources {
            flag: Some(dir.join("nope.sdk")),
            installed: vec![fake_sdk(&dir, "17.5")],
            ..Default::default()
        };
        let error = resolve_sdk(&sources).expect_err("must not fall through");
        assert!(error.to_string().contains("nope.sdk"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_sdk_that_has_vanished_says_so() {
        let dir = scratch("vanished");
        let sources = SdkSources {
            config: Some(dir.join("gone")),
            installed: vec![fake_sdk(&dir, "17.5")],
            ..Default::default()
        };
        let error = resolve_sdk(&sources).expect_err("must report the removal");
        let text = error.to_string();
        assert!(text.contains("saved in the global config is gone"), "{text}");
        assert_eq!(error.exit_code(), 5, "it is a setup problem: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_configured_anywhere_is_a_setup_error_naming_every_source() {
        let error = resolve_sdk(&SdkSources::default()).expect_err("must fail");
        let text = error.to_string();
        assert!(text.contains("DARWINFORGE_SDK"), "names the sources: {text}");
        assert!(text.contains("sdk install"), "and the fix: {text}");
        assert_eq!(error.exit_code(), 5, "exit 5 means 'not set up'");
    }

    #[test]
    fn a_directory_that_is_not_an_sdk_is_rejected() {
        let dir = scratch("not-an-sdk");
        let bogus = dir.join("iPhoneOS99.0");
        std::fs::create_dir_all(&bogus).expect("mkdir");
        let sources = SdkSources { flag: Some(bogus.clone()), ..Default::default() };
        let error = resolve_sdk(&sources).expect_err("must reject");
        assert!(error.to_string().contains("iPhoneOS"), "names it: {error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_origin_has_a_label() {
        // `doctor` prints this, so a missing label would print nothing.
        for origin in
            [SdkOrigin::Flag, SdkOrigin::Environment, SdkOrigin::Config, SdkOrigin::Installed]
        {
            assert!(!origin.label().is_empty(), "{origin:?} needs a label");
        }
    }
}
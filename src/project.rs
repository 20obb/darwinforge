//! Build target resolution.
//!
//! `darwinforge build` accepts three things:
//!
//! * a project directory containing `darwinforge.toml`,
//! * a path to a `darwinforge.toml`, or
//! * a compilable source file inside (or above) a project.
//!
//! The resolver keeps that distinction separate from [`crate::config::Config`]
//! so zero-config inference and explicit configuration share one front door.

use std::path::{Path, PathBuf};

use crate::compat;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::infer::{self, Inference};

/// What a target resolved to.
#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub config: Config,
    /// Present only in zero-config mode.
    pub inferred: Option<Inference>,
}

/// Resolve `target` to the directory that should be treated as the project.
///
/// A source file inside a configured project uses the surrounding project;
/// otherwise its own parent directory is the zero-config project root.
pub fn project_root(target: &Path) -> Result<PathBuf> {
    if target.is_dir() {
        return resolve_directory(target);
    }
    if target.is_file() {
        return resolve_file(target);
    }
    Err(Error::Prereq {
        what: format!("{} does not exist", target.display()),
        fix: "pass a project directory, a darwinforge.toml, or a single .c/.m/.mm/.cpp/.swift file"
            .to_string(),
    })
}

/// Load or infer the project at `target`.
pub fn load(target: &Path, sdk_version: Option<&str>) -> Result<Project> {
    let root = project_root(target)?;
    match compat::find_project_config(&root) {
        Some(_) => {
            let config = Config::load(&root)?;
            Ok(Project { root, config, inferred: None })
        }
        None => {
            let inferred = infer::infer(&root, sdk_version)?;
            let config = infer::to_config(&inferred)?;
            Ok(Project { root, config, inferred: Some(inferred) })
        }
    }
}

/// Where `clean` should operate for `target`.
pub fn build_dir_for(target: &Path) -> Result<PathBuf> {
    let root = project_root(target)?;
    match compat::find_project_config(&root) {
        Some(_) => Ok(Config::load(&root)?.build_dir()),
        None => Ok(root.join("build")),
    }
}

fn resolve_directory(target: &Path) -> Result<PathBuf> {
    if let Some(root) = configured_ancestor(target) {
        return Ok(root);
    }
    Ok(target.to_path_buf())
}

fn resolve_file(target: &Path) -> Result<PathBuf> {
    let Some(name) = target.file_name().and_then(|name| name.to_str()) else {
        return Err(invalid_file(target));
    };
    match name {
        compat::CONFIG_FILE_NAME | compat::LEGACY_CONFIG_FILE_NAME => parent_of(target),
        _ if target.extension().and_then(|ext| ext.to_str()) == Some("toml") => {
            Err(Error::Usage(format!(
                "unrecognised config file `{}`; supported config files are \
                 {} and {}",
                target.display(),
                compat::CONFIG_FILE_NAME,
                compat::LEGACY_CONFIG_FILE_NAME
            )))
        }
        _ if crate::discovery::Language::from_path(target).is_some() => {
            let parent = parent_of(target)?;
            if let Some(root) = configured_ancestor(&parent) {
                return Ok(root);
            }
            Ok(parent)
        }
        _ => Err(invalid_file(target)),
    }
}

fn parent_of(path: &Path) -> Result<PathBuf> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .ok_or_else(|| invalid_file(path))
}

fn configured_ancestor(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if compat::find_project_config(&current).is_some() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn invalid_file(path: &Path) -> Error {
    Error::Usage(format!(
        "`{}` is not a project directory, darwinforge.toml, or compilable source file",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "darwinforge-project-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parents");
        }
        std::fs::write(path, contents).expect("write");
    }

    fn configured_project(tag: &str) -> PathBuf {
        let root = temp_dir(tag);
        write(
            &root.join("darwinforge.toml"),
            r#"[app]
name = "App"
bundle_id = "com.example.app"
min_ios_version = "13.0"

[build]
sources = ["main.m"]
frameworks = ["UIKit"]
"#,
        );
        write(&root.join("main.m"), "int main(void){return 0;}\n");
        root
    }

    #[test]
    fn a_directory_with_a_config_loads_it() {
        let root = configured_project("config-dir");
        let project = load(&root, Some("17.5")).expect("loads");
        assert!(project.inferred.is_none());
        assert_eq!(project.root, root);
        let _ = std::fs::remove_dir_all(project.root);
    }

    #[test]
    fn explicit_config_extends_the_configured_root() {
        let root = configured_project("config-file");
        let project = load(&root.join("darwinforge.toml"), None).expect("loads");
        assert_eq!(project.root, root);
        assert!(project.inferred.is_none());
        let _ = std::fs::remove_dir_all(project.root);
    }

    #[test]
    fn a_source_file_builds_its_surrounding_project() {
        let root = configured_project("source-file");
        write(&root.join("Sources/main.m"), "int helper(void){return 1;}\n");
        let project = load(&root.join("Sources/main.m"), None).expect("loads");
        assert_eq!(project.root, root);
        assert!(project.inferred.is_none());
        let _ = std::fs::remove_dir_all(project.root);
    }

    #[test]
    fn a_directory_without_config_is_inferred() {
        let root = temp_dir("infer");
        write(&root.join("main.m"), "#import <UIKit/UIKit.h>\nint main(void){return 0;}\n");
        let project = load(&root, Some("17.5")).expect("infers");
        assert!(project.inferred.is_some());
        assert_eq!(project.config.build.frameworks, ["UIKit"]);
        assert_eq!(project.config.app.min_ios_version, "17.5");
        let _ = std::fs::remove_dir_all(project.root);
    }

    #[test]
    fn an_unknown_toml_file_is_rejected() {
        let root = temp_dir("bad-toml");
        write(&root.join("other.toml"), "[app]\nname = \"App\"\n");
        let error = load(&root.join("other.toml"), None).expect_err("must reject");
        let text = error.to_string();
        assert!(text.contains("darwinforge.toml"), "{text}");
        assert!(text.contains("ipaforge.toml"), "{text}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clean_uses_the_configured_build_directory() {
        let root = configured_project("clean");
        assert_eq!(build_dir_for(&root).expect("build dir"), root.join("build"));
        let _ = std::fs::remove_dir_all(root);
    }
}

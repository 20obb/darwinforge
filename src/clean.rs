//! `darwinforge clean` — remove only generated build output.

use std::path::PathBuf;

use crate::error::{Error, Result};

/// Remove the project's build directory.
pub fn run(target: Option<&PathBuf>, dry_run: bool) -> Result<()> {
    let target = target.map(PathBuf::as_path).unwrap_or_else(|| std::path::Path::new("."));
    let root = crate::project::project_root(target)?;
    let build_dir = crate::project::build_dir_for(target)?;
    if !build_dir.exists() {
        println!("nothing to clean: {} does not exist", build_dir.display());
        return Ok(());
    }
    if !build_dir.is_dir() {
        return Err(Error::Prereq {
            what: format!("{} exists but is not a directory", build_dir.display()),
            fix: "remove the stray file yourself; `clean` never deletes files it cannot identify as a build directory"
                .to_string(),
        });
    }
    if !is_under_project(&root, &build_dir) {
        return Err(Error::Prereq {
            what: format!("refusing to remove {}", build_dir.display()),
            fix: "the configured build directory must live under the project".to_string(),
        });
    }
    if dry_run {
        println!("would remove {}", build_dir.display());
        return Ok(());
    }
    std::fs::remove_dir_all(&build_dir).map_err(|source| {
        Error::io(format!("cannot remove {}", build_dir.display()), source)
    })?;
    println!("removed {}", build_dir.display());
    Ok(())
}

fn is_under_project(root: &std::path::Path, path: &std::path::Path) -> bool {
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    let Ok(path) = std::fs::canonicalize(path) else {
        return false;
    };
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_file_named_build() {
        let dir = std::env::temp_dir().join(format!("darwinforge-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp");
        std::fs::write(dir.join("darwinforge.toml"), "[app]\nname = \"App\"\nbundle_id = \"com.example.app\"\nmin_ios_version = \"13.0\"\n\n[build]\nsources = [\"main.m\"]\n").expect("config");
        std::fs::write(dir.join("main.m"), "int main(void){return 0;}\n").expect("source");
        std::fs::write(dir.join("build"), "not a directory").expect("file");
        let error = run(Some(&dir), false).expect_err("must refuse");
        assert!(error.to_string().contains("not a directory"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

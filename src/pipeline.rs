//! Orchestration: config → discovery → compile → link → bundle → sign → package.
//!
//! Every stage is a module function; this file only sequences them and owns
//! the deterministic layout under `./build/`.

use std::path::{Path, PathBuf};

use crate::bundle;
use crate::compile;
use crate::config::Config;
use crate::discovery;
use crate::error::{Error, Result};
use crate::link;
use crate::mach;
use crate::package::Packager;
use crate::reporter::Reporter;
use crate::sdk::{Sdk, Toolchain};
use crate::sign::{LdidSigner, Signer};

/// Everything the pipeline produced, for tests and callers.
pub struct BuildOutput {
    pub objects: Vec<PathBuf>,
    pub executable: PathBuf,
    pub app_bundle: PathBuf,
    pub ipa: PathBuf,
}

/// Knobs that come from the command line rather than `darwinforge.toml`.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub arch: String,
    pub output: Option<PathBuf>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions { arch: "arm64".to_string(), output: None }
    }
}

/// Run the full pipeline.
pub fn build(
    config: &Config,
    sdk: &Sdk,
    toolchain: &Toolchain,
    packager: &dyn Packager,
    options: &BuildOptions,
    reporter: &Reporter,
) -> Result<BuildOutput> {
    reporter.stage("discover sources");
    let found = discovery::discover(config, reporter)?;
    discovery::check_buildable(&found)?;
    reporter.info(&format!(
        "  {} source(s), {} resource(s)",
        found.sources.len(),
        found.resources.len()
    ));

    // Deterministic layout under ./build/.
    let build_dir = config.build_dir();
    let object_dir = build_dir.join("obj");
    let app_dir = build_dir.join(config.app_bundle_name());
    let executable = build_dir.join(&config.app.name);
    let ipa = match &options.output {
        Some(path) => path.clone(),
        None => build_dir.join(format!("{}.ipa", config.app.name)),
    };

    reporter.stage(&format!("compile ({})", found.sources.len()));
    let objects = compile::compile_all(
        config,
        sdk,
        &toolchain.clang,
        toolchain.swiftc.as_deref(),
        &found.sources,
        &object_dir,
        &options.arch,
        reporter,
    )?;

    reporter.stage("link");
    link::link_all(
        config,
        sdk,
        &toolchain.linker,
        toolchain.linker_kind,
        &objects,
        &executable,
        &options.arch,
        reporter,
    )?;
    verify_executable(&executable, &options.arch)?;

    reporter.stage("bundle");
    let app = bundle::assemble(config, &executable, &found.resources, &app_dir, &options.arch)?;
    reporter.info(&format!("  {}", app.path.display()));

    reporter.stage("sign");
    let signer = LdidSigner::new(toolchain.ldid.clone());
    signer.sign(&app.path, reporter)?;
    reporter.info(&format!("  {}", signer.name()));

    reporter.stage("package");
    packager.package(&app.path, &ipa, reporter)?;
    reporter.info(&format!("  {}", ipa.display()));

    Ok(BuildOutput { objects, executable, app_bundle: app.path, ipa })
}

/// Confirm the linker produced what we asked for: a thin arm64 iOS executable.
pub fn verify_executable(path: &Path, arch: &str) -> Result<()> {
    let bytes = std::fs::read(path)
        .map_err(|source| Error::io(format!("cannot read {}", path.display()), source))?;
    let macho =
        mach::parse(&bytes).map_err(|message| Error::format("link", format!("{}: {message}", path.display())))?;
    if !macho.is_executable() {
        return Err(Error::format(
            "link",
            format!("{} is a {} but an executable was expected", path.display(), macho.describe()),
        ));
    }
    if !macho.is_arm64() && arch == "arm64" {
        return Err(Error::format(
            "link",
            format!(
                "{} is a {} but arm64 was requested; check that -arch {arch} reached the linker",
                path.display(),
                macho.describe()
            ),
        ));
    }
    if !macho.segment_names.iter().any(|name| name == "__TEXT") {
        return Err(Error::format(
            "link",
            format!(
                "{} has no __TEXT segment, so it cannot run: {}",
                path.display(),
                macho.describe()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mach::testkit::synthesize;
    use mach::{LC_BUILD_VERSION, LC_SEGMENT_64};

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("darwinforge-verify-{tag}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("fake")
    }

    #[test]
    fn rejects_a_non_macho_executable() {
        let path = temp_path("not-macho");
        std::fs::write(&path, b"this is not a Mach-O binary at all").expect("write");
        let error = verify_executable(&path, "arm64").expect_err("must fail");
        assert!(error.to_string().contains("not a Mach-O"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_wrong_architecture() {
        let path = temp_path("arch");
        let mut bytes = synthesize(&[LC_SEGMENT_64, LC_BUILD_VERSION], None);
        bytes[4..8].copy_from_slice(&0x0100_0007u32.to_le_bytes()); // x86_64
        std::fs::write(&path, bytes).expect("write");
        let error = verify_executable(&path, "arm64").expect_err("must fail");
        assert!(error.to_string().contains("arm64 was requested"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn accepts_a_synthesized_arm64_executable() {
        let path = temp_path("ok");
        std::fs::write(&path, synthesize(&[LC_SEGMENT_64, LC_BUILD_VERSION], None))
            .expect("write");
        verify_executable(&path, "arm64").expect("must pass");
        let _ = std::fs::remove_file(&path);
    }
}
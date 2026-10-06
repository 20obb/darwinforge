//! `main` — thin shell: parse args, dispatch, map errors to exit codes.
//!
//! All behaviour lives in the library so the integration test can drive the
//! same code paths the binary uses.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use darwinforge::cli::{self, Command};
use darwinforge::error::Result;
use darwinforge::package;
use darwinforge::pipeline::{self, BuildOptions};
use darwinforge::reporter::Reporter;
use darwinforge::sdk::{Sdk, Toolchain};
use darwinforge::{bootstrap, clean, completions, doctor, sdkcmd, scaffold};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

fn run(args: &[String]) -> Result<()> {
    let invocation = cli::parse(args)?;
    let reporter = Reporter::new(invocation.verbose);

    match invocation.command {
        Command::Help => {
            print!("{}", cli::USAGE);
            Ok(())
        }
        Command::Version => {
            println!("{} {}", darwinforge::compat::DISPLAY_NAME, env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Doctor { sdk, fix } => doctor::run(sdk.as_deref(), fix, invocation.yes, &reporter),
        Command::Bootstrap(options) => bootstrap::run(&options, &reporter),
        Command::Init { name, bundle_id, dir, force, here } => {
            if here {
                let sources =
                    darwinforge::sdkpath::SdkSources::from_environment(None).with_config();
                let sdk_version = darwinforge::sdkpath::resolve_sdk(&sources)
                    .ok()
                    .and_then(|resolution| resolution.version);
                scaffold::init_here(&dir, bundle_id.as_deref(), force, sdk_version.as_deref())
                    .map(|_| ())
            } else {
                let name = name.expect("init name parses");
                scaffold::run(&name, bundle_id.as_deref(), &dir, force).map(|_| ())
            }
        }
        Command::Build { sdk, arch, target, project, output } => {
            build(target.as_deref(), project.as_deref(), sdk, &arch, output, &reporter)
        }
        Command::Sdk(options, action) => sdkcmd::run(&options, &action),
        Command::Clean { target, dry_run } => clean::run(target.as_ref(), dry_run),
        Command::Completions { shell } => {
            print!("{}", completions::for_shell(&shell)?);
            Ok(())
        }
    }
}

/// Resolve the SDK through the shared rule, then open it.
///
/// [`darwinforge::sdkpath`] is the single source of truth for *which* SDK a
/// command uses, so `build` and `doctor` cannot disagree about it — the
/// reported bug was exactly that disagreement.
fn resolve_sdk(
    sdk_arg: Option<PathBuf>,
    reporter: &Reporter,
) -> Result<(darwinforge::sdkpath::SdkResolution, Sdk)> {
    let sources =
        darwinforge::sdkpath::SdkSources::from_environment(sdk_arg).with_config();
    let (resolution, sdk) = darwinforge::sdkpath::resolve_and_open(&sources)?;
    reporter.info(&format!(
        "  SDK      {} ({}, from {})",
        resolution.root.display(),
        resolution.version.clone().unwrap_or_else(|| "unknown".to_string()),
        resolution.origin.label()
    ));
    Ok((resolution, sdk))
}

fn build(
    target: Option<&Path>,
    project: Option<&Path>,
    sdk_arg: Option<PathBuf>,
    arch: &str,
    output: Option<PathBuf>,
    reporter: &Reporter,
) -> Result<()> {
    if let (Some(target), Some(project)) = (target, project) {
        if target != project {
            return Err(darwinforge::error::Error::Usage(
                "pass the project either as `build PATH` or `build --project PATH`, not both"
                    .to_string(),
            ));
        }
    }
    let target = target.or(project).unwrap_or_else(|| Path::new("."));
    let (_, sdk) = resolve_sdk(sdk_arg, reporter)?;
    let project = darwinforge::project::load(target, sdk.version.as_deref())?;
    if let Some(inferred) = &project.inferred {
        for line in inferred.describe() {
            println!("{line}");
        }
    }
    let config = project.config;
    let toolchain = Toolchain::discover()?;
    let packager = package::select(config.build.use_external_zip, toolchain.zip.clone())?;

    let options = BuildOptions { arch: arch.to_string(), output };
    let result = pipeline::build(&config, &sdk, &toolchain, packager.as_ref(), &options, reporter)?;

    println!();
    println!("built {}", result.ipa.display());
    let size = std::fs::metadata(&result.ipa).map(|meta| meta.len()).unwrap_or(0);
    println!("size   {} bytes", size);
    println!("bundle {}", result.app_bundle.display());
    if reporter.warning_count() > 0 {
        println!();
        println!(
            "note: {} item(s) were skipped with warnings; see above — the app builds, but \
             those features are not part of this PoC.",
            reporter.warning_count()
        );
    }
    Ok(())
}
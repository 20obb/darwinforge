//! `main` — thin shell: parse args, dispatch, map errors to exit codes.
//!
//! All behaviour lives in the library so the integration test can drive the
//! same code paths the binary uses.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use darwinforge::cli::{self, Command};
use darwinforge::config::Config;
use darwinforge::error::{Error, Result};
use darwinforge::package;
use darwinforge::pipeline::{self, BuildOptions};
use darwinforge::reporter::Reporter;
use darwinforge::sdk::{Sdk, Toolchain};
use darwinforge::{bootstrap, doctor, scaffold};

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
        Command::Doctor { sdk } => doctor::run(sdk.as_deref(), &reporter),
        Command::Bootstrap(options) => bootstrap::run(&options, &reporter),
        Command::Init { name, bundle_id, dir, force } => {
            scaffold::run(&name, bundle_id.as_deref(), &dir, force).map(|_| ())
        }
        Command::Build { sdk, arch, project, output } => {
            build(&project, sdk, &arch, output, &reporter)
        }
    }
}

fn build(
    project: &Path,
    sdk_arg: Option<PathBuf>,
    arch: &str,
    output: Option<PathBuf>,
    reporter: &Reporter,
) -> Result<()> {
    let sdk_path = sdk_arg
        .or_else(|| darwinforge::compat::env_var("SDK").map(PathBuf::from))
        .ok_or_else(|| Error::Prereq {
            what: "no iPhoneOS SDK given".to_string(),
            fix: "pass --sdk /path/to/iPhoneOS17.4.sdk or set DARWINFORGE_SDK. darwinforge \
                  never downloads an SDK; you supply the extracted one"
                .to_string(),
        })?;
    let sdk = Sdk::open(&sdk_path)?;
    let config = Config::load(project)?;
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
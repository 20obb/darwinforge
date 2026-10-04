//! Hand-rolled argument parsing. Kept dependency-free and small; unknown flags
//! are errors, so a typo never silently changes what gets built.

use std::path::PathBuf;

use crate::error::{Error, Result};

pub const USAGE: &str = r#"DarwinForge — build an installable iOS .ipa on Linux, without a Mac

USAGE:
    darwinforge bootstrap [--yes] [--no-install] [--sdk-version V|latest] [--dry-run]
    darwinforge doctor [--sdk PATH] [-v]
    darwinforge init <name> [--bundle-id ID] [--dir PATH] [--force]
    darwinforge build [--sdk PATH] [--arch ARCH] [--project PATH] [-o FILE] [-v]

COMMANDS:
    bootstrap    Prepare this machine: detect the OS, install the toolchain,
                 install an iPhoneOS SDK and smoke-test the result. Idempotent.
    doctor      Check for clang, a Mach-O linker, ldid, zip and the SDK path.
    init        Create a minimal Objective-C UIKit project with a config file.
    build       Run the full pipeline: compile, link, bundle, sign, package.

BOOTSTRAP OPTIONS:
    --yes                 Accept every default; never prompt. Required in CI.
    --no-install          Change nothing: report what is missing and how to fix it.
    --sdk-version V|latest Which SDK to install (default: latest).
    --accept-sdk-license  Acknowledge Apple's SDK licence non-interactively.
    --dry-run             Print the full plan and exit without changing anything.
    --source URL          SDK repository (default: the configured sdk.source).

OPTIONS:
    --sdk PATH        Path to an extracted iPhoneOS SDK you provide.
                      Also read from $DARWINFORGE_SDK. darwinforge never downloads one.
    --arch ARCH       Target architecture (default: arm64).
    --project PATH    Project directory (default: current directory).
    -o, --output FILE Where to write the .ipa (default: <project>/build/<Name>.ipa).
    --bundle-id ID    Bundle identifier for `init` (default: com.example.<name>).
    --dir PATH        Parent directory for `init` (default: current directory).
    --force           Overwrite an existing project in `init`.
    -v, --verbose     Echo every external command before running it.
    -h, --help        Show this help.
    --version         Show the version.

FILES:
    darwinforge.toml            Per-project configuration.
    ~/.config/darwinforge/      Global configuration and installed SDKs.

ENVIRONMENT:
    DARWINFORGE_SDK      Default for --sdk.
    DARWINFORGE_CLANG    Override the clang binary.
    DARWINFORGE_LINKER   Override the linker (ld64.lld or cctools ld).
    DARWINFORGE_LDID     Override the ldid binary.
    DARWINFORGE_SWIFTC   Override swiftc (optional Swift support).

    The former IPAFORGE_* names are still read as deprecated fallbacks.

EXIT CODES:
    0 success  1 tool/IO failure  2 bad usage  3 bad config  4 missing prerequisite
    5 the environment is not set up (run `darwinforge bootstrap`)
"#;

/// Parsed command line.
#[derive(Debug)]
pub enum Command {
    /// Prepare the machine. Idempotent and safe to re-run.
    Bootstrap(BootstrapOptions),
    Doctor { sdk: Option<PathBuf> },
    Init { name: String, bundle_id: Option<String>, dir: PathBuf, force: bool },
    Build { sdk: Option<PathBuf>, arch: String, project: PathBuf, output: Option<PathBuf> },
    Help,
    Version,
}

/// Everything `bootstrap` can be told from the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootstrapOptions {
    /// Accept every default without prompting. Required in CI.
    pub yes: bool,
    /// Report what is missing but change nothing.
    pub no_install: bool,
    /// Print the plan and change nothing.
    pub dry_run: bool,
    /// Which SDK to install: a version, or `latest`.
    pub sdk_version: Option<String>,
    /// Acknowledge Apple's SDK licence without prompting.
    pub accept_sdk_license: bool,
    /// Override the SDK repository.
    pub source: Option<String>,
}

impl BootstrapOptions {
    /// True when the run must not change the machine.
    ///
    /// `--dry-run` and `--no-install` overlap: both promise read-only, but
    /// `--dry-run` additionally prints the full plan of what *would* happen,
    /// while `--no-install` reports the current state only.
    pub fn is_read_only(&self) -> bool {
        self.no_install || self.dry_run
    }
}

#[derive(Debug)]
pub struct Invocation {
    pub command: Command,
    pub verbose: bool,
}

/// Parse `argv[1..]`.
pub fn parse(args: &[String]) -> Result<Invocation> {
    let mut verbose = false;
    let mut sdk: Option<PathBuf> = None;
    let mut arch: Option<String> = None;
    let mut project: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut bundle_id: Option<String> = None;
    let mut dir: Option<PathBuf> = None;
    let mut force = false;
    let mut positional: Vec<String> = Vec::new();
    let mut bootstrap = BootstrapOptions::default();

    let mut index = 0usize;
    while index < args.len() {
        let arg = args[index].clone();
        // Support both `--flag value` and `--flag=value`.
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with('-') => (flag.to_string(), Some(value.to_string())),
            _ => (arg.clone(), None),
        };
        let take_value = |index: &mut usize| -> Result<String> {
            if let Some(value) = inline.clone() {
                return Ok(value);
            }
            *index += 1;
            args.get(*index)
                .cloned()
                .ok_or_else(|| Error::Usage(format!("{flag} needs a value")))
        };
        match flag.as_str() {
            "-v" | "--verbose" => verbose = true,
            "-h" | "--help" => return Ok(Invocation { command: Command::Help, verbose }),
            "--version" => return Ok(Invocation { command: Command::Version, verbose }),
            "--force" | "-f" => force = true,
            "--sdk" => sdk = Some(PathBuf::from(take_value(&mut index)?)),
            "--arch" => arch = Some(take_value(&mut index)?),
            "--project" | "-C" => project = Some(PathBuf::from(take_value(&mut index)?)),
            "-o" | "--output" => output = Some(PathBuf::from(take_value(&mut index)?)),
            "--bundle-id" => bundle_id = Some(take_value(&mut index)?),
            "--dir" => dir = Some(PathBuf::from(take_value(&mut index)?)),
            // --- bootstrap flags --------------------------------------------
            "--yes" | "-y" => bootstrap.yes = true,
            "--no-install" => bootstrap.no_install = true,
            "--dry-run" => bootstrap.dry_run = true,
            "--accept-sdk-license" => bootstrap.accept_sdk_license = true,
            "--sdk-version" => bootstrap.sdk_version = Some(take_value(&mut index)?),
            "--source" => bootstrap.source = Some(take_value(&mut index)?),
            other if other.starts_with('-') => {
                return Err(Error::Usage(format!("unknown option `{other}`")))
            }
            _ => positional.push(arg.clone()),
        }
        index += 1;
    }

let name = positional.first().cloned();
    let rest: Vec<String> = positional.into_iter().skip(1).collect();
    let command = match name.as_deref() {
        None => return Err(Error::Usage("no command given".to_string())),
        Some("help") => Command::Help,
        Some("bootstrap") | Some("setup") => {
            // `setup` is an alias, not a second implementation: the prompt asks
            // for exactly this so there is only ever one code path.
            if let Some(extra) = rest.first() {
                return Err(Error::Usage(format!(
                    "unexpected argument `{extra}` for `bootstrap` (did you mean a flag? \
                     see `darwinforge --help`)"
                )));
            }
            // A bare `bootstrap` is a change; `--dry-run` and `--no-install` are not.
            Command::Bootstrap(bootstrap)
        }
        Some("doctor") => {
            if let Some(extra) = rest.first() {
                return Err(Error::Usage(format!("unexpected argument `{extra}` for `doctor`")));
            }
            Command::Doctor { sdk }
        }
        Some("init") => {
            let name = rest
                .first()
                .cloned()
                .ok_or_else(|| Error::Usage("`init` needs a project name".to_string()))?;
            if let Some(extra) = rest.get(1) {
                return Err(Error::Usage(format!("unexpected argument `{extra}` for `init`")));
            }
            Command::Init { name, bundle_id, dir: dir.unwrap_or_else(|| PathBuf::from(".")), force }
        }
        Some("build") => {
            if let Some(extra) = rest.first() {
                return Err(Error::Usage(format!(
                    "unexpected argument `{extra}` for `build` (the project directory comes \
                     from --project, default: current directory)"
                )));
            }
            let arch = arch.unwrap_or_else(|| "arm64".to_string());
            if arch != "arm64" {
                return Err(Error::Usage(format!(
                    "unsupported --arch `{arch}`: this PoC targets arm64 iOS devices only"
                )));
            }
            Command::Build {
                sdk,
                arch,
                project: project.unwrap_or_else(|| PathBuf::from(".")),
                output,
            }
        }
        Some(other) => {
            return Err(Error::Usage(format!(
                "unknown command `{other}`; expected one of: \
                 bootstrap, doctor, init, build"
            )))
        }
    };
    Ok(Invocation { command, verbose })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn parses_doctor() {
        let invocation = parse(&args(&["doctor"])).expect("parses");
        assert!(matches!(invocation.command, Command::Doctor { sdk: None }));
        assert!(!invocation.verbose);
    }

    #[test]
    fn parses_build_flags_both_ways() {
        let invocation =
            parse(&args(&["build", "--sdk", "/sdk", "--arch", "arm64", "-o", "out.ipa", "-v"]))
                .expect("parses");
        match invocation.command {
            Command::Build { sdk, arch, output, .. } => {
                assert_eq!(sdk, Some(PathBuf::from("/sdk")));
                assert_eq!(arch, "arm64");
                assert_eq!(output, Some(PathBuf::from("out.ipa")));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(invocation.verbose);
    }

    #[test]
    fn parses_inline_values() {
        let invocation = parse(&args(&["build", "--sdk=/sdk", "--project=/p"])).expect("parses");
        match invocation.command {
            Command::Build { sdk, project, .. } => {
                assert_eq!(sdk, Some(PathBuf::from("/sdk")));
                assert_eq!(project, PathBuf::from("/p"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_init() {
        let invocation =
            parse(&args(&["init", "Hello", "--bundle-id", "com.acme.hi"])).expect("parses");
        match invocation.command {
            Command::Init { name, bundle_id, force, .. } => {
                assert_eq!(name, "Hello");
                assert_eq!(bundle_id.as_deref(), Some("com.acme.hi"));
                assert!(!force);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_flag_and_command() {
        assert!(parse(&args(&["build", "--nope"])).is_err());
        assert!(parse(&args(&["frobnicate"])).is_err());
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["init"])).is_err());
        assert!(parse(&args(&["build", "--sdk"])).is_err());
    }

    #[test]
    fn rejects_non_arm64_arch() {
        let error = parse(&args(&["build", "--arch", "x86_64"])).expect_err("must fail");
        assert!(error.to_string().contains("arm64"));
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert!(matches!(parse(&args(&["--help"])).expect("parses").command, Command::Help));
        assert!(matches!(parse(&args(&["-h"])).expect("parses").command, Command::Help));
        assert!(matches!(
            parse(&args(&["--version"])).expect("parses").command,
            Command::Version
        ));
    }

    #[test]
    fn a_bare_bootstrap_is_a_change_not_a_dry_run() {
        let invocation = parse(&args(&["bootstrap"])).expect("parses");
        let Command::Bootstrap(options) = invocation.command else {
            panic!("expected bootstrap");
        };
        assert_eq!(options, BootstrapOptions::default());
        assert!(!options.is_read_only(), "a bare bootstrap is allowed to change the machine");
        assert!(!options.yes, "and it may still prompt");
    }

    #[test]
    fn setup_is_an_alias_for_bootstrap_not_a_second_command() {
        let Command::Bootstrap(options) = parse(&args(&["setup"])).expect("parses").command
        else {
            panic!("setup must parse as bootstrap");
        };
        assert_eq!(options, BootstrapOptions::default());
    }

    #[test]
    fn bootstrap_flags_are_parsed_in_both_forms() {
        let inline =
            parse(&args(&["bootstrap", "--sdk-version=17.5", "--source=https://example.invalid/s"]))
                .expect("parses");
        let Command::Bootstrap(inline_options) = inline.command else { panic!("bootstrap") };
        assert_eq!(inline_options.sdk_version.as_deref(), Some("17.5"));
        assert_eq!(inline_options.source.as_deref(), Some("https://example.invalid/s"));

        let separate = parse(&args(&["bootstrap", "--sdk-version", "latest", "--source", "URL"]))
            .expect("parses");
        let Command::Bootstrap(separate_options) = separate.command else { panic!("bootstrap") };
        assert_eq!(separate_options.sdk_version.as_deref(), Some("latest"));
        assert_eq!(separate_options.source.as_deref(), Some("URL"));
    }

    #[test]
    fn every_documented_bootstrap_flag_parses() {
        let invocation = parse(&args(&[
            "bootstrap",
            "--yes",
            "--no-install",
            "--dry-run",
            "--accept-sdk-license",
            "--sdk-version",
            "latest",
        ]))
        .expect("parses");
        let Command::Bootstrap(options) = invocation.command else { panic!("bootstrap") };
        assert!(options.yes);
        assert!(options.no_install);
        assert!(options.dry_run);
        assert!(options.accept_sdk_license);
        assert_eq!(options.sdk_version.as_deref(), Some("latest"));
        assert!(options.is_read_only());
    }

    #[test]
    fn dry_run_and_no_install_are_both_read_only() {
        for extra in ["--dry-run", "--no-install"] {
            let Command::Bootstrap(options) =
                parse(&args(&["bootstrap", extra])).expect("parses").command
            else {
                panic!("bootstrap");
            };
            assert!(options.is_read_only(), "{extra} must promise to change nothing");
        }
        // A plain --yes does NOT make it read-only.
        let Command::Bootstrap(yes_only) =
            parse(&args(&["bootstrap", "--yes"])).expect("parses").command
        else {
            panic!("bootstrap");
        };
        assert!(!yes_only.is_read_only(), "--yes accepts defaults but still installs");
    }

    #[test]
    fn a_bootstrap_flag_needing_a_value_reports_it() {
        let error = parse(&args(&["bootstrap", "--sdk-version"])).expect_err("must fail");
        assert!(error.to_string().contains("--sdk-version"), "{error}");
        assert_eq!(error.exit_code(), 2, "a bad command line is exit 2");
    }

    #[test]
    fn bootstrap_rejects_stray_positional_arguments() {
        let error = parse(&args(&["bootstrap", "please"])).expect_err("must fail");
        assert!(error.to_string().contains("please"), "{error}");
    }

    #[test]
    fn the_unknown_command_error_lists_bootstrap() {
        let error = parse(&args(&["frobnicate"])).expect_err("must fail");
        let text = error.to_string();
        assert!(text.contains("bootstrap"), "must suggest the real commands: {text}");
    }
}
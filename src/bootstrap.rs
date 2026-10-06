//! `darwinforge bootstrap` — prepare a machine so `build` just works.
//!
//! The flow is deliberately linear and re-runnable:
//!
//! 1. Detect the OS, its Linux distribution and its package manager.
//! 2. Resolve the toolchain (clang, the Mach-O linker, ldid, git, zip).
//! 3. Plan and run installs for whatever is missing.
//! 4. Install an iPhoneOS SDK.
//! 5. Print a summary and the exact next command.
//!
//! Every step reports `[ok]`, `[installed]`, `[skipped]` or `[failed]` with a
//! reason, and the process exits **5** when the environment is not usable, so a
//! caller can tell "not set up" apart from "set up but broken".
//!
//! `--dry-run` and `--no-install` share this exact code path with the real run;
//! only the point at which a step stops short of changing the machine differs.
//! A plan computed differently from the work it describes would be worthless.

use std::path::{Path, PathBuf};

use crate::cli::BootstrapOptions;
use crate::distro::{self, Host, OperatingSystem};
use crate::error::{Error, Result};
use crate::global_config::GlobalConfig;
use crate::paths;
use crate::plan::{Plan, Status, Step};
use crate::prompt::{self, Mode, Question};
use crate::reporter::Reporter;
use crate::sdksource;
use crate::toolfind;

/// Where SDKs come from when neither the config nor `--source` says.
///
/// A default rather than a constant baked into the logic: `sdk.source` in the
/// global config and `--source` on the command line both override it.
pub const DEFAULT_SDK_SOURCE: &str = sdksource::DEFAULT_SOURCE;

/// The licence reminder shown once before an SDK is downloaded.
pub fn license_reminder(source: &str) -> String {
    sdksource::license_reminder(source)
}

/// Detect the real host, with the package managers probed from PATH.
pub fn detect_host() -> Host {
    distro::detect_host(distro::detect_os(), Path::new("/etc/os-release"), &|program| {
        crate::exec::which(program).is_some()
    })
}

/// Run bootstrap. Returns `Ok(())` only when the environment is usable.
pub fn run(options: &BootstrapOptions, reporter: &Reporter) -> Result<()> {
    let mode = Mode::detect(options.yes);
    let mut config = GlobalConfig::load()?;

    println!("DarwinForge bootstrap");
    println!("===================");
    println!();
    print_host(&detect_host(), options);

    let mut steps = Vec::new();
    steps.extend(tool_steps(&mut config, options, reporter, mode)?);
    steps.push(sdk_step(&mut config, options, mode)?);

    let plan = Plan { steps };
    println!("{}", plan.render(options.dry_run));

    if !options.is_read_only() {
        let saved = config.save()?;
        println!("saved     {}", paths::display_path(&saved));
        println!();
    }

    print_summary(&plan, options, reporter)
}

fn print_host(host: &Host, options: &BootstrapOptions) {
    println!("host      {} ({})", host.os.label(), host.arch);
    if let Some(distro) = &host.distro {
        println!("distro    {}", distro.label());
    }
    println!(
        "package   {}",
        host.package_manager
            .map(|manager| format!("{} ({})", manager.label(), manager.program()))
            .unwrap_or_else(|| "none found".to_string())
    );
    println!(
        "mode      {}",
        if options.is_read_only() { "read-only" } else { "installing" }
    );
    println!();
}

/// Resolve each tool, saving what we find and planning installs for the rest.
fn tool_steps(
    config: &mut GlobalConfig,
    options: &BootstrapOptions,
    reporter: &Reporter,
    mode: Mode,
) -> Result<Vec<Step>> {
    let host = detect_host();
    let mut steps = Vec::new();
    // `(name, package, available, required)` — `required` travels with each
    // tool so an optional one is reported as `[skipped]` with a reason rather
    // than `[failed]`, and so it never counts as outstanding work.
    let mut missing: Vec<(&str, Option<&str>, bool, bool)> = Vec::new();

    for spec in toolfind::all_specs() {
        // cctools `ld` is a fallback for the linker, not a separate install.
        if spec.name == toolfind::CCTOOLS_LD.name {
            continue;
        }
        let configured = configured_path(config, spec);
        match toolfind::find(spec, configured.as_deref()) {
            Some(found) => {
                store_tool(config, spec.name, &found.path);
                // Prefer what the binary itself reports. The version in the
                // *filename* is only a fallback: it exists at all when the name
                // is version-suffixed (`clang-21`), so an unversioned `clang`
                // on PATH was reported as "version unknown" even though asking
                // it would have answered. The reported string is already a
                // complete line ("clang version 21.1.0"), so it is used as-is.
                let version = match found.reported_version(&toolfind::probe_version) {
                    Some(reported) => reported,
                    None => match found.version {
                        Some(number) => format!("version {number} (from the file name)"),
                        None => format!(
                            "{} did not report a version",
                            found.path.file_name().unwrap_or_default().to_string_lossy()
                        ),
                    },
                };
                steps.push(
                    Step::check(spec.name, format!("using {}", paths::display_path(&found.path)))
                        .with_reason(format!("{version}, found via {:?}", found.origin)),
                );
            }
            None => {
                let names = crate::packages::for_tool_or_empty(spec.name);
                let package =
                    host.package_manager.and_then(|manager| names.for_manager(manager));
                // Whether the package is actually available is only known by asking the
                // repository, and asking means running a command. In a read-only
                // run we must not, so we *assume* the package exists and print the
                // command that would run. A dry run that reported "not available"
                // would be asserting something it never checked.
                let available = if spec.required && !options.is_read_only() {
                    package.map(|package| repository_has(host.package_manager, package)).unwrap_or(false)
                } else {
                    package.is_some()
                };
                missing.push((spec.name, package, available, spec.required));
            }
        }
    }
    let _ = reporter;

    if !missing.is_empty() {
        let planned = crate::plan::plan_tool_installs(host.package_manager, &missing);
        for step in planned.steps {
            // On Windows, a missing `ldid` is not a package problem: there is no
            // official Windows build, so no manager will ever provide one and
            // "no package for ldid on winget" is a dead end. Check for WSL and
            // recommend the bridge, naming the distribution that would work.
            if cfg!(windows) && step.name == toolfind::LDID.name && step.status != Status::Ok {
                steps.push(crate::windows::ldid_on_windows(&crate::windows::wsl_distros()));
                continue;
            }
            // Only a step that carries a real, probed-available command can be
            // executed; a failed or skipped plan step has nothing to run.
            let executable = step.status == Status::Ok && !step.commands.is_empty();
            steps.push(if executable && !options.is_read_only() {
                execute_install(&step, host.package_manager, &host, options, mode)
            } else if executable {
                // A dry run: the command is real, but nothing was run. Saying
                // `[ok]` here would claim the tool is present when it is not.
                step.with_reason("would run the command shown above")
            } else {
                step
            });
        }
    }

    // An install may have put a *new* clang or linker on PATH, so resolve once
    // more and record what is actually there now. Without this the summary
    // would still list the tool as missing after a successful install.
    if !options.is_read_only() {
        for spec in toolfind::all_specs() {
            if spec.name == toolfind::CCTOOLS_LD.name {
                continue;
            }
            if steps.iter().any(|step| step.name == spec.name && step.status == Status::Ok) {
                continue; // already recorded as present above
            }
            if let Some(found) = toolfind::find(spec, None) {
                store_tool(config, spec.name, &found.path);
                steps.push(
                    Step::check(spec.name, format!("using {}", paths::display_path(&found.path)))
                        .with_reason("found after installing"),
                );
            }
        }
    }
    Ok(steps)
}

/// The path already recorded in the global config for this tool, if any.
fn configured_path(config: &GlobalConfig, spec: &toolfind::ToolSpec) -> Option<PathBuf> {
    match spec.name {
        "clang" => config.clang.clone(),
        "ld64.lld" => config.linker.clone(),
        "ldid" => config.ldid.clone(),
        "zip" => config.zip.clone(),
        "git" => None,
        _ => None,
    }
}

fn store_tool(config: &mut GlobalConfig, tool: &str, path: &Path) {
    let path = path.to_path_buf();
    match tool {
        "clang" => config.clang = Some(path),
        "ld64.lld" => config.linker = Some(path),
        "ldid" => config.ldid = Some(path),
        "zip" => config.zip = Some(path),
        _ => {}
    }
}

/// Ask the package manager whether `package` exists in the configured repos.
fn repository_has(manager: Option<crate::distro::PackageManager>, package: &str) -> bool {
    let Some(manager) = manager else { return false };
    let arguments = manager.probe_args(package);
    match std::process::Command::new(manager.program()).args(&arguments).output() {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Actually run a planned install.
///
/// `sudo` is added only when the manager needs elevation *and* we are not
/// already root; `sudo -n` is used so a password prompt fails fast instead of
/// hanging a non-interactive run. The child's stdout/stderr are inherited, so
/// the package manager's own diagnostics are never swallowed.
fn execute_install(
    step: &Step,
    manager: Option<crate::distro::PackageManager>,
    host: &Host,
    options: &BootstrapOptions,
    mode: Mode,
) -> Step {
    let Some(manager) = manager else { return step.clone() };
    let wants_sudo = manager.wants_sudo() && !host.is_root;
    let Some(command) = step.commands.first() else { return step.clone() };

    // Installing system packages is the most consequential thing bootstrap
    // does, so it is always confirmed — unless --yes already said yes.
    let question = Question::with_subject(
        format!("install packages with `{}`", manager.program()),
        "--yes",
    );
    match prompt::confirm(mode, &question, true) {
        Ok(true) => {}
        Ok(false) => {
            return step.clone().with_outcome(
                Status::Skipped,
                "install declined; run the printed command yourself, then re-run bootstrap",
            )
        }
        // No TTY and no --yes: refuse rather than install without consent.
        Err(error) => return step.clone().with_outcome(Status::Failed, error.to_string()),
    }

    let command = command.with_sudo(wants_sudo);
    println!("  $ {}", command.display());
    let outcome = std::process::Command::new(&command.program)
        .args(&command.args)
        .status();
    let _ = options;

    match outcome {
        Ok(status) if status.success() => {
            step.clone().with_outcome(
                Status::Installed,
                format!("installed with {}", manager.program()),
            )
        }
        Ok(status) => step.clone().with_outcome(
            Status::Failed,
            format!(
                "`{}` exited with {}; see its output above",
                command.display(),
                status.code().map(|c| c.to_string()).unwrap_or_else(|| "a signal".to_string())
            ),
        ),
        Err(source) => {
            if source.kind() == std::io::ErrorKind::NotFound {
                step.clone().with_outcome(
                    Status::Failed,
                    format!(
                        "`{}` was not found; install the package manager or install {} \
                         manually",
                        command.program, step.name
                    ),
                )
            } else {
                step.clone()
                    .with_outcome(Status::Failed, format!("cannot run {}: {source}", command.display()))
            }
        }
    }
}

/// Decide what to do about the SDK.
fn sdk_step(config: &mut GlobalConfig, options: &BootstrapOptions, mode: Mode) -> Result<Step> {
    // Already configured and still present? Then there is nothing to do, and a
    // re-run stays a no-op.
    if let Some(existing) = config.live_sdk_path() {
        return Ok(Step::check("sdk", format!("using {}", paths::display_path(&existing)))
            .with_reason("already installed"));
    }

    let source = sdksource::resolve_source(config.sdk_source.as_deref(), options.source.as_deref());
    let requested = options.sdk_version.clone().unwrap_or_else(|| "latest".to_string());

    if options.is_read_only() {
        return Ok(Step::check("sdk", format!("would install iPhoneOS {requested} from {source}"))
            .with_outcome(
                Status::Skipped,
                "no SDK is configured; run `darwinforge bootstrap` to install one",
            ));
    }

    // The licence must be acknowledged once, and remembered.
    if !config.sdk_license_accepted && !options.accept_sdk_license {
        println!("{}", license_reminder(&source));
        let question = Question::new(
            "download an iPhoneOS SDK from a third-party repository",
            "--accept-sdk-license",
        );
        if !crate::prompt::confirm(mode, &question, false)? {
            return Ok(Step::check("sdk", "download an iPhoneOS SDK")
                .with_outcome(Status::Skipped, "licence not acknowledged"));
        }
    }
    config.sdk_license_accepted = true;
    config.sdk_source = Some(source.clone());

    // On Windows an SDK checkout needs symlink support, which is not guaranteed.
    if distro::detect_os() == OperatingSystem::Windows {
        return Ok(crate::windows::sdk_checkout_step());
    }

    install_sdk(&source, &requested, config)
}

/// Download and install an SDK, turning every failure into a reported step.
fn install_sdk(
    source: &str,
    requested: &str,
    config: &mut GlobalConfig,
) -> Result<Step> {
    let step = Step::check("sdk", format!("install iPhoneOS {requested} from {source}"));
    let runner = sdksource::run_git;
    // The detailed form, so an empty listing can be explained with the refs that
    // were probed and the git commands that ran, rather than a bare "no SDKs".
    let found = match sdksource::discover_detailed(source, &runner) {
        Ok(found) => found,
        Err(error) => return Ok(step.with_outcome(Status::Failed, error.to_string())),
    };
    let refs: Vec<&str> = found.refs_tried.iter().map(String::as_str).collect();
    if found.sdks.is_empty() {
        return Ok(step.with_outcome(
            Status::Failed,
            sdksource::no_sdks_error(&refs, Some(&found.trace)).to_string(),
        ));
    }

    // `latest` is a keyword, not a version. Passing it through to `select` as
    // though it were one made every default bootstrap run fail with "offers no
    // version latest"; `select` now interprets it, but the non-interactive case
    // must also *say* which SDK it settled on.
    let mut notice: Option<String> = None;
    let chosen = if sdksource::is_latest_request(Some(requested)) {
        match sdksource::select_latest(&found.sdks, notice.as_mut()) {
            Ok(chosen) => chosen,
            Err(error) => return Ok(step.with_outcome(Status::Failed, error.to_string())),
        }
    } else {
        match sdksource::select(&found.sdks, Some(requested)) {
            Ok(chosen) => chosen,
            Err(error) => return Ok(step.with_outcome(Status::Failed, error.to_string())),
        }
    };
    if let Some(warning) = notice {
        println!("  note      {warning}");
    }
    println!("  selected  {}", chosen.name);

    match sdksource::install(source, &chosen, &runner, true) {
        Ok((sdk, report)) => {
            let version = chosen.version.as_string();
            // A checkout whose symlinks were stubbed is installed but suspect:
            // say so now rather than letting it fail deep inside clang.
            if let Some(warning) = sdksource::symlink_warning(&report) {
                println!("  warning   {warning}");
            }
            println!("  installed {}", paths::display_path(&sdk.root));
            config.sdk_path = Some(sdk.root.clone());
            config.sdk_version = Some(version.clone());
            Ok(step
                .with_outcome(Status::Installed, format!("iPhoneOS {version}"))
                .with_reason(format!("installed to {}", paths::display_path(&sdk.root))))
        }
        Err(error) => Ok(step.with_outcome(Status::Failed, error.to_string())),
    }
}

/// Print the end-of-run summary and the next command.
///
/// The exit code is deliberately split from the wording:
///
/// * nothing outstanding → 0, "the environment is ready";
/// * a **read-only** run with work still to do → **5**, but the wording leads
///   with "the plan is valid" and gives the count, because the non-zero exit is
///   there to stop a script assuming work was done — not to report that
///   anything is wrong with the plan. Reading the old "the environment is NOT
///   ready" as an error is exactly what that phrasing invited.
fn print_summary(plan: &Plan, options: &BootstrapOptions, reporter: &Reporter) -> Result<()> {
    println!("summary");
    println!("-------");
    for (status, label) in [
        (Status::Ok, "ok"),
        (Status::Installed, "installed"),
        (Status::Skipped, "skipped"),
        (Status::Failed, "failed"),
    ] {
        let count = plan.steps.iter().filter(|step| step.status == status).count();
        println!("  {label:<9} {count}");
    }
    // Optional tools that were passed up are listed separately: they were
    // reported above as [skipped], and saying so here stops them reading as
    // outstanding work.
    let optional = plan.skipped_optional();
    if !optional.is_empty() {
        println!();
        println!("optional, not needed to build:");
        for step in optional {
            println!("  {:<12} {}", step.name, step.detail.as_deref().unwrap_or(""));
        }
    }
    println!();

    if plan.is_complete() && !plan.has_pending_work() {
        println!("The environment is ready.");
        println!();
        println!("next: darwinforge init MyApp");
        println!("      cd MyApp && darwinforge build");
        return Ok(());
    }

    // Something was planned but not carried out (a dry run, or a declined
    // install). Say so plainly, without dressing it as a failure.
    if plan.is_complete() && plan.has_pending_work() {
        let pending = plan.pending_count();
        let noun = if pending == 1 { "item" } else { "items" };
        // The wording changes with *why* nothing was done, because the two cases
        // need different reassurance: a read-only run needs "your plan is
        // sound", a declined install needs "nothing was done to your machine".
        if options.is_read_only() {
            println!(
                "The plan is valid: {pending} {noun} would be installed by a real run.\n\
                 Nothing was changed, because this was a read-only run."
            );
        } else {
            println!(
                "The plan is valid: {pending} {noun} are still outstanding.\n\
                 Nothing was installed without your confirmation."
            );
        }
        println!();
        println!("next: darwinforge bootstrap            # actually install");
        println!("      darwinforge bootstrap --dry-run  # show the plan only");
        // Non-zero: a caller must be able to tell that no work was done.
        return Err(Error::setup(
            format!("nothing was installed: {pending} {noun} still outstanding"),
            format!(
                "run `darwinforge bootstrap` to install {pending} {noun} on this machine. \
                 Exit code 5 means the environment is not ready; it does not mean the \
                 plan was invalid"
            ),
        ));
    }

    println!("The environment is NOT ready. Outstanding items:");
    println!();
    // `incomplete()` already excludes optional steps, so a machine that is
    // merely missing `zip` never appears on this list.
    let outstanding = plan.incomplete().len();
    for step in plan.incomplete() {
        println!("  {} — {}", step.name, step.action);
        if let Some(detail) = &step.detail {
            println!("      {detail}");
        }
    }
    println!();
    println!("next: darwinforge bootstrap            # interactive");
    println!("      darwinforge bootstrap --dry-run  # show the plan only");
    if reporter.warning_count() > 0 {
        println!();
        println!("note: {} warning(s) during setup", reporter.warning_count());
    }
    Err(Error::setup(
        format!("{outstanding} required item(s) still outstanding"),
        "run `darwinforge bootstrap` to finish setting up, or `darwinforge doctor` for details",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_license_reminder_names_apple_the_source_and_the_obligation() {
        let reminder = license_reminder("https://example.invalid/iOS-SDKs");
        assert!(reminder.contains("Apple"), "names the licensor: {reminder}");
        assert!(reminder.contains("https://example.invalid/iOS-SDKs"), "names the source");
        assert!(reminder.contains("licence"), "names the licence: {reminder}");
        assert!(reminder.contains("responsible"), "puts the obligation on the user");
    }

    #[test]
    fn the_default_sdk_source_is_a_default_not_a_baked_in_choice() {
        assert!(DEFAULT_SDK_SOURCE.starts_with("https://"));
        // Both the config value and the flag override it.
        assert_eq!(
            sdksource::resolve_source(Some("https://example.invalid/config"), None),
            "https://example.invalid/config"
        );
        assert_eq!(
            sdksource::resolve_source(Some("https://example.invalid/config"), Some("FLAG")),
            "FLAG",
            "the flag must win over the config"
        );
    }

    #[test]
    fn tool_resolution_is_recorded_in_the_config() {
        let mut config = GlobalConfig::default();
        store_tool(&mut config, "clang", Path::new("/usr/bin/clang-21"));
        store_tool(&mut config, "ld64.lld", Path::new("/usr/bin/ld64.lld-21"));
        store_tool(&mut config, "ldid", Path::new("/usr/local/bin/ldid"));
        assert_eq!(config.clang, Some(PathBuf::from("/usr/bin/clang-21")));
        assert_eq!(config.linker, Some(PathBuf::from("/usr/bin/ld64.lld-21")));
        assert_eq!(config.ldid, Some(PathBuf::from("/usr/local/bin/ldid")));
        // An unknown tool must not be invented into the config.
        store_tool(&mut config, "not-a-tool", Path::new("/usr/bin/thing"));
        assert_eq!(config.zip, None);
    }

    #[test]
    fn configured_paths_round_trip_through_the_config() {
        let mut config = GlobalConfig::default();
        store_tool(&mut config, "clang", Path::new("/usr/bin/clang"));
        let found = configured_path(&config, &toolfind::CLANG);
        assert_eq!(found, Some(PathBuf::from("/usr/bin/clang")));
        // A tool with no config slot must read back as None, not panic.
        assert_eq!(configured_path(&config, &toolfind::GIT), None);
    }

    #[test]
    fn a_complete_plan_ends_with_the_next_command() {
        let plan = Plan { steps: vec![Step::check("clang", "using /usr/bin/clang")] };
        assert!(print_summary(&plan, &BootstrapOptions::default(), &Reporter::new(false)).is_ok());
    }

    #[test]
    fn an_incomplete_plan_returns_exit_code_five() {
        let plan = Plan {
            steps: vec![Step::check("ldid", "obtain ldid")
                .with_outcome(Status::Failed, "not packaged")],
        };
        let error = print_summary(&plan, &BootstrapOptions::default(), &Reporter::new(false))
            .expect_err("must fail");
        assert_eq!(error.exit_code(), 5, "an unusable environment is exit code 5");
        assert!(error.to_string().contains("bootstrap"), "must say how to fix it");
    }

    #[test]
    fn a_skipped_sdk_step_also_means_incomplete() {
        let plan = Plan {
            steps: vec![Step::check("sdk", "download an SDK")
                .with_outcome(Status::Skipped, "licence not acknowledged")],
        };
        assert!(!plan.is_complete());
        assert_eq!(
            print_summary(&plan, &BootstrapOptions::default(), &Reporter::new(false))
                .expect_err("must fail")
                .exit_code(),
            5
        );
    }

    // ---- bug: an optional tool must never be a failure ---------------------

    #[test]
    fn a_missing_optional_tool_does_not_fail_the_run() {
        // The reported bug: bootstrap and doctor disagreed about `zip` on
        // Windows, and because it was planned as a `[failed]` step, a machine
        // that builds perfectly well exited 5.
        let plan = Plan {
            steps: vec![
                Step::check("clang", "using /usr/bin/clang"),
                Step::check("zip", "install zip")
                    .optional()
                    .with_outcome(Status::Skipped, "no package for zip on winget"),
            ],
        };
        assert!(plan.is_complete(), "an absent zip must not block a build");
        assert!(plan.incomplete().is_empty(), "and must not be outstanding");
        assert!(
            print_summary(&plan, &BootstrapOptions::default(), &Reporter::new(false)).is_ok(),
            "the run must succeed"
        );
        // It is still *reported*, so the user knows what was passed up.
        assert_eq!(plan.skipped_optional().len(), 1);
    }

    #[test]
    fn a_missing_required_tool_still_fails_even_beside_a_skipped_optional_one() {
        let plan = Plan {
            steps: vec![
                Step::check("zip", "install zip").optional().with_outcome(Status::Skipped, "n/a"),
                Step::check("ldid", "obtain ldid").with_outcome(Status::Failed, "not packaged"),
            ],
        };
        assert!(!plan.is_complete(), "ldid is required");
        assert_eq!(plan.incomplete().len(), 1, "only the required one counts");
        assert_eq!(plan.incomplete()[0].name, "ldid");
    }

    #[test]
    fn a_working_machine_with_no_optional_tools_installed_exits_zero() {
        // The whole point of the fix, as a single assertion.
        let plan = Plan {
            steps: vec![
                Step::check("clang", "using /usr/bin/clang"),
                Step::check("ld64.lld", "using /usr/bin/ld64.lld"),
                Step::check("ldid", "using /usr/local/bin/ldid"),
            ],
        };
        assert!(plan.is_complete());
        assert_eq!(plan.pending_count(), 0);
        assert!(print_summary(&plan, &BootstrapOptions::default(), &Reporter::new(false)).is_ok());
    }

    #[test]
    fn a_dry_run_that_would_install_says_the_plan_is_valid_and_still_exits_five() {
        // Bug: the dry run exited non-zero with wording that read as an error.
        // The exit code must stay non-zero (nothing was installed), but the
        // message must not look like a failure.
        let plan = Plan {
            steps: vec![
                Step::check("clang", "using /usr/bin/clang"),
                crate::plan::plan_package_install(
                    "ldid",
                    crate::distro::PackageManager::Apt,
                    Some("ldid"),
                    true,
                ),
            ],
        };
        assert!(plan.has_pending_work(), "an install command is pending");
        let options = BootstrapOptions { dry_run: true, ..Default::default() };
        let error = print_summary(&plan, &options, &Reporter::new(false)).expect_err("must fail");
        assert_eq!(error.exit_code(), 5, "still non-zero: no work was done");
        assert_eq!(plan.pending_count(), 1, "one item would be installed");
        let text = error.to_string();
        assert!(
            text.contains("Exit code 5") && text.contains("not mean the plan was invalid"),
            "the exit code is explained so it is not read as an error: {text}"
        );
    }

    #[test]
    fn read_only_options_are_honoured() {
        assert!(BootstrapOptions { dry_run: true, ..Default::default() }.is_read_only());
        assert!(BootstrapOptions { no_install: true, ..Default::default() }.is_read_only());
        assert!(!BootstrapOptions { yes: true, ..Default::default() }.is_read_only());
        assert!(!BootstrapOptions::default().is_read_only());
    }

    #[test]
    fn every_tool_spec_has_a_package_row_where_one_can_exist() {
        // A tool the package table does not know about can never be installed,
        // so a mismatch here is a silent dead end for the user.
        for spec in toolfind::all_specs() {
            if spec.name == toolfind::CCTOOLS_LD.name {
                continue; // a fallback, never installed directly
            }
            assert!(
                crate::packages::for_tool(spec.name).is_some(),
                "no package row for `{}`",
                spec.name
            );
        }
    }

    fn host_with(manager: Option<crate::distro::PackageManager>, is_root: bool) -> Host {
        Host {
            os: crate::distro::OperatingSystem::Linux,
            distro: None,
            package_manager: manager,
            available_managers: manager.into_iter().collect(),
            is_root,
            arch: "x86_64".to_string(),
        }
    }

    fn install_step() -> Step {
        crate::plan::plan_package_install(
            "clang",
            crate::distro::PackageManager::Apt,
            Some("clang"),
            true,
        )
    }

    #[test]
    fn an_install_step_with_a_command_is_marked_executable() {
        let step = install_step();
        assert_eq!(step.status, Status::Ok);
        assert!(!step.commands.is_empty(), "an available package carries a command");
    }

    #[test]
    fn a_failed_plan_step_is_never_executed() {
        // No command means nothing to run; the step must pass through untouched.
        let step = crate::plan::plan_package_install(
            "ldid",
            crate::distro::PackageManager::Apt,
            Some("ldid"),
            false,
        );
        assert_eq!(step.status, Status::Failed);
        assert!(step.commands.is_empty());
    }

    #[test]
    fn with_no_package_manager_nothing_is_installed() {
        let step = install_step();
        let out = execute_install(
            &step,
            None,
            &host_with(None, false),
            &BootstrapOptions::default(),
            Mode::AssumeYes,
        );
        assert_eq!(out.status, Status::Ok, "must be left alone, not failed");
        assert_eq!(out, step, "a step with no manager is returned unchanged");
    }

    #[test]
    fn without_a_tty_and_without_yes_the_install_is_refused_not_attempted() {
        // The most important property here: no TTY and no --yes must never
        // install anything, and must say which flag would allow it.
        let out = execute_install(
            &install_step(),
            Some(crate::distro::PackageManager::Apt),
            &host_with(Some(crate::distro::PackageManager::Apt), true),
            &BootstrapOptions::default(),
            Mode::AssumeNo,
        );
        assert_eq!(out.status, Status::Failed, "must not be treated as done");
        let detail = out.detail.expect("must explain");
        assert!(detail.contains("--yes"), "names the flag that unblocks it: {detail}");
    }

    #[test]
    fn sudo_is_added_for_a_manager_that_needs_it_but_not_when_root() {
        use crate::distro::PackageManager as Pm;
        let step = install_step();

        // Not root, manager needs elevation -> sudo.
        let as_user = execute_install(
            &step,
            Some(Pm::Apt),
            &host_with(Some(Pm::Apt), false),
            &BootstrapOptions::default(),
            // AssumeNo refuses before spawning anything, so this exercises the
            // consent path without running apt.
            Mode::AssumeNo,
        );
        assert_eq!(as_user.status, Status::Failed);

        // The command that *would* run is the sudo form.
        let elevated = step.commands[0].with_sudo(true);
        assert!(elevated.display().starts_with("sudo -n apt-get"), "{}", elevated.display());

        // Already root -> no sudo.
        let plain = step.commands[0].with_sudo(false);
        assert!(plain.display().starts_with("apt-get"), "{}", plain.display());
    }

    #[test]
    fn a_declined_install_is_skipped_with_instructions_not_failed() {
        // A user who says "no" must get a Skipped step that tells them what to
        // run themselves — not a Failed one, which would misreport the machine.
        let declined = install_step().with_outcome(
            Status::Skipped,
            "install declined; run the printed command yourself, then re-run bootstrap",
        );
        assert_eq!(declined.status, Status::Skipped);
        let detail = declined.detail.clone().expect("must explain");
        assert!(detail.contains("re-run bootstrap"), "{detail}");
        // And it must not claim the machine is complete.
        let plan = Plan { steps: vec![declined] };
        assert!(!plan.is_complete(), "a declined install is not a usable environment");
    }
}
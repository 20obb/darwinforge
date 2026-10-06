//! `darwinforge doctor` — report what is present, what is missing, and how to fix it.

use std::path::Path;

use crate::error::{Error, Result};
use crate::exec;
use crate::global_config::GlobalConfig;
use crate::paths;
use crate::prompt::{self, Mode};
use crate::reporter::Reporter;

/// One line of the doctor report.
struct Check {
    name: &'static str,
    status: &'static str,
    detail: String,
    fix: Option<String>,
}

/// Run the environment check. Returns an error only when a required tool is
/// missing, so `doctor` can be used as a CI gate.
pub fn run(sdk_arg: Option<&Path>, fix: bool, yes: bool, reporter: &Reporter) -> Result<()> {
    let _ = reporter;
    println!("darwinforge doctor — checking this machine for the iOS toolchain");
    println!();

    let mut checks: Vec<Check> = Vec::new();
    let mut missing_required = 0usize;

    match exec::resolve("CLANG", "clang") {
        Some(path) => {
            let version = exec::probe("clang", &["--version".to_string()])
                .ok()
                .flatten()
                .unwrap_or_else(|| "version unknown".to_string());
            checks.push(ok("clang (compiler)", &format!("{}\n{version}", path.display())));
        }
        None => {
            missing_required += 1;
            checks.push(missing(
                "clang (compiler)",
                "not found on PATH",
                "install LLVM/clang (`apt install clang`, `dnf install clang`) or set \
                 DARWINFORGE_CLANG=/path/to/clang",
            ));
        }
    }

    match exec::resolve("LINKER", "ld64.lld") {
        Some(path) => checks.push(ok("ld64.lld (Mach-O linker)", &path.display().to_string())),
        None => match exec::which("ld") {
            Some(path) => {
                // A bare `ld` on PATH is often MinGW/GNU ld, which cannot link
                // Mach-O at all. Detect that rather than failing deep in the
                // link step with a baffling "unknown emulation" error.
                let banner = exec::probe("ld", &["--version".to_string()])
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let mach_o = banner.contains("LLVM")
                    || banner.contains("cctools")
                    || banner.contains("ld64")
                    || banner.contains("PROJECT: ld64");
                if mach_o {
                    checks.push(ok(
                        "ld (cctools linker, fallback)",
                        &format!("{} — ld64.lld preferred but absent\n{banner}", path.display()),
                    ));
                } else {
                    missing_required += 1;
                    checks.push(Check {
                        name: "Mach-O linker (ld64.lld or ld)",
                        status: "MISSING",
                        detail: format!(
                            "found `ld` at {} but it is not a Mach-O linker ({})",
                            path.display(),
                            banner.lines().next().unwrap_or("version unknown")
                        ),
                        fix: Some(
                            "install lld (`apt install lld`), whose `ld64.lld` links \
                             Mach-O, or cctools-port for Linux; or set \
                             DARWINFORGE_LINKER=/path/to/ld64.lld"
                                .to_string(),
                        ),
                    });
                }
            }
            None => {
                missing_required += 1;
                checks.push(missing(
                    "Mach-O linker (ld64.lld or ld)",
                    "neither ld64.lld nor ld found on PATH",
                    "install lld (`apt install lld`), which ships ld64.lld, or \
                     cctools-port; or set DARWINFORGE_LINKER=/path/to/ld64.lld",
                ));
            }
        },
    }

    match exec::resolve("LDID", "ldid") {
        Some(path) => {
            let version =
                exec::probe("ldid", &["-v".to_string()]).ok().flatten().unwrap_or_default();
            let detail = if version.is_empty() {
                path.display().to_string()
            } else {
                format!("{}\n{version}", path.display())
            };
            checks.push(ok("ldid (ad-hoc signer)", &detail));
        }
        None => {
            missing_required += 1;
            checks.push(missing(
                "ldid (ad-hoc signer)",
                "not found on PATH",
                "install ldid (https://github.com/ProcursusTeam/ldid or your package \
                 manager); or set DARWINFORGE_LDID=/path/to/ldid",
            ));
        }
    }

match exec::which("zip") {
        Some(path) => checks.push(ok(
            "zip (optional external packager)",
            &format!("{} — only used when build.zip = true", path.display()),
        )),
        None => checks.push(Check {
            name: "zip (optional external packager)",
            status: "ok",
            detail: "not installed — the built-in zip writer will be used".to_string(),
            fix: None,
        }),
    }

    match exec::resolve("SWIFTC", "swiftc") {
        Some(path) => checks.push(ok(
            "swiftc (optional)",
            &format!("{} — Swift sources will be compiled", path.display()),
        )),
        None => checks.push(Check {
            name: "swiftc (optional)",
            status: "ok",
            detail: "not installed — Swift sources will be rejected with a clear \
                     \"not supported yet\" error; C/ObjC/C++ projects are fine"
                .to_string(),
            fix: None,
        }),
    }

    // The SDK is resolved through the *same* function `build` uses, so the two
    // commands can never disagree. The reported bug was that `doctor` looked
    // only at `--sdk` and the environment while `bootstrap` had saved an SDK in
    // the global config — so `doctor` reported a missing SDK on a machine that
    // could build perfectly well.
    let sources = crate::sdkpath::SdkSources::from_environment(sdk_arg.map(Path::to_path_buf))
        .with_config();
    let mut sdk_error = match crate::sdkpath::resolve_sdk(&sources) {
        Ok(resolved) => {
            let version = resolved.version.clone().unwrap_or_else(|| "unknown".to_string());
            checks.push(ok(
                "iPhoneOS SDK",
                &format!(
                    "{} (version {version}, from {})",
                    resolved.root.display(),
                    resolved.origin.label()
                ),
            ));
            None
        }
        Err(error) => {
            checks.push(missing(
                "iPhoneOS SDK",
                &error.to_string(),
                "run `darwinforge sdk install` to fetch one, `darwinforge sdk` to choose \
                 one, or pass --sdk /path/to/iPhoneOS17.5.sdk for this run only",
            ));
            Some(error)
        }
    };
    if sdk_error.is_some() {
        missing_required += 1;
    }

    if fix {
        let fixed_sdk = repair_config_and_sdk(sdk_arg, yes, &mut sdk_error, &mut checks)?;
        if fixed_sdk {
            missing_required -= 1;
        }
    }

    for check in &checks {
        println!("[{}] {}", check.status, check.name);
        for line in check.detail.lines() {
            println!("       {line}");
        }
        if let Some(fix) = &check.fix {
            println!("       fix: {fix}");
        }
    }

    if fix {
        let tool_missing = checks
            .iter()
            .any(|check| check.status == "MISSING" && check.name != "iPhoneOS SDK");
        if tool_missing {
            println!("[fix] missing tools are installed by `darwinforge bootstrap`; doctor --fix only applies SDK/config fixes.");
        }
    }

    println!();
    if missing_required == 0 {
        println!(
            "All required components are present. Run `darwinforge init <name>` then \
             `darwinforge build --sdk <path>`."
        );
        return Ok(());
    }
    Err(Error::Prereq {
        what: format!("{missing_required} required component(s) missing"),
        fix: "see the `fix:` lines above".to_string(),
    })
}

/// Apply only the fixes `doctor` can make safely: a broken global config and a
/// missing/stale SDK. Missing compilers and linkers are pointed at `bootstrap`.
fn repair_config_and_sdk(
    sdk_arg: Option<&Path>,
    yes: bool,
    sdk_error: &mut Option<Error>,
    checks: &mut Vec<Check>,
) -> Result<bool> {
    let mode = Mode::detect(yes);

    if let Err(Error::Config { path, .. }) = GlobalConfig::load() {
        println!("[fix] the global config is not valid TOML; it will be moved aside.");
        match mode {
            Mode::AssumeNo => {
                println!("       needs confirmation: run `darwinforge doctor --fix --yes` to apply.");
            }
            Mode::AssumeYes => {
                backup_config(&path)?;
                println!("       backed up to {}", paths::display_path(&backup_path(&path)));
            }
            Mode::Interactive => {
                let question = prompt::Question::with_subject(
                    format!("move {} aside?", paths::display_path(&path)),
                    "--yes",
                );
                if prompt::confirm(Mode::Interactive, &question, false)? {
                    backup_config(&path)?;
                    println!("       backed up to {}", paths::display_path(&backup_path(&path)));
                } else {
                    println!("       declined; the config file was left untouched.");
                }
            }
        }
    }

    let mut fixed = false;
    if sdk_error.is_some() {
        let fresh = crate::sdkpath::SdkSources::from_environment(sdk_arg.map(Path::to_path_buf))
            .with_config();
        if let Ok(resolved) = crate::sdkpath::resolve_sdk(&fresh) {
            *sdk_error = None;
            replace_sdk_check(checks, &resolved);
            return Ok(true);
        }
    }

    let installed = crate::sdkpath::installed_sdks();
    if sdk_error.is_some() && !installed.is_empty() {
        let newest = installed.last().expect("non-empty");
        match mode {
            Mode::AssumeNo => {
                println!(
                    "[fix] an installed SDK exists but is not active: \
                     `darwinforge sdk use {}`",
                    paths::display_path(newest)
                );
            }
            Mode::AssumeYes => {
                crate::sdkcmd::use_sdk(&newest.to_string_lossy())?;
                if let Ok(resolved) = crate::sdkpath::resolve_sdk(
                    &crate::sdkpath::SdkSources::from_environment(sdk_arg.map(Path::to_path_buf))
                        .with_config(),
                ) {
                    *sdk_error = None;
                    replace_sdk_check(checks, &resolved);
                    fixed = true;
                }
            }
            Mode::Interactive => {
                let question = prompt::Question::with_subject(
                    format!("activate the newest installed SDK {}?", paths::display_path(newest)),
                    "--yes",
                );
                if prompt::confirm(Mode::Interactive, &question, true)? {
                    crate::sdkcmd::use_sdk(&newest.to_string_lossy())?;
                    if let Ok(resolved) = crate::sdkpath::resolve_sdk(
                        &crate::sdkpath::SdkSources::from_environment(sdk_arg.map(Path::to_path_buf))
                            .with_config(),
                    ) {
                        *sdk_error = None;
                        replace_sdk_check(checks, &resolved);
                        fixed = true;
                    }
                } else {
                    println!("[fix] declined; run `darwinforge sdk use ...` to choose one yourself.");
                }
            }
        }
    }

    if sdk_error.is_some() && installed.is_empty() {
        match mode {
            Mode::AssumeNo => {
                println!("[fix] no SDK is installed; run `darwinforge sdk install` to fetch one.");
            }
            Mode::AssumeYes => {
                crate::sdkcmd::install(None, None, true, true)?;
                if let Ok(resolved) = crate::sdkpath::resolve_sdk(
                    &crate::sdkpath::SdkSources::from_environment(sdk_arg.map(Path::to_path_buf))
                        .with_config(),
                ) {
                    *sdk_error = None;
                    replace_sdk_check(checks, &resolved);
                    fixed = true;
                }
            }
            Mode::Interactive => {
                let question = prompt::Question::with_subject(
                    "install the newest available iPhoneOS SDK now?".to_string(),
                    "--yes",
                );
                if prompt::confirm(Mode::Interactive, &question, false)? {
                    crate::sdkcmd::install(None, None, yes, true)?;
                    if let Ok(resolved) = crate::sdkpath::resolve_sdk(
                        &crate::sdkpath::SdkSources::from_environment(sdk_arg.map(Path::to_path_buf))
                            .with_config(),
                    ) {
                        *sdk_error = None;
                        replace_sdk_check(checks, &resolved);
                        fixed = true;
                    }
                } else {
                    println!("[fix] declined; run `darwinforge sdk install` when ready.");
                }
            }
        }
    }

    Ok(fixed)
}

fn backup_path(path: &Path) -> std::path::PathBuf {
    let mut text = path.as_os_str().to_os_string();
    text.push(".bak");
    std::path::PathBuf::from(text)
}

fn backup_config(path: &Path) -> Result<()> {
    let backup = backup_path(path);
    std::fs::rename(path, &backup).map_err(|source| {
        Error::io(format!("cannot move {} aside", path.display()), source)
    })
}

fn replace_sdk_check(checks: &mut Vec<Check>, resolved: &crate::sdkpath::SdkResolution) {
    let version = resolved.version.clone().unwrap_or_else(|| "unknown".to_string());
    let detail = format!(
        "{} (version {version}, from {})",
        resolved.root.display(),
        resolved.origin.label()
    );
    if let Some(check) = checks.iter_mut().find(|check| check.name == "iPhoneOS SDK") {
        check.status = "ok";
        check.detail = detail;
        check.fix = None;
    } else {
        checks.push(ok("iPhoneOS SDK", &detail));
    }
}

fn ok(name: &'static str, detail: &str) -> Check {
    Check { name, status: "ok", detail: detail.to_string(), fix: None }
}

fn missing(name: &'static str, detail: &str, fix: &str) -> Check {
    Check {
        name,
        status: "MISSING",
        detail: detail.to_string(),
        fix: Some(fix.to_string()),
    }
}
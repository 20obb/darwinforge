//! `darwinforge doctor` — report what is present, what is missing, and how to fix it.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::exec;
use crate::reporter::Reporter;
use crate::sdk::Sdk;

/// One line of the doctor report.
struct Check {
    name: &'static str,
    status: &'static str,
    detail: String,
    fix: Option<String>,
}

/// Run the environment check. Returns an error only when a required tool is
/// missing, so `doctor` can be used as a CI gate.
pub fn run(sdk_arg: Option<&Path>, reporter: &Reporter) -> Result<()> {
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

    let sdk_path = sdk_arg
        .map(Path::to_path_buf)
        .or_else(|| crate::compat::env_var("SDK").map(PathBuf::from).filter(|p| p.is_dir()));
    match sdk_path {
        Some(path) => match Sdk::open(&path) {
            Ok(sdk) => {
                let version = sdk.version.clone().unwrap_or_else(|| "unknown".to_string());
                checks.push(ok(
                    "iPhoneOS SDK",
                    &format!("{} (version {version})", sdk.root.display()),
                ));
            }
            Err(error) => {
                missing_required += 1;
                checks.push(missing(
                    "iPhoneOS SDK",
                    &error.to_string(),
                    "pass --sdk /path/to/iPhoneOS17.4.sdk (extracted by you); darwinforge \
                     never downloads or redistributes Apple's SDK",
                ));
            }
        },
        None => checks.push(Check {
            name: "iPhoneOS SDK",
            status: "ok",
            detail: "not checked — no --sdk given and DARWINFORGE_SDK is unset".to_string(),
            fix: Some(
                "pass --sdk /path/to/iPhoneOS17.4.sdk to `doctor` or `build`; you must \
                 supply the SDK yourself"
                    .to_string(),
            ),
        }),
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
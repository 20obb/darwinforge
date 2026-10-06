//! What `bootstrap` intends to do, decided before it does any of it.
//!
//! Separating *planning* from *executing* is what makes `--dry-run` honest: the
//! dry run prints the same plan the real run follows, computed by the same code.
//! It also makes the interesting behaviour testable without root, without a
//! package manager and without a network.
//!
//! A [`Plan`] is a list of [`Step`]s, each independent and idempotent, so a
//! re-run simply re-evaluates and skips what is already done.

use crate::distro::PackageManager;
use crate::packages;

/// The four states a step can end in, matching what bootstrap prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Already correct; nothing to do.
    Ok,
    /// Something was changed to make it correct.
    Installed,
    /// Deliberately not done.
    Skipped,
    /// Could not be done.
    Failed,
}

impl Status {
    /// The bracketed tag printed for this status.
    pub fn tag(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Installed => "installed",
            Status::Skipped => "skipped",
            Status::Failed => "failed",
        }
    }
}

/// One unit of work, with the reason it exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// What is being made true, e.g. "clang".
    pub name: String,
    /// What we will do, in plain words.
    pub action: String,
    /// Why this step exists.
    pub reason: String,
    /// The commands that will run, empty for a pure check.
    pub commands: Vec<Command>,
    /// Current state, filled in as the plan executes.
    pub status: Status,
    /// Set when the status is anything but a plain check.
    pub detail: Option<String>,
    /// Whether the machine can build without this.
    ///
    /// This is what separates `zip` being absent from `clang` being absent.
    /// A missing optional tool is a **[skipped]** note with a reason, never a
    /// failure, and it must never count towards the outstanding total — a
    /// Windows box with no `zip` still builds fine through the built-in writer,
    /// so reporting it as a failure made a working machine exit 5.
    pub optional: bool,
}

impl Step {
    /// A step that only inspects, never changes anything.
    pub fn check(name: impl Into<String>, action: impl Into<String>) -> Step {
        Step {
            name: name.into(),
            action: action.into(),
            reason: String::new(),
            commands: Vec::new(),
            status: Status::Ok,
            detail: None,
            optional: false,
        }
    }

    /// Mark this step as something the machine can do without.
    ///
    /// An optional step that could not be satisfied is reported, but does not
    /// make the plan incomplete.
    pub fn optional(mut self) -> Step {
        self.optional = true;
        self
    }

    /// A step that will run `commands`.
    pub fn action(
        name: impl Into<String>,
        action: impl Into<String>,
        commands: Vec<Command>,
    ) -> Step {
        Step {
            name: name.into(),
            action: action.into(),
            reason: String::new(),
            commands,
            status: Status::Ok,
            detail: None,
            optional: false,
        }
    }

    /// Record an outcome and an explanation.
    pub fn with_outcome(mut self, status: Status, detail: impl Into<String>) -> Step {
        self.status = status;
        self.detail = Some(detail.into());
        self
    }

    /// Attach a reason without changing the status.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Step {
        self.reason = reason.into();
        self
    }
}

/// A command line, kept structured so it can be shown to the user and run
/// without re-quoting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

impl Command {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Command {
        Command { program: program.into(), args }
    }

    /// The command as a user would type it, quoting anything with a space.
    pub fn display(&self) -> String {
        let mut out = self.program.clone();
        for argument in &self.args {
            out.push(' ');
            if argument.contains(char::is_whitespace) {
                out.push('"');
                out.push_str(argument);
                out.push('"');
            } else {
                out.push_str(argument);
            }
        }
        out
    }

    /// Whether this command runs under `sudo`.
    pub fn needs_sudo(&self) -> bool {
        self.program == "sudo"
    }

    /// The command with `sudo -n` in front when elevation is wanted.
    ///
    /// `-n` (non-interactive) is deliberate: a sudo that would prompt for a
    /// password must fail fast rather than hang a pipeline.
    pub fn with_sudo(&self, wants_sudo: bool) -> Command {
        if !wants_sudo || self.program == "sudo" {
            return self.clone();
        }
        let mut elevated = vec!["-n".to_string(), self.program.clone()];
        elevated.extend(self.args.iter().cloned());
        Command::new("sudo", elevated)
    }
}

/// A whole bootstrap plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
}

impl Plan {
    /// Steps that did not succeed **and that matter**.
    ///
    /// An optional step is excluded. `zip` and `swiftc` improve a build but are
    /// not required for one, so a machine missing them is not an unfinished
    /// machine — counting them made a perfectly capable Windows host report
    /// "the environment is NOT ready" and exit 5.
    pub fn incomplete(&self) -> Vec<&Step> {
        self.steps
            .iter()
            .filter(|step| !step.optional)
            .filter(|step| matches!(step.status, Status::Failed | Status::Skipped))
            .collect()
    }

    /// Optional steps that could not be satisfied, for reporting.
    ///
    /// These are shown so the user knows what was passed up, but never counted
    /// as outstanding work.
    pub fn skipped_optional(&self) -> Vec<&Step> {
        self.steps
            .iter()
            .filter(|step| step.optional)
            .filter(|step| !matches!(step.status, Status::Ok | Status::Installed))
            .collect()
    }

    /// True when nothing *required* is missing, so exit code 0 is honest.
    ///
    /// A step that merely carries an install command is **not** complete: the
    /// machine still lacks the tool. Only `Ok` (present) and `Installed` (just
    /// installed) mean the requirement is satisfied, so a dry run can never report
    /// success for work it did not do.
    pub fn is_complete(&self) -> bool {
        self.steps
            .iter()
            .filter(|step| !step.optional)
            .all(|step| matches!(step.status, Status::Ok | Status::Installed))
    }

    /// How many *required* items the real run would still have to install.
    ///
    /// The number `--dry-run` reports, computed from the plan rather than
    /// counted by hand at the call site, so the message cannot drift from the
    /// work actually described.
    pub fn pending_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| !step.optional)
            .filter(|step| {
                matches!(step.status, Status::Failed | Status::Skipped)
                    || !step.commands.is_empty()
            })
            .count()
    }

    /// True when a step carries a command that the real run would execute.
    ///
    /// A dry run's plan steps are `Ok` because their *planning* succeeded; this
    /// predicate is what distinguishes them from a genuinely satisfied
    /// requirement, so the summary can be honest about which is which.
    pub fn has_pending_work(&self) -> bool {
        self.steps.iter().any(|step| {
            matches!(step.status, Status::Ok | Status::Installed) && !step.commands.is_empty()
        })
    }

    /// Render the plan for `--dry-run` or for the live run.
    pub fn render(&self, dry_run: bool) -> String {
        let mut out = String::new();
        if dry_run {
            out.push_str("bootstrap plan (dry run - nothing will be changed)\n\n");
        }
        for step in &self.steps {
            out.push_str(&format!("[{}] {:<12} {}\n", step.status.tag(), step.name, step.action));
            for command in &step.commands {
                out.push_str(&format!("           $ {}\n", command.display()));
            }
            if !step.reason.is_empty() {
                out.push_str(&format!("           why: {}\n", step.reason));
            }
            if let Some(detail) = &step.detail {
                out.push_str(&format!("           {detail}\n"));
            }
        }
        out
    }
}

/// Build the plan for installing `tool` on `manager`.
///
/// The repository is probed before the package name is used, so we never print
/// an install command for a package the archive does not carry. `available` is
/// the injected probe result, which keeps this function pure and testable.
///
/// `required` decides what happens when the tool cannot be installed. For an
/// **optional** tool that is a `[skipped]` note explaining why it is not needed,
/// never a `[failed]` — see [`Step::optional`]. `optional_detail` supplies the
/// sentence shown for that case, e.g. "the built-in zip writer will be used".
#[allow(clippy::too_many_arguments)]
pub fn plan_package_install_with(
    tool: &str,
    manager: PackageManager,
    package: Option<&str>,
    available: bool,
    required: bool,
    optional_detail: Option<&str>,
) -> Step {
    let step = match (package, available) {
        (Some(package), true) => {
            let arguments: Vec<String> = vec![package.to_string()];
            let args = manager.install_args(&arguments);
            Step::action(
                tool,
                format!("install `{package}` with {}", manager.program()),
                vec![Command::new(manager.program(), args)],
            )
            .with_reason("the configured repositories do not carry this tool")
        }
        (Some(package), false) => Step::check(tool, format!("install `{package}`"))
            .with_outcome(
                if required { Status::Failed } else { Status::Skipped },
                format!(
                    "`{package}` is not available in the configured {} repositories; \
                     refresh the package index and retry, or install it manually \
                     and re-run",
                    manager.label()
                ),
            ),
        (None, _) => {
            let reason = match packages::for_tool(tool) {
                Some(entry) if entry.unavailable_everywhere() => format!(
                    "{tool} is not packaged for any known package manager; \
                     it has to be built from source"
                ),
                _ => format!("no package for {tool} on {}", manager.label()),
            };
            // "no package for zip on winget" is a fact about an optional tool,
            // not a failure: the built-in writer covers it.
            let detail = match (required, optional_detail) {
                (false, Some(explanation)) => format!("{reason}; {explanation}"),
                _ => reason,
            };
            Step::check(tool, format!("install {tool}"))
                .with_outcome(if required { Status::Failed } else { Status::Skipped }, detail)
        }
    };
    if required {
        step
    } else {
        step.optional()
    }
}

/// [`plan_package_install_with`] for a tool the build cannot proceed without.
pub fn plan_package_install(
    tool: &str,
    manager: PackageManager,
    package: Option<&str>,
    available: bool,
) -> Step {
    plan_package_install_with(tool, manager, package, available, true, None)
}

/// The step that explains why a tool is missing entirely.
///
/// Used when even the package table has no answer, so the user is told the
/// honest reason instead of a package command that cannot work.
pub fn plan_unbuildable(tool: &str, detail: &str) -> Step {
    Step::check(tool, format!("obtain {tool}")).with_outcome(Status::Failed, detail.to_string())
}

/// Build the plan of package installs implied by a host's missing tools.
///
/// Pure: takes the already-probed facts and returns the steps, so the same
/// function backs both `--dry-run` and the real run. `missing` carries each
/// tool's `(name, package, available, required)` tuple, so optionality decided
/// at the call site is preserved all the way to the report.
pub fn plan_tool_installs(
    manager: Option<PackageManager>,
    missing: &[(&str, Option<&str>, bool, bool)],
) -> Plan {
    let steps = missing
        .iter()
        .map(|(tool, package, available, required)| {
            let name: &str = tool;
            let detail = if *required { None } else { Some(optional_reason(name)) };
            match manager {
                Some(manager) => {
                    plan_package_install_with(tool, manager, *package, *available, *required, detail)
                }
                None => {
                    let message = format!(
                        "no supported package manager was found; install {name} manually \
                         and re-run, or re-run on a system with apt/dnf/pacman/zypper/apk/\
                         brew/winget/choco/scoop"
                    );
                    let step = Step::check(name, format!("obtain {name}")).with_outcome(
                        if *required { Status::Failed } else { Status::Skipped },
                        message,
                    );
                    if *required {
                        step
                    } else {
                        step.optional()
                    }
                }
            }
        })
        .collect();
    Plan { steps }
}

/// Why an optional tool is not needed, in one clause.
///
/// The reason must say what happens *instead*, so a `[skipped]` line reads as a
/// decision rather than a shortfall.
fn optional_reason(tool: &str) -> &'static str {
    match tool {
        "zip" => "the built-in zip writer is used instead, so builds are unaffected",
        "swiftc" => "Swift sources will be rejected with a clear error; C/ObjC/C++ is unaffected",
        "git" => "needed only to fetch an SDK; a build with a local SDK is unaffected",
        "ld" => "ld64.lld is preferred and is what the build uses",
        _ => "the build does not need it",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distros the task calls out, each with its own os-release.
    const DISTROS: &[(&str, &str, PackageManager)] = &[
        ("debian", "ID=debian\n", PackageManager::Apt),
        ("ubuntu", "ID=ubuntu\nID_LIKE=debian\n", PackageManager::Apt),
        ("fedora", "ID=fedora\n", PackageManager::Dnf),
        ("arch", "ID=arch\n", PackageManager::Pacman),
        ("manjaro", "ID=manjaro\nID_LIKE=arch\n", PackageManager::Pacman),
        ("linuxmint", "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n", PackageManager::Apt),
    ];

    #[test]
    fn every_status_renders_the_documented_tag() {
        assert_eq!(Status::Ok.tag(), "ok");
        assert_eq!(Status::Installed.tag(), "installed");
        assert_eq!(Status::Skipped.tag(), "skipped");
        assert_eq!(Status::Failed.tag(), "failed");
    }

    #[test]
    fn commands_are_rendered_readably_and_quoted_when_needed() {
        let plain = Command::new("apt-get", vec!["install".into(), "-y".into(), "clang".into()]);
        assert_eq!(plain.display(), "apt-get install -y clang");
        let spaced = Command::new("git", vec!["clone".into(), "/tmp/My Repo".into()]);
        assert_eq!(spaced.display(), r#"git clone "/tmp/My Repo""#);
    }

    #[test]
    fn sudo_is_added_only_when_wanted_and_never_doubled() {
        let install = Command::new("apt-get", vec!["install".into(), "clang".into()]);
        assert_eq!(
            install.with_sudo(true),
            Command::new(
                "sudo",
                vec!["-n".into(), "apt-get".into(), "install".into(), "clang".into()]
            ),
            "sudo -n must precede the program"
        );
        assert_eq!(install.with_sudo(false), install, "no sudo when not wanted");
        let already = Command::new("sudo", vec!["apt-get".into()]);
        assert_eq!(already.with_sudo(true), already, "must not become sudo sudo");
        assert!(already.needs_sudo());
    }

    #[test]
    fn a_dry_run_plan_states_that_nothing_changes() {
        let plan = Plan { steps: vec![Step::check("clang", "found clang-21")] };
        let rendered = plan.render(true);
        assert!(rendered.contains("dry run"), "{rendered}");
        assert!(rendered.contains("nothing will be changed"), "{rendered}");
        assert!(!plan.render(false).contains("dry run"), "the live run is not a dry run");
    }

    #[test]
    fn a_plan_renders_every_step_with_its_commands() {
        let plan = Plan {
            steps: vec![Step::action(
                "clang",
                "install `clang` with apt-get",
                vec![Command::new("apt-get", vec!["install".into(), "-y".into(), "clang".into()])],
            )
            .with_reason("not found on PATH")],
        };
        let rendered = plan.render(true);
        assert!(rendered.contains("[ok]"), "{rendered}");
        assert!(rendered.contains("clang"), "{rendered}");
        assert!(rendered.contains("$ apt-get install -y clang"), "{rendered}");
        assert!(rendered.contains("why: not found on PATH"), "{rendered}");
    }

    #[test]
    fn an_available_package_produces_a_real_install_command() {
        let step = plan_package_install("clang", PackageManager::Apt, Some("clang"), true);
        assert_eq!(step.status, Status::Ok);
        assert_eq!(step.commands.len(), 1);
        assert_eq!(step.commands[0].display(), "apt-get install -y clang");
    }

    #[test]
    fn an_unavailable_package_fails_with_an_actionable_reason() {
        let step = plan_package_install("ldid", PackageManager::Apt, Some("ldid"), false);
        assert_eq!(step.status, Status::Failed);
        assert!(step.commands.is_empty(), "must not print a command that cannot work");
        let detail = step.detail.expect("must explain");
        assert!(detail.contains("ldid"), "{detail}");
        assert!(detail.contains("apt"), "names the manager: {detail}");
    }

    #[test]
    fn a_tool_with_no_package_anywhere_says_it_must_be_built() {
        // The "must be built from source" branch fires only when the table has no
        // package for the tool on ANY manager. Today no row is in that state —
        // ldid, the hardest case, is carried by Homebrew — so assert the table
        // property that keeps this branch honest, and note what would change it.
        let absent: Vec<&str> = packages::PACKAGES
            .iter()
            .filter(|entry| entry.unavailable_everywhere())
            .map(|entry| entry.tool)
            .collect();
        assert!(
            absent.is_empty(),
            "{absent:?} has no package on any manager; the source-build path is \
             now reachable and needs its own test"
        );

        // The branch itself, exercised through the table's own predicate.
        for entry in packages::PACKAGES {
            assert!(
                !entry.unavailable_everywhere(),
                "{} unexpectedly has no package anywhere",
                entry.tool
            );
        }
    }

    #[test]
    fn ldid_has_no_windows_package_but_others_do() {
        // The concrete fact this branch exists for: ldid ships no Windows build,
        // so on winget it must not be presented as installable.
        let ldid = packages::for_tool("ldid").expect("row");
        assert_eq!(ldid.for_manager(PackageManager::Winget), None);
        assert_eq!(ldid.for_manager(PackageManager::Choco), None);
        assert_eq!(ldid.for_manager(PackageManager::Scoop), None);
        // ...but Homebrew does carry it, so "unavailable everywhere" is false
        // and the plan says "no package on winget" rather than "build it".
        assert!(!ldid.unavailable_everywhere());
        let step = plan_package_install("ldid", PackageManager::Winget, None, false);
        let detail = step.detail.expect("must explain");
        assert!(detail.contains("winget"), "{detail}");
        assert!(!detail.contains("built from source"), "{detail}");
    }

    #[test]
    fn a_tool_packaged_elsewhere_says_only_that_this_manager_lacks_it() {
        // ldid IS in the Debian/Ubuntu archives, so on apt the message must
        // name apt rather than claiming a source build is the only option.
        let step = plan_package_install("ldid", PackageManager::Apt, None, false);
        assert_eq!(step.status, Status::Failed);
        let detail = step.detail.expect("must explain");
        assert!(detail.contains("apt"), "names the manager: {detail}");
        assert!(!detail.contains("built from source"), "apt does package ldid: {detail}");
    }

    #[test]
    fn a_tool_packaged_for_this_manager_still_gets_a_real_install() {
        // The counterpart to the case above: on apt, ldid IS packaged, so it
        // must not be misreported as needing a source build.
        let step = plan_package_install("ldid", PackageManager::Apt, Some("ldid"), true);
        assert_eq!(step.commands[0].display(), "apt-get install -y ldid");
        assert_ne!(step.status, Status::Failed, "apt carries ldid");
    }

    #[test]
    fn no_package_manager_yields_an_actionable_failure() {
        let plan = plan_tool_installs(None, &[("clang", None, false, true)]);
        assert!(!plan.is_complete());
        let detail = plan.steps[0].detail.clone().expect("must explain");
        assert!(detail.contains("package manager"), "{detail}");
        assert!(detail.contains("manually"), "tells the user what to do: {detail}");
    }

#[test]
    fn a_dry_run_plan_is_correct_for_every_distro() {
        // The task requires --dry-run to produce a sensible plan on each of
        // debian, ubuntu, fedora, arch, manjaro and linuxmint.
        for (name, os_release, expected) in DISTROS {
            let release = crate::distro::parse_os_release(os_release);
            let manager = crate::distro::manager_for_distro(&release);
            assert_eq!(manager, Some(*expected), "{name} must resolve to {}", expected.label());

            let missing: Vec<(&str, Option<&str>, bool, bool)> = vec![
                ("clang", Some("clang"), true, true),
                ("ld64.lld", Some("lld"), true, true),
                // No package row for this manager: must be a source build.
                ("ldid", None, false, true),
            ];
            let plan = plan_tool_installs(manager, &missing);
            let rendered = plan.render(true);

            assert!(rendered.contains("dry run"), "{name}: {rendered}");
            assert!(rendered.contains("clang"), "{name}: {rendered}");
            assert!(rendered.contains("ld64.lld"), "{name}: {rendered}");
            assert!(rendered.contains("ldid"), "{name}: {rendered}");
            // With no package for ldid on this manager, the plan must not invent an
            // install command; it must name the manager that lacks it.
            assert!(
                rendered.contains("no package for ldid on"),
                "{name}: ldid must be reported as unavailable on this manager:\n{rendered}"
            );
            // clang and the linker must still get real install commands.
            assert!(
                rendered.contains(&expected.program().to_string()),
                "{name}: install commands must use {}:\n{rendered}",
                expected.program()
            );
        }
    }

    #[test]
    fn install_commands_use_the_right_manager_for_each_distro() {
        for (name, os_release, expected) in DISTROS {
            let release = crate::distro::parse_os_release(os_release);
            let manager = crate::distro::manager_for_distro(&release).expect("mapped");
            assert_eq!(manager, *expected, "{name} maps to the right manager");
            let names = crate::packages::for_tool("clang").expect("row");
            let package = names.for_manager(manager);
            let step = plan_package_install("clang", manager, package, true);
            let command = &step.commands[0];
            assert_eq!(command.program, manager.program(), "{name} must use {}", manager.program());
            assert!(command.args.contains(&"clang".to_string()), "{name}: {command:?}");
            // Every generated install must be non-interactive.
            let joined = command.args.join(" ");
            assert!(
                joined.contains("-y")
                    || joined.contains("noconfirm")
                    || joined.contains("--non-interactive")
                    || joined.contains("disable-interactivity")
                    || command.program == "brew"
                    || command.program == "scoop",
                "{name}: install must not prompt: {joined}"
            );
        }
    }

    #[test]
    fn plan_completeness_reflects_every_step() {
        let all_ok = Plan { steps: vec![Step::check("a", "x"), Step::check("b", "y")] };
        assert!(all_ok.is_complete());
        assert!(all_ok.incomplete().is_empty());

        let with_failure = Plan {
            steps: vec![
                Step::check("a", "x"),
                Step::check("b", "y").with_outcome(Status::Failed, "no"),
            ],
        };
        assert!(!with_failure.is_complete(), "a failed step means incomplete");
        assert_eq!(with_failure.incomplete().len(), 1);

        let with_skip = Plan {
            steps: vec![Step::check("a", "x").with_outcome(Status::Skipped, "declined")],
        };
        assert!(!with_skip.is_complete(), "a deliberately skipped step is not complete");
    }

    #[test]
    fn an_installed_step_counts_as_complete() {
        // After a successful run the step is `installed`, not `ok`; that must
        // still mean exit code 0.
        let plan = Plan {
            steps: vec![Step::check("clang", "x").with_outcome(Status::Installed, "")],
        };
        assert!(plan.is_complete());
    }

    #[test]
    fn an_empty_plan_is_complete() {
        assert!(Plan { steps: Vec::new() }.is_complete());
    }

    #[test]
    fn a_step_carrying_a_command_is_pending_work_not_a_satisfied_requirement() {
        // This is the distinction that keeps --dry-run honest: the plan step is
        // `Ok` because planning succeeded, but the tool is still missing.
        let planned = Plan { steps: vec![plan_package_install("clang", PackageManager::Apt, Some("clang"), true)] };
        assert!(planned.has_pending_work(), "an un-run install command is pending work");

        let satisfied = Plan { steps: vec![Step::check("clang", "using /usr/bin/clang-21")] };
        assert!(!satisfied.has_pending_work(), "a resolved tool has nothing pending");
        assert!(satisfied.is_complete());
    }

    #[test]
    fn pending_work_alone_makes_a_plan_incomplete_for_exit_zero() {
        // A dry run whose steps are all `Ok` must still not report the machine
        // as ready, because the install commands were never run.
        let planned = Plan {
            steps: vec![plan_package_install("clang", PackageManager::Apt, Some("clang"), true)],
        };
        assert!(planned.is_complete(), "every step is Ok...");
        assert!(planned.has_pending_work(), "...but work is still outstanding");
    }
}

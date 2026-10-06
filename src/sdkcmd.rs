//! `darwinforge sdk` — inspect, install, select and remove SDKs.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::cli::{SdkAction, SdkOptions};
use crate::error::{Error, Result};
use crate::global_config::GlobalConfig;
use crate::paths;
use crate::prompt::{self, Mode};
use crate::sdk;
use crate::sdkpath;
use crate::sdksource;

/// Run `darwinforge sdk ...`.
pub fn run(options: &SdkOptions, action: &SdkAction) -> Result<()> {
    match action {
        SdkAction::Picker => picker(options),
        SdkAction::List { filter } => list(options, filter.as_deref(), false),
        SdkAction::Install { requested } => install(
            requested.as_deref(),
            options.source.as_deref(),
            options.yes,
            true,
        ),
        SdkAction::Use { target } => use_sdk(target),
        SdkAction::Remove { requested } => remove(requested, options.yes),
        SdkAction::Path => path(),
    }
}

/// Print the SDK table; optionally treat the process as a list rather than a
/// picker after a non-TTY invocation.
fn list(options: &SdkOptions, filter: Option<&str>, markdown: bool) -> Result<()> {
    let config = GlobalConfig::load()?;
    let source = source_for(options, &config);
    let remote = discover(&source)?;
    let installed = installed();
    let active = active_root();
    if markdown {
        println!("SDK\tinstalled\tactive");
    }
    for row in rows(&remote.sdks, filter, &installed, active.as_deref()) {
        if markdown {
            println!(
                "{}\t{}\t{}",
                row.remote.name,
                if row.installed { "yes" } else { "no" },
                if row.active { "yes" } else { "no" }
            );
        } else {
            let mut suffix = String::new();
            if row.installed {
                suffix.push_str(" [installed]");
            }
            if row.active {
                suffix.push_str(" [active]");
            }
            println!("  {} ({}){}", row.remote.name, row.remote.version, suffix);
        }
    }
    if remote.sdks.is_empty() {
        println!("No iPhoneOS SDKs found in {}", source);
    }
    Ok(())
}

/// Interactive picker. Without a TTY this prints the list and exits successfully,
/// rather than blocking on a prompt.
fn picker(options: &SdkOptions) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        println!("Installed and available device SDKs (newest first):");
        list(options, None, false)?;
        println!();
        println!("Interactive selection needs a TTY.");
        println!("Use `darwinforge sdk install <version>`, `darwinforge sdk use <version|path>`,");
        println!("or `darwinforge sdk path` instead.");
        return Ok(());
    }
    println!("Available device SDKs (newest first):");
    let config = GlobalConfig::load()?;
    let source = source_for(options, &config);
    let remote = discover(&source)?;
    let installed = installed();
    let active = active_root();
    let mut filtered = rows(&remote.sdks, None, &installed, active.as_deref());
    if filtered.is_empty() {
        println!("No iPhoneOS SDKs found in {}", source);
        return Ok(());
    }
    print_rows(&filtered);
    println!();
    println!("Type a version prefix (for example 17) to filter, or press Enter for all.");
    let prefix = read_line("filter: ")?;
    let prefix = prefix.trim();
    filtered = rows(&remote.sdks, (!prefix.is_empty()).then_some(prefix), &installed, active.as_deref());
    if filtered.is_empty() {
        println!("No SDK versions match `{prefix}`.");
        return Ok(());
    }
    print_rows(&filtered);
    println!();
    println!("Type a number to install/use it, or press Enter to quit.");
    let choice = read_line("choice: ")?;
    let choice = choice.trim();
    if choice.is_empty() {
        return Ok(());
    }
    let index = match choice.parse::<usize>() {
        Ok(number) if (1..=filtered.len()).contains(&number) => number - 1,
        _ => {
            println!("`{choice}` is not one of the choices; nothing changed.");
            return Ok(());
        }
    };
    let row = &filtered[index];
    if row.installed {
        if row.active {
            println!("{} is already active.", row.remote.name);
            return Ok(());
        }
        let path = installed.iter().find(|installed| version_matches(installed, &row.remote.version.as_string()));
        if let Some(path) = path {
            return use_path(path);
        }
        return Err(Error::Prereq {
            what: format!("{} is marked installed but its directory cannot be found", row.remote.name),
            fix: "run `darwinforge sdk list` to refresh the installation state".to_string(),
        });
    }
    let reminder = sdksource::license_reminder(&source);
    println!("{reminder}");
    let question = prompt::Question::with_subject(
        format!("install and activate {}?", row.remote.name),
        "--yes",
    );
    if !prompt::confirm(Mode::Interactive, &question, false)? {
        println!("declined; nothing was installed.");
        return Ok(());
    }
    install_selected(&source, row.remote, true, true)
}

/// `darwinforge sdk install [version]`.
pub fn install(
    requested: Option<&str>,
    source: Option<&str>,
    yes: bool,
    activate: bool,
) -> Result<()> {
    let config = GlobalConfig::load()?;
    let source = source.map(str::to_string).unwrap_or_else(|| source_for(
        &SdkOptions { source: None, yes },
        &config,
    ));
    let remote = discover(&source)?;
    let mut notice_text: Option<String> = None;
    let selected = if sdksource::is_latest_request(requested) {
        sdksource::select_latest(&remote.sdks, notice_text.as_mut())?
    } else {
        sdksource::select(&remote.sdks, requested)?
    };
    if let Some(notice) = notice_text {
        println!("{notice}");
    }
    println!("Installing {} from {}", selected, source);
    install_selected(&source, &selected, yes || config.sdk_license_accepted, activate)
}

fn install_selected(
    source: &str,
    selected: &sdksource::RemoteSdk,
    license_accepted: bool,
    activate: bool,
) -> Result<()> {
    let reminder = sdksource::license_reminder(source);
    if !license_accepted {
        // `install` repeats the refusal with the reminder; printing it here lets
        // the user satisfy it in the same command with `--yes`.
        println!("{reminder}");
    }
    let runner = sdksource::run_git;
    let (sdk, report) = sdksource::install(source, selected, &runner, license_accepted)?;
    println!("Installed to {}", paths::display_path(&sdk.root));
    if let Some(warning) = sdksource::symlink_warning(&report) {
        println!("warning: {warning}");
    }
    let mut config = GlobalConfig::load()?;
    config.sdk_path = Some(sdk.root.clone());
    config.sdk_version = sdk.version;
    config.sdk_source = Some(source.to_string());
    config.sdk_license_accepted = true;
    if activate {
        config.save()?;
        println!("Active SDK: {}", paths::display_path(&sdk.root));
    } else {
        config.save()?;
    }
    Ok(())
}

fn discover(source: &str) -> Result<sdksource::Discovery> {
    let runner = sdksource::run_git;
    sdksource::discover_detailed(source, &runner)
}

/// `darwinforge sdk use <path|version>`.
pub fn use_sdk(target: &str) -> Result<()> {
    let path = Path::new(target);
    if path.exists() {
        let _ = sdk::Sdk::open(path)?;
        return use_path(path);
    }
    let installed = installed();
    let mut candidates: Vec<PathBuf> = Vec::new();
    for path in &installed {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if version_matches(path, target) || name == target {
            candidates.push(path.clone());
        }
    }
    match candidates.len() {
        0 => Err(Error::Prereq {
            what: format!("`{target}` is neither an existing SDK path nor an installed version"),
            fix: "run `darwinforge sdk list` to see installed SDKs, or `darwinforge sdk install {target}`"
                .to_string(),
        }),
        1 => use_path(&candidates[0]),
        _ => {
            candidates.sort();
            use_path(candidates.last().expect("non-empty"))
        }
    }
}

fn use_path(path: &Path) -> Result<()> {
    let sdk = sdk::Sdk::open(path)?;
    let mut config = GlobalConfig::load()?;
    config.sdk_path = Some(path.to_path_buf());
    config.sdk_version = sdk.version;
    config.save()?;
    println!("Active SDK: {}", paths::display_path(path));
    Ok(())
}

/// `darwinforge sdk remove <version>`.
fn remove(requested: &str, yes: bool) -> Result<()> {
    let installed = installed();
    let candidates: Vec<PathBuf> = installed
        .into_iter()
        .filter(|path| version_matches(path, requested))
        .collect();
    let path = match candidates.as_slice() {
        [] => {
            return Err(Error::Prereq {
                what: format!("no installed SDK matches `{requested}`"),
                fix: "run `darwinforge sdk list` to see installed versions".to_string(),
            })
        }
        [one] => one,
        many => {
            return Err(Error::Usage(format!(
                "`{requested}` matches {} installed SDKs; use the exact version: {}",
                many.len(),
                many.iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
    };
    if !paths::is_managed_dir(path) {
        return Err(Error::Prereq {
            what: format!("refusing to remove {}", path.display()),
            fix: "only SDKs installed under darwinforge's managed sdks directory can be removed"
                .to_string(),
        });
    }
    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err(Error::Setup {
                what: format!("removing {} needs confirmation", path.display()),
                fix: format!("run `darwinforge sdk remove {} --yes`", requested),
            });
        }
        let question = prompt::Question::with_subject(
            format!("remove {}?", path.display()),
            "--yes",
        );
        if !prompt::confirm(Mode::Interactive, &question, false)? {
            println!("declined; nothing was removed.");
            return Ok(());
        }
    }
    std::fs::remove_dir_all(path).map_err(|source| {
        Error::io(format!("cannot remove {}", path.display()), source)
    })?;
    let mut config = GlobalConfig::load()?;
    if config.sdk_path.as_deref() == Some(path) {
        config.sdk_path = None;
        config.sdk_version = None;
        config.save()?;
    }
    println!("removed {}", paths::display_path(path));
    Ok(())
}

/// `darwinforge sdk path`.
fn path() -> Result<()> {
    let sources = sdkpath::SdkSources::from_environment(None).with_config();
    let (resolution, _) = sdkpath::resolve_and_open(&sources)?;
    println!("{}", paths::display_path(&resolution.root));
    Ok(())
}

#[derive(Debug)]
struct Row<'a> {
    remote: &'a sdksource::RemoteSdk,
    installed: bool,
    active: bool,
}

fn rows<'a>(
    remote: &'a [sdksource::RemoteSdk],
    filter: Option<&str>,
    installed: &[PathBuf],
    active: Option<&Path>,
) -> Vec<Row<'a>> {
    remote
        .iter()
        .filter(|sdk| match filter {
            Some(filter) => sdk.version.as_string().starts_with(filter)
                || sdk.name.starts_with(filter),
            None => true,
        })
        .map(|sdk| Row {
            remote: sdk,
            installed: installed.iter().any(|path| version_matches(path, &sdk.version.as_string())),
            active: active.is_some_and(|root| {
                installed
                    .iter()
                    .any(|path| same_path(path, root) && version_matches(path, &sdk.version.as_string()))
            }),
        })
        .collect()
}

fn print_rows(rows: &[Row<'_>]) {
    for (index, row) in rows.iter().enumerate() {
        let mut suffix = String::new();
        if row.installed {
            suffix.push_str(" [installed]");
        }
        if row.active {
            suffix.push_str(" [active]");
        }
        println!("  {}) {} ({}){}", index + 1, row.remote.name, row.remote.version, suffix);
    }
}

fn installed() -> Vec<PathBuf> {
    sdkpath::installed_sdks()
}

fn active_root() -> Option<PathBuf> {
    let sources = sdkpath::SdkSources::from_environment(None).with_config();
    sdkpath::resolve_sdk(&sources).ok().map(|resolution| resolution.root)
}

fn version_matches(path: &Path, requested: &str) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == requested || name.starts_with(requested)
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn source_for(options: &SdkOptions, config: &GlobalConfig) -> String {
    options
        .source
        .clone()
        .or_else(|| config.sdk_source.clone())
        .unwrap_or_else(|| sdksource::DEFAULT_SOURCE.to_string())
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Ok(_) => Ok(line),
        Err(error) => Err(Error::io("cannot read from standard input", error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_sdk(version: &str) -> PathBuf {
        let parent = std::env::temp_dir().join(format!(
            "darwinforge-sdkcmd-{}-{version}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&parent);
        let root = parent.join(version);
        std::fs::create_dir_all(root.join("usr/include")).expect("include");
        std::fs::create_dir_all(root.join("usr/lib")).expect("lib");
        std::fs::create_dir_all(root.join("System/Library/Frameworks")).expect("frameworks");
        root
    }

    #[test]
    fn the_version_filter_matches_directory_names() {
        let path = temp_sdk("17.5");
        assert!(version_matches(&path, "17"));
        assert!(version_matches(&path, "17.5"));
        assert!(!version_matches(&path, "18"));
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn rows_mark_installed_and_active() {
        let sdks = sdksource::parse_tree_listing("iPhoneOS17.5.sdk\niPhoneOS18.0.sdk\n");
        let installed = temp_sdk("17.5");
        let active = installed.clone();
        let rows = rows(&sdks, Some("17"), std::slice::from_ref(&installed), Some(&active));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].installed);
        assert!(rows[0].active);
        let _ = std::fs::remove_dir_all(installed);
    }

    #[test]
    fn a_source_override_wins_over_the_saved_one() {
        let options = SdkOptions { source: Some("flag".to_string()), yes: false };
        let config = GlobalConfig { sdk_source: Some("config".to_string()), ..Default::default() };
        assert_eq!(source_for(&options, &config), "flag");
    }
}

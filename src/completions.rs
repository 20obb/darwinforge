//! Hand-written shell completion scripts.

use crate::error::{Error, Result};

/// Return the completion script for `shell`.
pub fn for_shell(shell: &str) -> Result<String> {
    match shell {
        "bash" => Ok(BASH.to_string()),
        "zsh" => Ok(ZSH.to_string()),
        "fish" => Ok(FISH.to_string()),
        "powershell" | "pwsh" => Ok(POWERSHELL.to_string()),
        other => Err(Error::Usage(format!(
            "unknown completion shell `{other}`; expected bash, zsh, fish, or powershell"
        ))),
    }
}

const BASH: &str = r#"_darwinforge() {
    local cur prev words cword
    _init_completion || return
    local commands="bootstrap doctor init build sdk clean completions"
    local sdk_cmds="list install use remove path"
    local shells="bash zsh fish powershell"
    case "${words[1]}" in
        bootstrap)
            COMPREPLY=( $(compgen -W "--yes --no-install --sdk-version --accept-sdk-license --dry-run --source -v -h --version" -- "$cur") )
            ;;
        doctor)
            COMPREPLY=( $(compgen -W "--sdk --fix --yes -v -h --version" -- "$cur") )
            ;;
        init)
            COMPREPLY=( $(compgen -W "--bundle-id --dir --force --here -v -h --version" -- "$cur") )
            ;;
        build)
            COMPREPLY=( $(compgen -W "--sdk --arch --project -o --output -v -h --version" -- "$cur") )
            ;;
        sdk)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=( $(compgen -W "$sdk_cmds --yes --source -v -h --version" -- "$cur") )
            else
                COMPREPLY=( $(compgen -W "--yes --source -v -h --version" -- "$cur") )
            fi
            ;;
        clean)
            COMPREPLY=( $(compgen -W "--dry-run -v -h --version" -- "$cur") )
            ;;
        completions)
            COMPREPLY=( $(compgen -W "$shells" -- "$cur") )
            ;;
        *)
            COMPREPLY=( $(compgen -W "$commands -v -h --help --version" -- "$cur") )
            ;;
    esac
}
complete -F _darwinforge darwinforge
"#;

const ZSH: &str = r#"#compdef darwinforge

_darwinforge() {
    local -a commands sdk_cmds shells
    commands=(bootstrap doctor init build sdk clean completions)
    sdk_cmds=(list install use remove path)
    shells=(bash zsh fish powershell)

    if (( CURRENT == 2 )); then
        _describe 'command' commands
        return
    fi

    case "$words[2]" in
        bootstrap)
            _arguments '--yes[accept defaults]' '--no-install[do not change anything]' \
                '--sdk-version[SDK version]:version:' '--accept-sdk-license[accept SDK licence]' \
                '--dry-run[print the plan only]' '--source[SDK repository]:url:' \
                '-v[verbose]' '-h[help]' '--version[version]'
            ;;
        doctor)
            _arguments '--sdk[SDK path]:path:_files' '--fix[apply safe fixes]' '--yes[answer yes]' \
                '-v[verbose]' '-h[help]' '--version[version]'
            ;;
        init)
            _arguments '--bundle-id[bundle id]:id:' '--dir[parent directory]:path:_files' \
                '--force[overwrite]' '--here[write current project config]' \
                '-v[verbose]' '-h[help]' '--version[version]'
            ;;
        build)
            _arguments ':project path:_files' '--sdk[SDK path]:path:_files' \
                '--arch[architecture]:arch:(arm64)' '--project[project directory]:path:_files' \
                '-o[output]:file:_files' '--output[output]:file:_files' \
                '-v[verbose]' '-h[help]' '--version[version]'
            ;;
        sdk)
            if (( CURRENT == 3 )); then
                _describe 'sdk command' sdk_cmds
            else
                _arguments '--yes[answer yes]' '--source[SDK repository]:url:' \
                    '-v[verbose]' '-h[help]' '--version[version]'
            fi
            ;;
        clean)
            _arguments ':project path:_files' '--dry-run[print what would be removed]' \
                '-v[verbose]' '-h[help]' '--version[version]'
            ;;
        completions)
            _describe 'shell' shells
            ;;
    esac
}

_darwinforge "$@"
"#;

const FISH: &str = r#"complete -c darwinforge -f
complete -c darwinforge -n '__fish_use_subcommand' -a 'bootstrap' -d 'Prepare this machine'
complete -c darwinforge -n '__fish_use_subcommand' -a 'doctor' -d 'Check the environment'
complete -c darwinforge -n '__fish_use_subcommand' -a 'init' -d 'Create or describe a project'
complete -c darwinforge -n '__fish_use_subcommand' -a 'build' -d 'Build an .ipa'
complete -c darwinforge -n '__fish_use_subcommand' -a 'sdk' -d 'Manage SDKs'
complete -c darwinforge -n '__fish_use_subcommand' -a 'clean' -d 'Remove generated build output'
complete -c darwinforge -n '__fish_use_subcommand' -a 'completions' -d 'Print completions'

complete -c darwinforge -n '__fish_seen_subcommand_from bootstrap' -l yes -a ''
complete -c darwinforge -n '__fish_seen_subcommand_from bootstrap' -l no-install
complete -c darwinforge -n '__fish_seen_subcommand_from bootstrap' -l sdk-version -r
complete -c darwinforge -n '__fish_seen_subcommand_from bootstrap' -l accept-sdk-license
complete -c darwinforge -n '__fish_seen_subcommand_from bootstrap' -l dry-run
complete -c darwinforge -n '__fish_seen_subcommand_from bootstrap' -l source -r

complete -c darwinforge -n '__fish_seen_subcommand_from doctor' -l sdk -r -F
complete -c darwinforge -n '__fish_seen_subcommand_from doctor' -l fix
complete -c darwinforge -n '__fish_seen_subcommand_from doctor' -l yes

complete -c darwinforge -n '__fish_seen_subcommand_from init' -l bundle-id -r
complete -c darwinforge -n '__fish_seen_subcommand_from init' -l dir -r -F
complete -c darwinforge -n '__fish_seen_subcommand_from init' -l force
complete -c darwinforge -n '__fish_seen_subcommand_from init' -l here

complete -c darwinforge -n '__fish_seen_subcommand_from build' -l sdk -r -F
complete -c darwinforge -n '__fish_seen_subcommand_from build' -l arch -r -a 'arm64'
complete -c darwinforge -n '__fish_seen_subcommand_from build' -l project -r -F
complete -c darwinforge -n '__fish_seen_subcommand_from build' -s o -l output -r -F

complete -c darwinforge -n '__fish_seen_subcommand_from sdk' -a 'list install use remove path'
complete -c darwinforge -n '__fish_seen_subcommand_from sdk' -l yes
complete -c darwinforge -n '__fish_seen_subcommand_from sdk' -l source -r

complete -c darwinforge -n '__fish_seen_subcommand_from clean' -l dry-run

complete -c darwinforge -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish powershell'
"#;

const POWERSHELL: &str = r#"using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName darwinforge -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = @('bootstrap','doctor','init','build','sdk','clean','completions')
    $sdkCommands = @('list','install','use','remove','path')
    $shells = @('bash','zsh','fish','powershell')
    $words = $commandAst.Extent.Text.Split(' ', [System.StringSplitOptions]::RemoveEmptyEntries)
    if ($words.Count -lt 2) {
        $commands | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
            [CompletionResult]::new($_, $_, 'ParameterValue', $_)
        }
        return
    }
    switch ($words[1]) {
        'bootstrap' { @('--yes','--no-install','--sdk-version','--accept-sdk-license','--dry-run','--source') }
        'doctor' { @('--sdk','--fix','--yes') }
        'init' { @('--bundle-id','--dir','--force','--here') }
        'build' { @('--sdk','--arch','--project','-o','--output') }
        'sdk' {
            if ($words.Count -lt 3) { $sdkCommands }
            else { @('--yes','--source') }
        }
        'clean' { @('--dry-run') }
        'completions' { $shells }
        default { $commands }
    } | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
        [CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_names_every_top_level_command() {
        for shell in ["bash", "zsh", "fish", "powershell"] {
            let script = for_shell(shell).expect("script");
            for command in ["bootstrap", "doctor", "init", "build", "sdk", "clean", "completions"] {
                assert!(script.contains(command), "{shell} missing {command}");
            }
        }
    }

    #[test]
    fn an_unknown_shell_is_a_usage_error() {
        let error = for_shell("elvish").expect_err("must fail");
        assert_eq!(error.exit_code(), 2);
    }
}

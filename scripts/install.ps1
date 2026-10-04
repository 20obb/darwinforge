<#
.SYNOPSIS
    Installs the darwinforge CLI on Windows, per-user, without elevation.

.DESCRIPTION
    Copies darwinforge.exe into a per-user Programs directory, adds that
    directory to the *User* PATH via [Environment]::SetEnvironmentVariable
    (so no administrator rights and no UAC prompt are needed), and also updates
    the PATH of the current process so `darwinforge` works immediately in the
    shell that ran this script.

    Re-running is safe: the binary is overwritten in place, the PATH entry is
    added only if it is not already present, and nothing outside the install
    directory is removed.

.PARAMETER InstallDir
    Where to install. Defaults to $env:LOCALAPPDATA\Programs\darwinforge.

.PARAMETER SourcePath
    Path to darwinforge.exe. Defaults to the copy shipped next to this script.

.PARAMETER Bootstrap
    Offer to run `darwinforge bootstrap` after installing.

.PARAMETER Force
    Recreate the install directory instead of merging into an existing one.

.EXAMPLE
    .\install.ps1

.EXAMPLE
    .\install.ps1 -InstallDir D:\tools\darwinforge -Bootstrap
#>
[CmdletBinding()]
param(
    [string] $InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\darwinforge'),
    [string] $SourcePath,
    [switch] $Bootstrap,
    [switch] $Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ProgramName = 'darwinforge.exe'

function Write-Step {
    param([string] $Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-Warn {
    param([string] $Message)
    Write-Warning $Message
}

# Read the version out of the crate manifest, so a release artefact and the
# package metadata can never drift apart. Returns $null when not found.
function Get-CrateVersion {
    param([string] $RepoRoot)
    $cargoToml = Join-Path $RepoRoot 'Cargo.toml'
    if (-not (Test-Path -LiteralPath $cargoToml)) { return $null }
    $match = Select-String -LiteralPath $cargoToml -Pattern '^\s*version\s*=\s*"([^"]+)"' |
        Select-Object -First 1
    if ($null -eq $match) { return $null }
    return $match.Matches[0].Groups[1].Value
}

# Add a directory to the *user* PATH, without duplicating an existing entry.
# Returns $true when the persisted value actually changed.
function Add-ToUserPath {
    param([string] $Directory)

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($null -eq $current) { $current = '' }

    $entries = @($current -split ';' | Where-Object { $_ -ne '' })
    foreach ($entry in $entries) {
        # Compare with a trailing separator removed and case-insensitively:
        # Windows paths are case-insensitive and PATH often ends entries in '\'.
        $trimmed = $entry.TrimEnd('\', '/')
        if ($trimmed.Equals($Directory.TrimEnd('\', '/'), [System.StringComparison]::OrdinalIgnoreCase)) {
            return $false
        }
    }

    $updated = if ($current.TrimEnd(';') -eq '') { $Directory } else { "$current;$Directory" }
    [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
    return $true
}

# Make the directory usable in *this* process without restarting the shell.
function Add-ToProcessPath {
    param([string] $Directory)

    $parts = @($env:Path -split ';' | Where-Object { $_ -ne '' })
    foreach ($part in $parts) {
        $trimmed = $part.TrimEnd('\', '/')
        if ($trimmed.Equals($Directory.TrimEnd('\', '/'), [System.StringComparison]::OrdinalIgnoreCase)) {
            return $false
        }
    }
    $env:Path = if ($parts.Count -eq 0) { $Directory } else { "$Directory;$($parts -join ';')" }
    return $true
}

function Resolve-SourceBinary {
    param([string] $Requested, [string] $ScriptRoot)

    if ($Requested) {
        if (-not (Test-Path -LiteralPath $Requested -PathType Leaf)) {
            throw "-SourcePath: no such file: $Requested"
        }
        return (Resolve-Path -LiteralPath $Requested).Path
    }

    $candidates = @(
        (Join-Path $ScriptRoot $ProgramName)
        (Join-Path $ScriptRoot "bin\$ProgramName")
        (Join-Path $ScriptRoot '..\target\release\darwinforge.exe')
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    $onPath = Get-Command 'darwinforge.exe' -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }

    throw "No $ProgramName found next to this script. Unpack the full release ZIP, or pass -SourcePath."
}

# Does this build implement `bootstrap`? Probing the help text keeps a trimmed
# or older binary from producing a confusing error.
function Test-HasBootstrap {
    param([string] $ExePath)
    try {
        $help = & $ExePath '--help' 2>&1 | Out-String
        return $help -match 'bootstrap'
    } catch {
        return $false
    }
}

# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptRoot
$version = Get-CrateVersion -RepoRoot $repoRoot

Write-Step 'darwinforge installer (Windows)'
if ($version) { Write-Host "  version    : $version" } else { Write-Host '  version    : unknown (Cargo.toml not found next to the script)' }
Write-Host "  install dir: $InstallDir"
Write-Host '  scope      : current user (no administrator rights required)'
Write-Host ''

$source = Resolve-SourceBinary -Requested $SourcePath -ScriptRoot $scriptRoot
Write-Host "  source     : $source"
Write-Host ''

$existing = Join-Path $InstallDir $ProgramName
if ((Test-Path -LiteralPath $InstallDir) -and (Test-Path -LiteralPath $existing) -and -not $Force) {
    Write-Host "An existing install is present at $existing; it will be replaced."
}

# -Force means "start clean", but even then we only remove the install
# directory this script owns, never anything above it.
if ($Force -and (Test-Path -LiteralPath $InstallDir)) {
    Write-Step "Removing previous install at $InstallDir"
    Remove-Item -LiteralPath $InstallDir -Recurse -Force
}

if (-not (Test-Path -LiteralPath $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$target = Join-Path $InstallDir $ProgramName
Write-Step "Installing to $target"
Copy-Item -LiteralPath $source -Destination $target -Force

Write-Step 'Updating the user PATH'
if (Add-ToUserPath -Directory $InstallDir) {
    Write-Host '  added to your user PATH.'
} else {
    Write-Host '  already present in your user PATH.'
}
Add-ToProcessPath -Directory $InstallDir | Out-Null
Write-Host '  this shell can use it now.'

Write-Step 'Verifying'
$versionLine = & $target '--version' 2>&1 | Select-Object -First 1
Write-Host "  $versionLine"

if (Test-HasBootstrap -ExePath $target) {
    Write-Host ''
    Write-Step 'Next step'
    Write-Host "  'darwinforge bootstrap' installs the toolchain this project needs"
    Write-Host '  (clang, a Mach-O linker, ldid) using your package manager.'
    if ($Bootstrap) {
        if ($PSCmdlet.ShouldProcess($target, 'run darwinforge bootstrap')) {
            & $target 'bootstrap'
            if ($LASTEXITCODE -ne 0) {
                Write-Warn "bootstrap exited with $LASTEXITCODE; run 'darwinforge doctor' to see what is missing."
            }
        }
    } else {
        Write-Host "  Re-run with -Bootstrap, or just call 'darwinforge bootstrap' when ready."
    }
} else {
    Write-Host ''
    Write-Host "  This build does not provide 'darwinforge bootstrap'."
    Write-Host "  Run 'darwinforge doctor' to see what the iOS toolchain still needs."
}

Write-Host ''
Write-Host 'Re-running this script is safe: it only replaces the binary and the PATH entry.'
Write-Host 'New shells will see the PATH change automatically; this shell is already updated.'


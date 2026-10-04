<#
.SYNOPSIS
    Builds the Windows release artefacts into ./dist.

.DESCRIPTION
    Produces:
        dist\darwinforge-x86_64-pc-windows-msvc.zip
        dist\SHA256SUMS

    The ZIP contains darwinforge.exe, README.md, LIMITATIONS.md, LICENSE and
    install.ps1, so a user only has to unzip and run one script.

    The version is read from Cargo.toml at build time and never written into
    this script.

    The test suite runs FIRST. If it fails, nothing is packaged: a release
    should never ship a build that does not pass its own tests.

.PARAMETER OutDir
    Where to write the artefacts. Defaults to <repo>\dist.

.PARAMETER SkipTests
    Skip `cargo test`. Not recommended; CI runs them too.

.PARAMETER DryRun
    Print the plan and exit without building anything.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\release.ps1
#>
[CmdletBinding()]
param(
    [string] $OutDir,
    [switch] $SkipTests,
    [switch] $DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ProgramName = 'darwinforge.exe'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $repoRoot 'dist' }

function Write-Step { param([string] $m) Write-Host "==> $m" -ForegroundColor Cyan }
function Write-Note { param([string] $m) Write-Host "  - $m" }
function Write-Warn { param([string] $m) Write-Warning $m }

function Get-CrateVersion {
    $cargoToml = Join-Path $repoRoot 'Cargo.toml'
    if (-not (Test-Path -LiteralPath $cargoToml)) { throw "no Cargo.toml at $cargoToml" }
    $m = Select-String -LiteralPath $cargoToml -Pattern '^\s*version\s*=\s*"([^"]+)"' |
        Select-Object -First 1
    if ($null -eq $m) { throw 'no `version = "..."` found in Cargo.toml' }
    return $m.Matches[0].Groups[1].Value
}

# Run a native command and fail the script if it returns a non-zero exit code.
function Invoke-Native {
    param([string] $Exe, [string[]] $Arguments, [string] $What)
    Write-Note $What
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$What failed with exit code $LASTEXITCODE"
    }
}

# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

$version = Get-CrateVersion
# The artifact name deliberately uses the full rustup target triple so it is
# unambiguous next to the musl tarballs in the same release.
$targetTriple = 'x86_64-pc-windows-msvc'
$zipName = "darwinforge-$targetTriple.zip"

Write-Step "darwinforge $version"
Write-Note "repo    : $repoRoot"
Write-Note "out     : $OutDir"
Write-Note "artefact: $zipName"

if ($DryRun) {
    Write-Step 'dry run: nothing will be built'
    if (-not $SkipTests) { Write-Note 'would run : cargo test' }
    Write-Note "would build : $zipName"
    Write-Note 'would write : SHA256SUMS'
    return
}

if (-not (Get-Command 'cargo' -ErrorAction SilentlyContinue)) {
    throw 'cargo not found on PATH'
}

# 1. Tests first.
if ($SkipTests) {
    Write-Warn 'skipping cargo test because -SkipTests was given'
} else {
    Push-Location $repoRoot
    try {
        Invoke-Native -Exe 'cargo' -Arguments @('test') -What 'cargo test'
    } finally {
        Pop-Location
    }
}

# 2. Build.
Push-Location $repoRoot
try {
    Invoke-Native -Exe 'cargo' -Arguments @('build', '--release') -What 'cargo build --release'
} finally {
    Pop-Location
}

$binary = Join-Path $repoRoot "target\release\$ProgramName"
if (-not (Test-Path -LiteralPath $binary)) {
    throw "cargo did not produce $binary"
}

# 3. Stage the ZIP contents in a scratch directory, then zip it.
$stage = Join-Path $repoRoot 'target\release-stage'
if (Test-Path -LiteralPath $stage) {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
New-Item -ItemType Directory -Path $stage -Force | Out-Null

Write-Step 'staging the archive contents'
Copy-Item -LiteralPath $binary -Destination (Join-Path $stage $ProgramName)
foreach ($doc in @('README.md', 'LIMITATIONS.md')) {
    $p = Join-Path $repoRoot $doc
    if (Test-Path -LiteralPath $p) { Copy-Item -LiteralPath $p -Destination $stage }
}
Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination $stage
Copy-Item -LiteralPath (Join-Path $repoRoot 'scripts\install.ps1') -Destination $stage

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$zipPath = Join-Path $OutDir $zipName
if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }

Write-Step "creating $zipName"
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zipPath -CompressionLevel Optimal

Remove-Item -LiteralPath $stage -Recurse -Force

# 4. SHA256SUMS over the artefact, in the same format the Linux release.sh
#    writes, so one `sha256sum -c` covers the whole release.
Write-Step 'writing SHA256SUMS'
$sumsPath = Join-Path $OutDir 'SHA256SUMS'
$hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
# sha256sum's format is "<hash><two spaces><name>".
#
# The line ending must be LF. Set-Content emits CRLF on Windows, and
# `sha256sum -c` on Linux then looks for a file whose name ends in a literal
# \r and fails. [IO.File]::WriteAllText writes exactly the bytes given, with no
# CRLF translation and no unexpected trailing content.
$sumsLine = $hash + '  ' + $zipName + "`n"
[IO.File]::WriteAllText($sumsPath, $sumsLine, (New-Object Text.UTF8Encoding $false))

Write-Step 'artefacts'
foreach ($f in Get-ChildItem -LiteralPath $OutDir -File | Sort-Object Name) {
    $size = '{0,8:N0}' -f $f.Length
    Write-Note ("{0}  ({1} bytes)" -f $f.Name, $size)
}

Write-Host ''
Write-Host "Done. To verify:  cd $OutDir; certutil -hashfile $zipName SHA256"
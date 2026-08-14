#Requires -Version 5.1
<##
.SYNOPSIS
    Provision and assert the Windows release toolchain.

.DESCRIPTION
    The hosted build has no credentials. Missing machine tools may be installed
    from the explicit winget package IDs below; the versions used by the build
    are then asserted before any source or artifact work starts. Use -AssertOnly
    for a no-network local check.
#>

[CmdletBinding()]
param(
    [switch]$AssertOnly,
    [string]$RepoRoot = ''
)

Set-StrictMode -Version Latest
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = Split-Path -Parent $PSScriptRoot
}
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot 'release-pipeline-common.ps1')

function Invoke-Native {
    param([Parameter(Mandatory)][string]$FilePath, [Parameter(Mandatory)][string[]]$Arguments)

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath exited with code $LASTEXITCODE"
    }
}

function Get-CommandPath {
    param([Parameter(Mandatory)][string]$Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    return $null
}

function Install-WingetPackage {
    param(
        [Parameter(Mandatory)][string]$Id,
        [switch]$Upgrade
    )

    if ($AssertOnly) {
        throw "Missing prerequisite '$Id' (AssertOnly mode does not install packages)."
    }
    $winget = Get-CommandPath 'winget'
    if (-not $winget) {
        throw "Missing '$Id' and winget is unavailable. Install the pinned package on the CircleCI Windows image."
    }
    Write-Host "Installing winget package $Id"
    $arguments = @(
        'install', '--id', $Id, '--exact', '--source', 'winget',
        '--silent', '--accept-source-agreements', '--accept-package-agreements'
    )
    if ($Upgrade) { $arguments += '--upgrade' }
    Invoke-Native $winget $arguments
}

function Require-Command {
    param([Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string]$PackageId)

    $path = Get-CommandPath $Name
    if (-not $path) {
        Install-WingetPackage $PackageId
        $path = Get-CommandPath $Name
    }
    if (-not $path) {
        throw "Required command '$Name' is still unavailable after provisioning '$PackageId'."
    }
    Write-Host "[ok] $($Name): $path"
    return $path
}

function Get-InnoSetupCompiler {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
    )
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) { return $candidate }
    }
    return (Get-CommandPath 'ISCC.exe')
}

$packageJsonPath = Join-Path $RepoRoot 'apps\desktop-tauri\package.json'
if (-not (Test-Path -LiteralPath $packageJsonPath -PathType Leaf)) {
    throw "Missing package metadata: $packageJsonPath"
}
$packageJson = Get-Content -Raw -LiteralPath $packageJsonPath | ConvertFrom-Json
$expectedPnpm = [string]$packageJson.packageManager -replace '^pnpm@', ''
if ($expectedPnpm -notmatch '^10\.18\.1$') {
    throw "Unexpected packageManager '$($packageJson.packageManager)'; release pipeline pins pnpm 10.18.1."
}

Require-Command 'git' 'Git.Git' | Out-Null
$requiredNodeMajor = 24
$nodePackageId = 'OpenJS.NodeJS.LTS'
$node = Get-CommandPath 'node'
$nodeVersion = ''
$nodeMajor = $null
if ($node) {
    $nodeVersion = (& $node --version).Trim()
    try {
        $nodeMajor = Get-NodeMajor $nodeVersion
    } catch {
        $nodeMajor = $null
    }
}
if ($nodeMajor -ne $requiredNodeMajor) {
    if ($AssertOnly) {
        throw "Node $requiredNodeMajor.x is required; found $nodeVersion."
    }
    if ($node) {
        Install-WingetPackage $nodePackageId -Upgrade
    } else {
        Install-WingetPackage $nodePackageId
    }
    $node = Get-CommandPath 'node'
    if (-not $node) {
        throw "Required command 'node' is still unavailable after provisioning '$nodePackageId'."
    }
    $nodeVersion = (& $node --version).Trim()
}
Assert-NodeMajor $nodeVersion $requiredNodeMajor | Out-Null
Write-Host "[ok] Node $nodeVersion (major $requiredNodeMajor)"

$corepack = Get-CommandPath 'corepack'
if (-not $corepack) {
    throw "Corepack is required with Node $requiredNodeMajor to activate pinned pnpm $expectedPnpm."
}
if (-not $AssertOnly) {
    Invoke-Native $corepack @('enable')
    Invoke-Native $corepack @('prepare', "pnpm@$expectedPnpm", '--activate')
}
$pnpm = Get-CommandPath 'pnpm'
if (-not $pnpm) {
    throw "pnpm $expectedPnpm is unavailable after Corepack provisioning."
}
$pnpmVersion = (& $pnpm --version).Trim()
if ($pnpmVersion -ne $expectedPnpm) {
    throw "pnpm $pnpmVersion is active; expected exact $expectedPnpm."
}
Write-Host "[ok] pnpm $pnpmVersion"

Require-Command 'cargo' 'Rustlang.Rustup' | Out-Null
Require-Command 'rustc' 'Rustlang.Rustup' | Out-Null
$rustup = Require-Command 'rustup' 'Rustlang.Rustup'
$target = 'x86_64-pc-windows-msvc'
$installedTargets = @(& $rustup target list --installed)
if ($installedTargets -notcontains $target) {
    if ($AssertOnly) {
        throw "Rust target $target is not installed."
    }
    Invoke-Native $rustup @('target', 'add', $target)
}
Write-Host "[ok] Rust target $target"

$iscc = Get-InnoSetupCompiler
if (-not $iscc) {
    if ($AssertOnly) {
        throw 'Inno Setup 6 ISCC.exe is unavailable.'
    }
    Install-WingetPackage 'JRSoftware.InnoSetup'
    $iscc = Get-InnoSetupCompiler
}
if (-not $iscc) {
    throw 'Inno Setup 6 ISCC.exe is unavailable after provisioning.'
}
$innoVersion = (Get-Item -LiteralPath $iscc).VersionInfo.FileVersion
if ($innoVersion -notmatch '^6\.') {
    throw "Inno Setup 6.x is required; found '$innoVersion'."
}
Write-Host "[ok] Inno Setup $innoVersion ($iscc)"

Write-Host ''
Write-Host "Release prerequisites passed (Git, Node $requiredNodeMajor, pnpm 10.18.1, Rust MSVC target, Inno Setup 6)."

#Requires -Version 5.1
<##
.SYNOPSIS
    Run the credential-free CircleCI Windows release build and artifact bundle.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Tag = $env:CIRCLE_TAG,
    [Parameter(Mandatory)][string]$Sha = $env:CIRCLE_SHA1,
    [string]$RepoRoot = '',
    [string]$OutputDir = '',
    [string]$Repository = 'https://github.com/nesszer/Win-CodexBar.git'
)

Set-StrictMode -Version Latest
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = Split-Path -Parent $PSScriptRoot
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $RepoRoot 'release-output'
}
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-pipeline-common.ps1')

function Invoke-LoggedPowerShell {
    param(
        [Parameter(Mandatory)][string]$ScriptPath,
        [Parameter(Mandatory)][string[]]$Arguments,
        [Parameter(Mandatory)][string]$LogPath
    )

    & powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $ScriptPath @Arguments *> $LogPath
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        Write-Host "--- $LogPath ---"
        if (Test-Path -LiteralPath $LogPath) { Get-Content -LiteralPath $LogPath }
        throw "$ScriptPath failed with exit code $exitCode"
    }
}

function Clear-DirectoryContents {
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return }
    foreach ($entry in Get-ChildItem -LiteralPath $Path -Force) {
        if ($entry.PSIsContainer) { [IO.Directory]::Delete($entry.FullName, $true) }
        else { [IO.File]::Delete($entry.FullName) }
    }
}

if (-not (Test-CanonicalReleaseTag $Tag)) { throw "Invalid canonical release tag '$Tag'." }
if ($Sha -notmatch '^[0-9a-fA-F]{40}$') { throw "Invalid immutable SHA '$Sha'." }
if ((Normalize-GitHubRepository $Repository) -ne 'nesszer/win-codexbar') { throw 'Repository must be canonical nesszer/Win-CodexBar.' }
if (-not (Test-Path -LiteralPath $RepoRoot -PathType Container)) { throw "Missing repository root: $RepoRoot" }

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
Clear-DirectoryContents $OutputDir
$version = Get-ReleaseVersionFromTag $Tag
$tempRoot = if ($env:USERPROFILE) {
    Join-Path $env:USERPROFILE 'cb'
} elseif ($env:TEMP) {
    Join-Path $env:TEMP 'cb'
} else {
    Join-Path ([IO.Path]::GetTempPath()) 'cb'
}
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
$workRoot = Join-Path $tempRoot ('r-' + [guid]::NewGuid().ToString('N'))
$assetsDir = Join-Path $workRoot 'assets'
$doctorLog = Join-Path $OutputDir 'release-doctor.log'
$buildLog = Join-Path $OutputDir 'windows-release-build.log'

try {
    Invoke-LoggedPowerShell (Join-Path $RepoRoot 'scripts\release-doctor.ps1') @('-Version', $version, '-SkipGitHub') $doctorLog
    Invoke-LoggedPowerShell (Join-Path $RepoRoot 'scripts\windows-release-build.ps1') @(
        '-Ref', $Sha,
        '-RepoUrl', $Repository,
        '-WorkRoot', $workRoot,
        '-SmokeInstall'
    ) $buildLog
    Invoke-LoggedPowerShell (Join-Path $RepoRoot 'scripts\emit-release-manifest.ps1') @(
        '-AssetsDir', $assetsDir,
        '-OutputDir', $OutputDir,
        '-Tag', $Tag,
        '-Sha', $Sha
    ) (Join-Path $OutputDir 'emit-release-manifest.log')
    Write-Host "Credential-free release build passed for $Tag ($Sha)."
} finally {
    if (Test-Path -LiteralPath $workRoot -PathType Container) {
        foreach ($entry in @(Get-ChildItem -LiteralPath $workRoot -Force -Recurse -ErrorAction SilentlyContinue)) {
            if ($entry -is [IO.FileInfo]) {
                try { $entry.IsReadOnly = $false } catch { }
            }
        }
        try {
            [IO.Directory]::Delete($workRoot, $true)
        } catch {
            Write-Warning "Could not remove ephemeral WorkRoot '$workRoot': $($_.Exception.Message)"
        }
    }
}

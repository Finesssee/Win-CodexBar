#Requires -Version 5.1
<##
.SYNOPSIS
    Validate the exact, canonical release source selected by CircleCI.

.DESCRIPTION
    This is deliberately credential-free. It accepts only canonical vX.Y.Z tags,
    verifies that the checked-out commit and tag resolve to the supplied SHA, and
    proves the tag commit is reachable from the protected main branch. It also checks
    every committed project version file before a release build is allowed to run.
#>

[CmdletBinding()]
param(
    [string]$Tag = $env:CIRCLE_TAG,
    [string]$Sha = $env:CIRCLE_SHA1,
    [string]$RepoRoot = '',
    [string]$Repository = "nesszer/Win-CodexBar",
    [string]$MainBranch = "main"
)

Set-StrictMode -Version Latest
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = Split-Path -Parent $PSScriptRoot
}
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "release-pipeline-common.ps1")

function Invoke-GitCapture {
    param([Parameter(Mandatory)][string[]]$Arguments)

    $git = Get-Command git -ErrorAction Stop
    $output = & $git.Source @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE`n$($output | Out-String)"
    }
    return ($output | Out-String).Trim()
}

function Get-CargoPackageVersion {
    param([Parameter(Mandatory)][string]$Path)

    $line = Get-Content -LiteralPath $Path | Where-Object { $_ -match '^version\s*=\s*"([^"]+)"' } | Select-Object -First 1
    if (-not $line -or $line -notmatch '^version\s*=\s*"([^"]+)"') {
        throw "Could not read package version from $Path"
    }
    return $Matches[1]
}

function Get-VersionEnvValue {
    param([Parameter(Mandatory)][string]$Path)

    $line = Get-Content -LiteralPath $Path | Where-Object { $_ -match '^MARKETING_VERSION=(.+)$' } | Select-Object -First 1
    if (-not $line -or $line -notmatch '^MARKETING_VERSION=(.+)$') {
        throw "Could not read MARKETING_VERSION from $Path"
    }
    return $Matches[1].Trim()
}

function Assert-VersionEqual {
    param([Parameter(Mandatory)][string]$Label, [Parameter(Mandatory)][string]$Actual, [Parameter(Mandatory)][string]$Expected)

    if ($Actual -ne $Expected) {
        throw "$Label is '$Actual', expected '$Expected'."
    }
    Write-Host "[ok] $Label = $Expected"
}

if (-not (Test-Path -LiteralPath $RepoRoot -PathType Container)) {
    throw "Repository root does not exist: $RepoRoot"
}
if (-not (Test-CanonicalReleaseTag $Tag)) {
    throw "CircleCI release requires a canonical vX.Y.Z tag; received '$Tag'."
}
if ([string]::IsNullOrWhiteSpace($Sha) -or $Sha -notmatch '^[0-9a-fA-F]{40}$') {
    throw "CircleCI release requires a full 40-character commit SHA; received '$Sha'."
}
if ([string]::IsNullOrWhiteSpace($MainBranch) -or $MainBranch -ne 'main') {
    throw "Release ancestry must be checked against the protected main branch."
}
if ($env:CIRCLE_PULL_REQUEST) {
    throw "Pull-request builds are not release builds."
}
if ($env:CIRCLE_BRANCH) {
    throw "Branch builds are not release builds; CIRCLE_BRANCH was '$env:CIRCLE_BRANCH'."
}

Push-Location $RepoRoot
try {
    $origin = Invoke-GitCapture @('config', '--get', 'remote.origin.url')
    if ((Normalize-GitHubRepository $origin) -ne (Normalize-GitHubRepository $Repository)) {
        throw "Remote origin '$origin' is not canonical repository '$Repository'."
    }
    Write-Host "[ok] canonical repository: $(Normalize-GitHubRepository $origin)"

    if ($env:CIRCLE_PROJECT_USERNAME -and $env:CIRCLE_PROJECT_REPONAME) {
        $circleProject = "$($env:CIRCLE_PROJECT_USERNAME)/$($env:CIRCLE_PROJECT_REPONAME)"
        if ((Normalize-GitHubRepository $circleProject) -ne (Normalize-GitHubRepository $Repository)) {
            throw "CircleCI project '$circleProject' is not canonical repository '$Repository'."
        }
    }

    $head = Invoke-GitCapture @('rev-parse', 'HEAD')
    if ($head -ne $Sha.ToLowerInvariant()) {
        throw "Checked-out HEAD '$head' does not equal immutable release SHA '$Sha'."
    }
    Write-Host "[ok] HEAD is immutable SHA $Sha"

    $tagSha = Invoke-GitCapture @('rev-parse', '--verify', "$Tag^{commit}")
    if ($tagSha -ne $Sha.ToLowerInvariant()) {
        throw "Tag '$Tag' resolves to '$tagSha', not immutable release SHA '$Sha'."
    }
    Write-Host "[ok] $Tag resolves to $Sha"

    $remoteMainRef = "refs/remotes/origin/$MainBranch"
    $git = Get-Command git -ErrorAction Stop
    $isShallow = Invoke-GitCapture @('rev-parse', '--is-shallow-repository')
    if ($isShallow -eq 'true') {
        & $git.Source fetch --quiet --no-tags --unshallow origin 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw 'Could not obtain full history for protected main ancestry validation.'
        }
    }
    & $git.Source fetch --quiet --no-tags --force origin "$MainBranch`:$remoteMainRef" 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Could not fetch protected origin/$MainBranch for ancestry validation."
    }
    $mainSha = Invoke-GitCapture @('rev-parse', "${remoteMainRef}^{commit}")
    & $git.Source merge-base --is-ancestor $Sha $remoteMainRef 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Release commit $Sha is not reachable from protected origin/$MainBranch ($mainSha)."
    }
    Write-Host "[ok] $Sha is reachable from protected origin/$MainBranch ($mainSha)"

    $version = Get-ReleaseVersionFromTag $Tag
    $versionFiles = [ordered]@{
        'rust/Cargo.toml' = Get-CargoPackageVersion (Join-Path $RepoRoot 'rust\Cargo.toml')
        'apps/desktop-tauri/src-tauri/Cargo.toml' = Get-CargoPackageVersion (Join-Path $RepoRoot 'apps\desktop-tauri\src-tauri\Cargo.toml')
        'apps/desktop-tauri/package.json' = ((Get-Content -Raw -LiteralPath (Join-Path $RepoRoot 'apps\desktop-tauri\package.json') | ConvertFrom-Json).version)
        'apps/desktop-tauri/src-tauri/tauri.conf.json' = ((Get-Content -Raw -LiteralPath (Join-Path $RepoRoot 'apps\desktop-tauri\src-tauri\tauri.conf.json') | ConvertFrom-Json).version)
        'version.env' = Get-VersionEnvValue (Join-Path $RepoRoot 'version.env')
    }
    foreach ($entry in $versionFiles.GetEnumerator()) {
        Assert-VersionEqual $entry.Key ([string]$entry.Value) $version
    }

    Write-Host ""
    Write-Host "Release preflight passed: $Repository $Tag ($Sha)"
    Write-Host "RELEASE_VERSION=$version"
} finally {
    Pop-Location
}

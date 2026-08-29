#Requires -Version 5.1
<#
.SYNOPSIS
    Check whether a Win-CodexBar release is ready or complete.

.DESCRIPTION
    Verifies version-file consistency, changelog presence, optional local
    Windows assets, asset SHA-256 sidecars, Git tag presence, and GitHub release
    asset presence when gh is authenticated.
#>

param(
    [string]$Version = "",
    [string]$AssetsDir = "C:\code\Win-CodexBar-release\assets",
    [switch]$SkipGitHub
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$Failures = New-Object System.Collections.Generic.List[string]
$Warnings = New-Object System.Collections.Generic.List[string]

function Write-Ok {
    param([string]$Message)
    Write-Host "[ok] $Message"
}

function Write-Warn {
    param([string]$Message)
    $Warnings.Add($Message)
    Write-Host "[warn] $Message" -ForegroundColor Yellow
}

function Write-Fail {
    param([string]$Message)
    $Failures.Add($Message)
    Write-Host "[fail] $Message" -ForegroundColor Red
}

function Get-CargoVersion {
    param([string]$Path)
    $line = Get-Content $Path | Where-Object { $_ -match '^version = "([^"]+)"' } | Select-Object -First 1
    if ($line -and $line -match '^version = "([^"]+)"') {
        return $Matches[1]
    }
    return ""
}

function Get-VersionEnvValue {
    param([string]$Path)
    if (-not (Test-Path $Path)) {
        return ""
    }
    $line = Get-Content $Path | Where-Object { $_ -match '^MARKETING_VERSION=(.+)$' } | Select-Object -First 1
    if ($line -and $line -match '^MARKETING_VERSION=(.+)$') {
        return $Matches[1].Trim()
    }
    return ""
}

function Assert-Version {
    param(
        [string]$Label,
        [string]$Actual,
        [string]$Expected
    )
    if ($Actual -eq $Expected) {
        Write-Ok "$Label version is $Actual"
    } else {
        Write-Fail "$Label version is $Actual, expected $Expected"
    }
}

function Test-AssetHash {
    param([string]$AssetPath)

    $shaPath = "$AssetPath.sha256"
    if (-not (Test-Path $AssetPath)) {
        Write-Fail "missing asset: $AssetPath"
        return
    }
    if (-not (Test-Path $shaPath)) {
        Write-Fail "missing sha256 sidecar: $shaPath"
        return
    }

    $expected = ((Get-Content $shaPath | Select-Object -First 1) -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $AssetPath).Hash.ToLowerInvariant()
    if ($actual -eq $expected) {
        Write-Ok "$(Split-Path $AssetPath -Leaf) hash matches sidecar"
    } else {
        Write-Fail "$(Split-Path $AssetPath -Leaf) hash mismatch: expected $expected, got $actual"
    }
}

$rustVersion = Get-CargoVersion (Join-Path $RepoRoot "rust\Cargo.toml")
if (-not $Version) {
    $Version = $rustVersion
}
if (-not $Version) {
    throw "Could not determine release version."
}

$tag = "v$Version"
Write-Host "Release doctor: Win-CodexBar $Version"
Write-Host ""

Assert-Version "rust/Cargo.toml" $rustVersion $Version
Assert-Version "apps/desktop-tauri/src-tauri/Cargo.toml" (Get-CargoVersion (Join-Path $RepoRoot "apps\desktop-tauri\src-tauri\Cargo.toml")) $Version
Assert-Version "version.env" (Get-VersionEnvValue (Join-Path $RepoRoot "version.env")) $Version

$packageJsonPath = Join-Path $RepoRoot "apps\desktop-tauri\package.json"
$packageVersion = ((Get-Content -Raw $packageJsonPath) | ConvertFrom-Json).version
Assert-Version "apps/desktop-tauri/package.json" $packageVersion $Version

$tauriConfigPath = Join-Path $RepoRoot "apps\desktop-tauri\src-tauri\tauri.conf.json"
$tauriVersion = ((Get-Content -Raw $tauriConfigPath) | ConvertFrom-Json).version
Assert-Version "tauri.conf.json" $tauriVersion $Version

$git = Get-Command git -ErrorAction SilentlyContinue
if ($git) {
    Push-Location $RepoRoot
    try {
        & $git.Source rev-parse --verify --quiet "$tag^{commit}" *> $null
        if ($LASTEXITCODE -eq 0) {
            Write-Ok "Git tag exists: $tag"
        } else {
            Write-Warn "Git tag not found locally: $tag"
        }
    } finally {
        Pop-Location
    }
} else {
    Write-Warn "git not found; skipped local tag check"
}

$changelogPath = Join-Path $RepoRoot "CHANGELOG.md"
if ((Test-Path $changelogPath) -and (Select-String -Path $changelogPath -Pattern ([regex]::Escape($Version)) -Quiet)) {
    Write-Ok "CHANGELOG.md mentions $Version"
} else {
    Write-Warn "CHANGELOG.md does not mention $Version"
}

if (Test-Path $AssetsDir) {
    Test-AssetHash (Join-Path $AssetsDir "CodexBar-$Version-Setup.exe")
    Test-AssetHash (Join-Path $AssetsDir "CodexBar-$Version-portable.exe")
    Test-AssetHash (Join-Path $AssetsDir "CodexBarCLI-v$Version-windows-x64.zip")
} else {
    Write-Warn "local assets directory not found: $AssetsDir"
}

if (-not $SkipGitHub) {
    $gh = Get-Command gh -ErrorAction SilentlyContinue
    if ($gh) {
        Push-Location $RepoRoot
        try {
            $ghJsonPath = Join-Path $env:TEMP "win-codexbar-release-doctor-gh.json"
            $ghErrPath = Join-Path $env:TEMP "win-codexbar-release-doctor-gh.err"
            $repoFlag = @()
            $originUrl = (& $git.Source config --get remote.origin.url 2>$null)
            if ($originUrl) { $repoFlag = @("-R", ($originUrl -replace '^https://github\.com/','' -replace '\.git$','' -replace '^git@github\.com:','')) }
            & $gh.Source release view $tag @repoFlag --json assets,url 1>$ghJsonPath 2>$ghErrPath
            if ($LASTEXITCODE -eq 0) {
                $release = Get-Content -Raw $ghJsonPath | ConvertFrom-Json
                Write-Ok "GitHub release exists: $($release.url)"
                $assetNames = @($release.assets | ForEach-Object { $_.name })
                foreach ($name in @(
                    "CodexBar-$Version-Setup.exe",
                    "CodexBar-$Version-Setup.exe.sha256",
                    "CodexBar-$Version-portable.exe",
                    "CodexBar-$Version-portable.exe.sha256",
                    "CodexBarCLI-v$Version-windows-x64.zip",
                    "CodexBarCLI-v$Version-windows-x64.zip.sha256"
                )) {
                    if ($assetNames -contains $name) {
                        Write-Ok "GitHub release has $name"
                    } else {
                        Write-Fail "GitHub release missing $name"
                    }
                }
            } else {
                $err = Get-Content -Raw $ghErrPath
                Write-Warn "GitHub release $tag not found or gh is not authenticated: $err"
            }
        } finally {
            Pop-Location
        }
    } else {
        Write-Warn "gh not found; skipped GitHub release checks"
    }
}

Write-Host ""
Write-Host "Winget reminder: after GitHub assets are stable, copy the previous manifest folder and update PackageVersion, InstallerUrl, InstallerSha256, DisplayVersion, ReleaseNotes, and ReleaseNotesUrl."

if ($Failures.Count -gt 0) {
    Write-Host ""
    Write-Host "$($Failures.Count) release doctor check(s) failed." -ForegroundColor Red
    exit 1
}

if ($Warnings.Count -gt 0) {
    Write-Host ""
    Write-Host "$($Warnings.Count) warning(s)." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "Release doctor passed."

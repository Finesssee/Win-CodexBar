#Requires -Version 5.1
param(
    [switch]$Rust,
    [switch]$Tauri,
    [switch]$Frontend,
    [switch]$Format,
    [switch]$Clippy,
    [switch]$ReleaseDoctor,
    [switch]$All,
    [string]$Version = "",
    [ValidateSet("ci")]
    [string]$Slice = ""
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = "Continue"

$RepoRoot = Split-Path -Parent $PSScriptRoot

function Invoke-Step {
    param(
        [string]$Name,
        [string]$FilePath,
        [string[]]$ArgumentList
    )

    Write-Host ""
    Write-Host "==> $Name" -ForegroundColor Cyan
    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

# Hosted pr-check slice (-Slice ci): mirrors .github/workflows/pr-check.yml
# step for step (workspace-wide fmt/clippy/test, frozen frontend install,
# frontend test/build, interaction-guard script tests). The guard's script
# tests are pure Node, so the mirror stays honest locally too.
if ($Slice -eq 'ci') {
    Push-Location $RepoRoot
    try {
        Invoke-Step "Rust format check" "cargo" @("fmt", "--all", "--check")
        Invoke-Step "Rust clippy (workspace)" "cargo" @("clippy", "--workspace", "--all-targets", "--", "-D", "warnings")
        Invoke-Step "Rust tests (workspace)" "cargo" @("test", "--workspace")
        Invoke-Step "Frontend install" "pnpm" @("--dir", "apps\desktop-tauri", "install", "--frozen-lockfile")
        Invoke-Step "Frontend tests" "pnpm" @("--dir", "apps\desktop-tauri", "test")
        Invoke-Step "Frontend type check / build" "pnpm" @("--dir", "apps\desktop-tauri", "run", "build")
        Invoke-Step "Interaction guard script tests" "node" @("--test", ".github/scripts/interaction-guard.test.mjs")
    } finally {
        Pop-Location
    }
    Write-Host ""
    Write-Host "Local checks passed." -ForegroundColor Green
    return
}

if (-not ($Rust -or $Tauri -or $Frontend -or $Format -or $Clippy -or $ReleaseDoctor -or $All)) {
    $Rust = $true
    $Tauri = $true
    $Frontend = $true
}

Push-Location $RepoRoot
try {
    Invoke-Step "GitHub write-safety tests" "bash" @("scripts/gh-safe.tests.sh")
    if ($All -or $Format) {
        Invoke-Step "Rust format" "cargo" @("fmt", "--all", "--check")
    }
    if ($All -or $Clippy) {
        Invoke-Step "Shared Rust clippy" "cargo" @("clippy", "--manifest-path", "rust\Cargo.toml", "--all-targets", "--", "-D", "warnings")
        Invoke-Step "Tauri Rust clippy" "cargo" @("clippy", "--manifest-path", "apps\desktop-tauri\src-tauri\Cargo.toml", "--all-targets", "--", "-D", "warnings")
    }
    if ($All -or $Rust) {
        Invoke-Step "Shared Rust tests" "cargo" @("test", "--manifest-path", "rust\Cargo.toml")
    }
    if ($All -or $Tauri) {
        Invoke-Step "Tauri Rust tests" "cargo" @("test", "--manifest-path", "apps\desktop-tauri\src-tauri\Cargo.toml")
    }
    if ($All -or $Frontend) {
        Invoke-Step "Frontend tests" "pnpm" @("--dir", "apps\desktop-tauri", "test")
        Invoke-Step "Frontend build" "pnpm" @("--dir", "apps\desktop-tauri", "run", "build")
    }
    if ($All -or $ReleaseDoctor) {
        $args = @("-File", "scripts\release-doctor.ps1")
        if ($Version) {
            $args += @("-Version", $Version)
        }
        Invoke-Step "Release doctor" "powershell.exe" $args
    }
} finally {
    Pop-Location
}

Write-Host ""
Write-Host "Local checks passed." -ForegroundColor Green

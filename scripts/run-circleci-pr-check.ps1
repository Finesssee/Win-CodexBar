#Requires -Version 5.1
<##
.SYNOPSIS
    Provision the pinned toolchain and run the canonical check slice on the
    CircleCI Windows executor.

.DESCRIPTION
    Owns the provisioning that used to live inline in .circleci/config.yml:

    - Rust: pinned by -RustVersion (the config passes the same version the
      config's cache key and restore fallback are keyed on). If rustup is
      absent it is installed from static.rust-lang.org with the adjacent
      official .sha256 file verified (Get-FileHash) before execution. Never
      `curl | iex`.
    - Node: exact major pinned by -NodeVersion (and -NodeMajor), installed
      from the official nodejs.org MSI whose SHA-256 is verified against the
      official SHASUMS256.txt entry for that exact MSI before msiexec runs.
      The MSI installs into a dedicated per-version directory because the
      image's preinstalled Node is a different major and a same-product MSI
      upgrade silently no-ops.
    - pnpm: the exact packageManager pin from apps/desktop-tauri/package.json
      is activated by corepack and asserted.
    - Checks: delegates everything else to scripts\local-check.ps1 -Slice ci,
      the single source of truth for fmt/clippy/test/frontend/guard steps.

    Pure checksum logic lives in scripts\circleci-pr-common.ps1 and is
    exercised by scripts\circleci-pr.tests.ps1 without network access.
#>
[CmdletBinding()]
param(
    # Pinned Rust version installed and asserted (single source of truth:
    # .circleci/config.yml passes the same value its cache keys use).
    [Parameter(Mandatory)][string]$RustVersion,

    # Exact Node version to install if the image does not already provide it.
    [Parameter(Mandatory)][string]$NodeVersion,

    # Major of -NodeVersion; asserted against whatever Node is active.
    [Parameter(Mandatory)][ValidateRange(1, 99)][int]$NodeMajor,

    # Repository root; defaults to the checkout this script lives in.
    [AllowEmptyString()][string]$RepoRoot = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = Split-Path -Parent $PSScriptRoot
}

. (Join-Path $PSScriptRoot 'circleci-pr-common.ps1')

Push-Location $RepoRoot
try {
    # --- Rust ---------------------------------------------------------------
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
        Write-Host 'rustup is unavailable on the image; installing via verified rustup-init.exe (winget is not on the hosted image).'
        $rustupInit = Join-Path $env:TEMP 'rustup-init.exe'
        $rustupUrl = 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe'
        & curl.exe -sSfL -o $rustupInit $rustupUrl
        if ($LASTEXITCODE -ne 0) { throw "downloading rustup-init.exe exited with code $LASTEXITCODE" }
        # Official checksum published adjacent to the binary.
        $rustupShaFile = Join-Path $env:TEMP 'rustup-init.exe.sha256'
        & curl.exe -sSfL -o $rustupShaFile "$rustupUrl.sha256"
        if ($LASTEXITCODE -ne 0) { throw "downloading rustup-init.exe.sha256 exited with code $LASTEXITCODE" }
        $expectedRustupSha = Get-ExpectedSha256 -ChecksumText (Get-Content -Raw -LiteralPath $rustupShaFile) -FileName 'rustup-init.exe'
        Assert-FileSha256 -Path $rustupInit -ExpectedSha256 $expectedRustupSha
        Write-Host "[ok] rustup-init.exe SHA-256 verified ($expectedRustupSha)"
        & $rustupInit -y --default-toolchain none --profile minimal
        if ($LASTEXITCODE -ne 0) { throw "rustup-init exited with code $LASTEXITCODE" }
        $env:Path = "$(Join-Path $env:USERPROFILE '.cargo\bin');$env:Path"
    }
    & rustup toolchain install $RustVersion --profile default -c rustfmt,clippy -t x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw "rustup toolchain install exited with code $LASTEXITCODE" }
    & rustup default $RustVersion
    if ($LASTEXITCODE -ne 0) { throw "rustup default exited with code $LASTEXITCODE" }
    $activeRust = ((& rustc --version) -join ' ').Trim()
    if ($activeRust -notmatch [regex]::Escape($RustVersion)) { throw "rustc reports '$activeRust'; expected pinned $RustVersion." }
    Write-Host "[ok] rustup default $RustVersion (rustc $activeRust)"

    # --- Node ---------------------------------------------------------------
    # The image's own Node may occupy C:\Program Files\nodejs, so probe with
    # that prefix first and keep it on PATH for every branch below.
    $env:Path = "C:\Program Files\nodejs;$env:Path"
    # NOTE: these locals are case-insensitively distinct from the $NodeVersion
    # parameter; never reuse $nodeVersion here or it clobbers the pinned
    # version before the MSI URL is built (dist/vv<image>/... -> 404).
    $nodeDir = ''
    $imageNodeVersion = $null
    if (Get-Command node -ErrorAction SilentlyContinue) { $imageNodeVersion = (& node --version).Trim() }
    $activeNodeMajor = 0
    if ($imageNodeVersion -match '^v(\d+)\.') { $activeNodeMajor = [int]$Matches[1] }
    if ($activeNodeMajor -ne $NodeMajor) {
        $found = if ($imageNodeVersion) { $imageNodeVersion } else { 'none' }
        Write-Host "Node $NodeMajor.x is required (image has $found); installing Node $NodeVersion via the official MSI (winget is not on the hosted image)."
        $msiName = "node-v$NodeVersion-x64.msi"
        $nodeMsi = Join-Path $env:TEMP $msiName
        & curl.exe -sSfL -o $nodeMsi "https://nodejs.org/dist/v$NodeVersion/$msiName"
        if ($LASTEXITCODE -ne 0) { throw "downloading Node MSI exited with code $LASTEXITCODE" }
        # Official SHASUMS256.txt entry for this exact MSI.
        $shasumsFile = Join-Path $env:TEMP 'SHASUMS256.txt'
        & curl.exe -sSfL -o $shasumsFile "https://nodejs.org/dist/v$NodeVersion/SHASUMS256.txt"
        if ($LASTEXITCODE -ne 0) { throw "downloading SHASUMS256.txt exited with code $LASTEXITCODE" }
        $expectedNodeSha = Get-ExpectedSha256 -ChecksumText (Get-Content -Raw -LiteralPath $shasumsFile) -FileName $msiName
        Assert-FileSha256 -Path $nodeMsi -ExpectedSha256 $expectedNodeSha
        Write-Host "[ok] $msiName SHA-256 verified ($expectedNodeSha)"
        # A same-product MSI upgrade can silently no-op; install into a
        # dedicated per-version directory and put it first on PATH.
        $nodeDir = "C:\node-v$NodeVersion"
        $msiArgs = @('/i', $nodeMsi, '/qn', '/norestart', "INSTALLDIR=$nodeDir")
        $proc = Start-Process msiexec.exe -ArgumentList $msiArgs -Wait -PassThru
        if ($proc.ExitCode -ne 0) { throw "msiexec install exited with code $($proc.ExitCode)" }
        $nodeExe = Join-Path $nodeDir 'node.exe'
        if (-not (Test-Path $nodeExe)) { throw 'node.exe is still unavailable after MSI provisioning.' }
        $installedNodeVersion = (& $nodeExe --version).Trim()
        $activeNodeMajor = [int]($installedNodeVersion -replace '^v(\d+)\..*$', '$1')
        if ($activeNodeMajor -ne $NodeMajor) { throw "Node $installedNodeVersion is installed; required major $NodeMajor." }
    }
    $env:Path = "$(if ($nodeDir) { "$nodeDir;" })$env:Path"
    Write-Host "[ok] Node $installedNodeVersion"

    # --- pnpm ---------------------------------------------------------------
    $pnpmShimDir = Join-Path $env:LOCALAPPDATA 'CodexBar\ci-toolchain\pnpm'
    New-Item -ItemType Directory -Force -Path $pnpmShimDir | Out-Null
    & corepack enable --install-directory $pnpmShimDir
    if ($LASTEXITCODE -ne 0) { throw "corepack enable exited with code $LASTEXITCODE" }
    # packageManager in apps/desktop-tauri/package.json is the source of
    # truth for the pnpm version (no hardcoded drift).
    $packageManager = [string]((Get-Content -Raw -LiteralPath (Join-Path $RepoRoot 'apps\desktop-tauri\package.json') | ConvertFrom-Json).packageManager)
    if ($packageManager -notmatch '^pnpm@(.+)$') { throw "packageManager '$packageManager' does not match ^pnpm@<version>." }
    $expectedPnpm = $Matches[1]
    & corepack prepare $packageManager --activate
    if ($LASTEXITCODE -ne 0) { throw "corepack prepare $packageManager --activate exited with code $LASTEXITCODE" }
    $env:Path = "$pnpmShimDir;$env:Path"
    $pnpmVersion = (& pnpm --version).Trim()
    if ($pnpmVersion -ne $expectedPnpm) { throw "pnpm $pnpmVersion is active; expected exact $expectedPnpm (packageManager $packageManager)." }
    Write-Host "[ok] pnpm $pnpmVersion"

    # --- Checks -------------------------------------------------------------
    # Delegate the check slice to the local-check script (fmt/clippy/test,
    # frontend install/test/build, interaction-guard tests) with the PATH
    # prefix set once. local-check.ps1 resolves commands through PATH, so
    # this in-process call inherits the provisioned toolchain.
    & powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $RepoRoot 'scripts\local-check.ps1') -Slice ci
    if ($LASTEXITCODE -ne 0) { throw "local-check.ps1 -Slice ci failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

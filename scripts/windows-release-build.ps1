#Requires -Version 5.1
<#
.SYNOPSIS
    Build Windows release artifacts with persistent caches.

.DESCRIPTION
    Creates or updates a clean managed checkout for the requested Git ref, builds
    the Tauri desktop release binary with pnpm and Cargo caches outside the
    source tree, packages the Inno Setup installer, and emits the same release
    assets used by GitHub Releases.

    This is intended for the Windows build server path. It preserves expensive
    build caches between releases without reusing a dirty source checkout.

.PARAMETER Ref
    Git ref to build. Use a tag such as v0.27.4 for release artifacts.

.PARAMETER RepoUrl
    Git repository URL used when the managed checkout does not exist.

.PARAMETER WorkRoot
    Root directory for the managed source checkout, cache, and output assets.

.PARAMETER RefreshInstallerDependencies
    Re-download WebView2 and VC++ bootstrapper files instead of reusing the
    signed cached copies.

.PARAMETER WarmCacheOnly
    Build the desktop binary and stop before installer packaging. Use this to
    warm the Windows Cargo and pnpm caches after a large port.
.PARAMETER SmokeInstall
    After packaging, run scripts/windows-smoke-install.ps1 against the generated
    installer and uninstall it again.


.EXAMPLE
    .\scripts\windows-release-build.ps1 -Ref v0.27.4

#>

param(
    [string]$Ref = "HEAD",
    [string]$RepoUrl = "https://github.com/nesszer/Win-CodexBar.git",
    [string]$WorkRoot = "C:\code\Win-CodexBar-release",
    [switch]$RefreshInstallerDependencies,
    [switch]$WarmCacheOnly,
    [switch]$SmokeInstall
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$env:CARGO_TERM_COLOR = "never"
$env:CARGO_TERM_PROGRESS_WHEN = "never"
$env:NO_COLOR = "1"
trap {
    Write-Host $_
    [Environment]::Exit(1)
}

$SourceDir = Join-Path $WorkRoot "source"
$CacheDir = Join-Path $WorkRoot "cache"
$PnpmStoreDir = Join-Path $CacheDir "pnpm-store"
$InstallerDepsDir = Join-Path $CacheDir "installer-deps"
$AssetsDir = Join-Path $WorkRoot "assets"
$DesktopCargoTargetDir = Join-Path $CacheDir "cargo-target"
$CliCargoTargetDir = Join-Path $CacheDir "cargo-target-cli"

$UserCargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $UserCargoBin) {
    $env:Path = "$UserCargoBin;$env:Path"
}

function Require-Command {
    param([string]$Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if (-not $command) {
        throw "Missing required command: $Name"
    }
    return $command
}

function Invoke-Native {
    param(
        [string]$FilePath,
        [string[]]$ArgumentList
    )

    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath exited with code $LASTEXITCODE"
    }
}

function Get-AppVersion {
    param([string]$CargoTomlPath)

    $line = Get-Content $CargoTomlPath | Where-Object { $_ -match '^version = "([^"]+)"' } | Select-Object -First 1
    if (-not $line -or $line -notmatch '^version = "([^"]+)"') {
        throw "Failed to determine app version from $CargoTomlPath"
    }
    return $Matches[1]
}

function Assert-MicrosoftSignature {
    param([string]$Path)

    if (-not (Test-Path $Path)) {
        throw "Missing installer dependency: $Path"
    }

    $signature = Get-AuthenticodeSignature -FilePath $Path
    if ($signature.Status -ne "Valid") {
        throw "$Path signature is not valid. Status: $($signature.Status)"
    }

    $subject = $signature.SignerCertificate.Subject
    if ($subject -notlike "*Microsoft Corporation*") {
        throw "$Path signer is unexpected: $subject"
    }
}

function Get-InnoSetupCompiler {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe")
    )

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) {
            return $candidate
        }
    }

    $command = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    throw "Inno Setup compiler not found. Install JRSoftware.InnoSetup with winget or Inno Setup 6 from jrsoftware.org."
}

function Invoke-DownloadWithRetry {
    param(
        [string]$Uri,
        [string]$OutFile,
        [int]$Attempts = 3
    )

    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        try {
            Write-Host "Downloading $Uri (attempt $attempt/$Attempts)"
            Invoke-WebRequest -Uri $Uri -OutFile $OutFile
            return
        } catch {
            if ($attempt -eq $Attempts) {
                throw
            }
            Write-Host "Download failed: $($_.Exception.Message)"
            Start-Sleep -Seconds (5 * $attempt)
        }
    }
}

function Get-ObjdumpImportsWebView2Loader {
    param([string]$ExePath)

    $objdump = Get-Command objdump -ErrorAction SilentlyContinue
    if (-not $objdump) {
        return $false
    }

    $output = & $objdump.Source -p $ExePath
    return [bool]($output | Select-String -Pattern "DLL Name: WebView2Loader.dll" -Quiet)
}

$git = Require-Command "git"
$cargo = Require-Command "cargo"
$pnpm = Require-Command "pnpm"
$rustup = Get-Command rustup -ErrorAction SilentlyContinue

New-Item -ItemType Directory -Force $WorkRoot, $CacheDir, $DesktopCargoTargetDir, $CliCargoTargetDir, $PnpmStoreDir, $InstallerDepsDir, $AssetsDir | Out-Null

if (-not (Test-Path (Join-Path $SourceDir ".git"))) {
    if (Test-Path $SourceDir) {
        throw "$SourceDir exists but is not a Git checkout. Move it aside or choose another WorkRoot."
    }
    Invoke-Native $git.Source @("clone", "--quiet", $RepoUrl, $SourceDir)
}

Push-Location $SourceDir
try {
    Invoke-Native $git.Source @("fetch", "--quiet", "--tags", "--prune", "origin")
    Invoke-Native $git.Source @("-c", "advice.detachedHead=false", "checkout", "--quiet", "--force", $Ref)
    Invoke-Native $git.Source @("reset", "--quiet", "--hard", "HEAD")
    Invoke-Native $git.Source @("clean", "-ffdq", "-e", "apps/desktop-tauri/node_modules/")

    $commit = (& $git.Source rev-parse HEAD).Trim()
    $version = Get-AppVersion -CargoTomlPath (Join-Path $SourceDir "rust\Cargo.toml")

    $env:APP_VERSION = $version
    $env:CARGO_TARGET_DIR = $DesktopCargoTargetDir
    if (-not $env:CARGO_BUILD_TARGET -and [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
        $env:CARGO_BUILD_TARGET = "x86_64-pc-windows-msvc"
    }
    if ($env:CARGO_BUILD_TARGET -and $rustup) {
        $toolchain = "stable-x86_64-pc-windows-msvc"
        & $rustup.Source set auto-self-update disable
        if ($LASTEXITCODE -ne 0) {
            Write-Host "Warning: rustup auto-self-update disable failed with exit code $LASTEXITCODE"
        }
        Invoke-Native $rustup.Source @("toolchain", "install", $toolchain, "--profile", "minimal")
        if ($env:CARGO_BUILD_TARGET -ne "x86_64-pc-windows-msvc") {
            Invoke-Native $rustup.Source @("target", "add", $env:CARGO_BUILD_TARGET, "--toolchain", $toolchain)
        }
        $env:RUSTUP_TOOLCHAIN = $toolchain
    }
    $env:PNPM_HOME = if ($env:PNPM_HOME) { $env:PNPM_HOME } else { Join-Path $CacheDir "pnpm-home" }

    Write-Host "Building Win-CodexBar $version from $commit"
    Write-Host "Source: $SourceDir"
    Write-Host "Cargo target cache: $DesktopCargoTargetDir"
    Write-Host "pnpm store cache: $PnpmStoreDir"

    Invoke-Native $pnpm.Source @(
        "--dir", "apps\desktop-tauri",
        "install",
        "--frozen-lockfile",
        "--store-dir", $PnpmStoreDir
    )

    $tauriBuildLog = Join-Path $AssetsDir "tauri-build.log"
    $tauriBuildErrLog = Join-Path $AssetsDir "tauri-build.err.log"
    Write-Host "Running Tauri build. Logs: $tauriBuildLog and $tauriBuildErrLog"
    $tauriBuildArgs = @(
        "--dir",
        "apps\desktop-tauri",
        "exec",
        "tauri",
        "build",
        "--ci",
        "--no-bundle"
    )
    if ($env:CARGO_BUILD_TARGET) {
        $tauriBuildArgs += @("--target", $env:CARGO_BUILD_TARGET)
    }
    $tauriBuildArgs += @("--", "--quiet")
    $quotedArgs = $tauriBuildArgs | ForEach-Object {
        if ($_ -match '[\s"]') {
            '"' + ($_ -replace '"', '\"') + '"'
        } else {
            $_
        }
    }
    $commandLine = "pnpm " + ($quotedArgs -join " ")
    $process = Start-Process -FilePath "cmd.exe" `
        -ArgumentList @("/d", "/s", "/c", $commandLine) `
        -NoNewWindow `
        -PassThru `
        -RedirectStandardOutput $tauriBuildLog `
        -RedirectStandardError $tauriBuildErrLog
    while (-not $process.HasExited) {
        Start-Sleep -Seconds 30
        Write-Host "Tauri build still running..."
        $process.Refresh()
    }
    $process.WaitForExit()
    $process.Refresh()
    $releaseBinDir = if ($env:CARGO_BUILD_TARGET) {
        Join-Path $DesktopCargoTargetDir "$($env:CARGO_BUILD_TARGET)\release"
    } else {
        Join-Path $DesktopCargoTargetDir "release"
    }
    $sourceExe = Join-Path $releaseBinDir "codexbar-desktop-tauri.exe"
    if ($null -eq $process.ExitCode) {
        if (Test-Path $sourceExe) {
            Write-Host "Warning: Tauri build did not report an exit code, but produced $sourceExe."
        } else {
            Write-Host "Tauri build did not report an exit code. Last 200 stdout lines:"
            if (Test-Path $tauriBuildLog) {
                Get-Content $tauriBuildLog -Tail 200
            }
            Write-Host "Last 200 stderr lines:"
            if (Test-Path $tauriBuildErrLog) {
                Get-Content $tauriBuildErrLog -Tail 200
            }
            throw "pnpm tauri build completed without a reliable exit code"
        }
    }
    $tauriExitCode = if ($null -eq $process.ExitCode) { 0 } else { $process.ExitCode }
    if ($tauriExitCode -ne 0) {
        Write-Host "Tauri build failed with exit code $tauriExitCode. Last 200 stdout lines:"
        if (Test-Path $tauriBuildLog) {
            Get-Content $tauriBuildLog -Tail 200
        }
        Write-Host "Last 200 stderr lines:"
        if (Test-Path $tauriBuildErrLog) {
            Get-Content $tauriBuildErrLog -Tail 200
        }
        throw "pnpm tauri build exited with code $tauriExitCode"
    }

    $desktopExe = Join-Path $releaseBinDir "codexbar.exe"
    $legacyDesktopExe = Join-Path $releaseBinDir "codexbar-desktop.exe"
    $releaseExe = Join-Path $releaseBinDir "codexbar-cli.exe"
    if (-not (Test-Path $sourceExe)) {
        throw "Missing expected Tauri binary: $sourceExe"
    }

    Copy-Item $sourceExe $desktopExe -Force
    Copy-Item $sourceExe $legacyDesktopExe -Force
    if (Get-ObjdumpImportsWebView2Loader -ExePath $desktopExe) {
        throw "codexbar.exe imports WebView2Loader.dll, but release builds are expected to statically link the loader."
    }

    $env:CARGO_TARGET_DIR = $CliCargoTargetDir
    Write-Host "Building CLI binary"
    Write-Host "CLI Cargo target cache: $CliCargoTargetDir"
    Invoke-Native $cargo.Source @(
        "build",
        "--manifest-path", "rust\Cargo.toml",
        "--release",
        "--bin", "codexbar"
    )
    $env:CARGO_TARGET_DIR = $DesktopCargoTargetDir

    $cliBinDir = if ($env:CARGO_BUILD_TARGET) {
        Join-Path $CliCargoTargetDir "$($env:CARGO_BUILD_TARGET)\release"
    } else {
        Join-Path $CliCargoTargetDir "release"
    }
    $sourceCliExe = Join-Path $cliBinDir "codexbar.exe"
    if (-not (Test-Path $sourceCliExe)) {
        throw "Missing expected CLI binary: $sourceCliExe"
    }
    Copy-Item $sourceCliExe $releaseExe -Force

    $verifyExecutablesScript = Join-Path $SourceDir "scripts\verify-windows-executables.ps1"
    if (-not (Test-Path $verifyExecutablesScript)) {
        throw "Executable verification script not found: $verifyExecutablesScript"
    }
    & $verifyExecutablesScript `
        -DesktopExe $desktopExe `
        -CliExe $releaseExe `
        -LegacyDesktopExe $legacyDesktopExe `
        -CheckCliStdout

    if ($WarmCacheOnly) {
        $warmExe = Join-Path $AssetsDir "CodexBar-$version-warm.exe"
        Copy-Item $desktopExe $warmExe -Force
        Write-Host ""
        Write-Host "Warm build artifact: $warmExe"
        Write-Host "Warm cache completed. Skipping installer packaging because -WarmCacheOnly was supplied."
        return
    }

    $vcRedistPath = Join-Path $InstallerDepsDir "vc_redist.x64.exe"
    $webView2BootstrapperPath = Join-Path $InstallerDepsDir "MicrosoftEdgeWebview2Setup.exe"

    if ($RefreshInstallerDependencies -or -not (Test-Path $vcRedistPath)) {
        Invoke-DownloadWithRetry -Uri "https://aka.ms/vc14/vc_redist.x64.exe" -OutFile $vcRedistPath
    }
    if ($RefreshInstallerDependencies -or -not (Test-Path $webView2BootstrapperPath)) {
        Invoke-DownloadWithRetry -Uri "https://go.microsoft.com/fwlink/p/?LinkId=2124703" -OutFile $webView2BootstrapperPath
    }

    Assert-MicrosoftSignature -Path $vcRedistPath
    Assert-MicrosoftSignature -Path $webView2BootstrapperPath

    $iscc = Get-InnoSetupCompiler

    $installerOut = Join-Path $CacheDir "installer"
    New-Item -ItemType Directory -Force $installerOut | Out-Null

    Push-Location "rust\installer"
    try {
        Invoke-Native $iscc @(
            "/Qp",
            "/DAppVersion=$version",
            "/DTargetBinDir=$releaseBinDir",
            "/DVCRedistPath=$vcRedistPath",
            "/DWebView2BootstrapperPath=$webView2BootstrapperPath",
            "/DOutputDir=$installerOut",
            "/DOutputBaseFilename=CodexBar-$version-Setup",
            "codexbar.iss"
        )
    } finally {
        Pop-Location
    }

    $installer = Join-Path $installerOut "CodexBar-$version-Setup.exe"
    $portableExe = Join-Path $AssetsDir "CodexBar-$version-portable.exe"
    $installerAsset = Join-Path $AssetsDir "CodexBar-$version-Setup.exe"

    foreach ($path in @($desktopExe, $releaseExe, $installer)) {
        if (-not (Test-Path $path)) {
            throw "Missing expected asset: $path"
        }
    }

    Copy-Item $desktopExe $portableExe -Force
    Copy-Item $installer $installerAsset -Force

    foreach ($asset in @($installerAsset, $portableExe)) {
        $fileName = Split-Path $asset -Leaf
        $hash = (Get-FileHash -Algorithm SHA256 $asset).Hash.ToLower()
        "$hash  $fileName" | Set-Content -Encoding ascii "$asset.sha256"
    }

    if ($SmokeInstall) {
        $smokeScript = Join-Path $SourceDir "scripts\windows-smoke-install.ps1"
        if (-not (Test-Path $smokeScript)) {
            throw "Smoke install script not found: $smokeScript"
        }
        & $smokeScript -InstallerPath $installerAsset -ExpectedVersion $version
        if ($LASTEXITCODE -ne 0) {
            throw "Smoke install failed with exit code $LASTEXITCODE"
        }
    }


    Write-Host ""
    Write-Host "Release assets:"
    Get-ChildItem $AssetsDir -Filter "CodexBar-$version-*" |
        Sort-Object Name |
        Select-Object Name, Length, LastWriteTime |
        Format-Table -AutoSize
} finally {
    Pop-Location
}

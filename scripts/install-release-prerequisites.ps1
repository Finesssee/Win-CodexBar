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
function Refresh-PrerequisitePath {
    $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $paths = @($env:Path, $machinePath, $userPath) | Where-Object {
        -not [string]::IsNullOrWhiteSpace($_)
    }
    if ($paths.Count -gt 0) {
        $env:Path = $paths -join ';'
    }
}

function Add-PrerequisitePath {
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "Cannot add missing tool directory to PATH: $Path"
    }
    if (@($env:Path -split ';' | Where-Object { $_ }) -notcontains $Path) {
        $env:Path = "$Path;$env:Path"
    }
    foreach ($scope in @('Machine', 'User')) {
        try {
            $scopePath = [Environment]::GetEnvironmentVariable('Path', $scope)
            $scopeEntries = @($scopePath -split ';' | Where-Object { $_ })
            if ($scopeEntries -notcontains $Path) {
                [Environment]::SetEnvironmentVariable('Path', "$Path;$scopePath", $scope)
            }
        } catch {
            Write-Warning "Could not persist PATH for $scope scope: $($_.Exception.Message)"
        }
    }
}

function Invoke-PrerequisiteDownload {
    param(
        [Parameter(Mandatory)][string]$Uri,
        [Parameter(Mandatory)][string]$Destination
    )

    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $maxAttempts = 3
    for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
        try {
            Write-Host "Downloading $Uri (attempt $attempt/$maxAttempts)"
            Invoke-WebRequest -UseBasicParsing -Uri $Uri -OutFile $Destination
            return
        } catch {
            if ($attempt -lt $maxAttempts) {
                Write-Warning "Download failed (attempt $attempt/$maxAttempts): $($_.Exception.Message)"
                Start-Sleep -Seconds 10
            } else {
                throw
            }
        }
    }
}

function Assert-PrerequisiteDownloadHash {
    param(
        [Parameter(Mandatory)][string]$Uri,
        [Parameter(Mandatory)][string]$FilePath
    )

    $fileName = Split-Path -Leaf $FilePath
    $checksums = (Invoke-WebRequest -UseBasicParsing -Uri $Uri).Content
    $pattern = "\s$([regex]::Escape($fileName))$"
    $line = @($checksums -split "`r?`n" | Where-Object { $_ -match $pattern } | Select-Object -First 1)
    if ($line.Count -ne 1) {
        throw "No SHA-256 checksum was published for $fileName."
    }
    $expected = ($line[0] -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -LiteralPath $FilePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "SHA-256 mismatch for $fileName (actual $actual, expected $expected)."
    }
    Write-Host "[ok] SHA-256 $fileName"
}

function Install-ChocoPackageFallback {
    param([Parameter(Mandatory)][string]$Id)

    if ($AssertOnly) {
        throw "Missing prerequisite '$Id' (AssertOnly mode does not install packages)."
    }
    $choco = Get-CommandPath 'choco'
    if (-not $choco) {
        throw "choco is unavailable."
    }
    $maxAttempts = 3
    for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
        Write-Host "Installing Chocolatey package $Id (attempt $attempt/$maxAttempts)"
        $previousErrorActionPreference = $ErrorActionPreference
        try {
            $ErrorActionPreference = 'Continue'
            & $choco install $Id --yes --no-progress 2>&1 | ForEach-Object { Write-Host $_ }
            $chocoExit = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $previousErrorActionPreference
        }
        if ($chocoExit -eq 0) {
            Refresh-PrerequisitePath
            return
        }
        if ($attempt -lt $maxAttempts) {
            Write-Warning "choco install $Id exited with code $chocoExit (attempt $attempt/$maxAttempts), retrying in 10s"
            Start-Sleep -Seconds 10
        } else {
            throw "choco install $Id exited with code $chocoExit after $maxAttempts attempts."
        }
    }
}

function Install-PackageWithFallback {
    param(
        [Parameter(Mandatory)][string]$WingetId,
        [Parameter(Mandatory)][string]$ChocoId
    )

    if ($AssertOnly) {
        throw "Missing prerequisite '$WingetId' (AssertOnly mode does not install packages)."
    }
    $attempts = New-Object System.Collections.Generic.List[string]
    if (Get-CommandPath 'winget') {
        try {
            Install-WingetPackage $WingetId | Out-Null
            Refresh-PrerequisitePath
            return
        } catch {
            $attempts.Add("winget: $($_.Exception.Message)")
        }
    } else {
        $attempts.Add('winget: unavailable')
    }
    if (Get-CommandPath 'choco') {
        try {
            Install-ChocoPackageFallback $ChocoId | Out-Null
            return
        } catch {
            $attempts.Add("choco: $($_.Exception.Message)")
        }
    } else {
        $attempts.Add('choco: unavailable')
    }
    if ($WingetId -eq 'Rustlang.Rustup') {
        try {
            $rustupInit = Join-Path ([IO.Path]::GetTempPath()) 'rustup-init.exe'
            Invoke-PrerequisiteDownload 'https://win.rustup.rs/x86_64' $rustupInit
            Write-Host 'Installing rustup via official rustup-init.exe'
            $previousErrorActionPreference = $ErrorActionPreference
            try {
                $ErrorActionPreference = 'Continue'
                & $rustupInit '-y' '--default-toolchain' 'stable-x86_64-pc-windows-msvc' '--profile' 'default' '--no-modify-path' 2>&1 | ForEach-Object { Write-Host $_ }
                $rustupInitExit = $LASTEXITCODE
            } finally {
                $ErrorActionPreference = $previousErrorActionPreference
            }
            if ($rustupInitExit -ne 0) {
                throw "rustup-init.exe exited with code $rustupInitExit."
            }
            $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
            if (Test-Path -LiteralPath $cargoBin -PathType Container) {
                Add-PrerequisitePath $cargoBin
            }
            Refresh-PrerequisitePath
            return
        } catch {
            $attempts.Add("rustup-init: $($_.Exception.Message)")
        }
    }
    throw "Unable to provision '$WingetId'/'$ChocoId'. $($attempts -join '; ')"
}

function Require-PrerequisiteCommand {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$WingetId,
        [Parameter(Mandatory)][string]$ChocoId
    )

    $path = Get-CommandPath $Name
    if (-not $path) {
        Install-PackageWithFallback $WingetId $ChocoId | Out-Null
        $path = Get-CommandPath $Name
    }
    if (-not $path) {
        throw "Required command '$Name' is still unavailable after provisioning '$WingetId'."
    }
    Write-Host "[ok] $($Name): $path"
    return $path
}

function Get-NodeInfoFallback {
    Refresh-PrerequisitePath
    $path = Get-CommandPath 'node'
    if (-not $path) {
        $candidates = @(
            (Join-Path $env:ProgramFiles 'nodejs\node.exe'),
            (Join-Path ${env:ProgramFiles(x86)} 'nodejs\node.exe'),
            (Join-Path $env:LOCALAPPDATA 'Programs\nodejs\node.exe')
        )
        foreach ($candidate in $candidates) {
            if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
                $path = $candidate
                break
            }
        }
    }
    if (-not $path) { return $null }
    $version = (& $path --version).Trim()
    $major = $null
    try {
        $major = Get-NodeMajor $version
    } catch {
        $major = $null
    }
    return [pscustomobject]@{
        Path = $path
        Version = $version
        Major = $major
    }
}

function Install-NodeMsiFallback {
    param([Parameter(Mandatory)][string]$Version)

    $root = Join-Path ([IO.Path]::GetTempPath()) 'codexbar-node-bootstrap'
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    $fileName = "node-$Version-x64.msi"
    $installer = Join-Path $root $fileName
    $baseUri = "https://nodejs.org/dist/$Version"
    Invoke-PrerequisiteDownload "$baseUri/$fileName" $installer
    Assert-PrerequisiteDownloadHash "$baseUri/SHASUMS256.txt" $installer
    $msiexec = Join-Path $env:WINDIR 'System32\msiexec.exe'
    if (-not (Test-Path -LiteralPath $msiexec -PathType Leaf)) {
        throw "msiexec.exe is unavailable at $msiexec."
    }
    Write-Host "Installing Node $Version from MSI"
    & $msiexec '/i' $installer '/quiet' '/norestart'
    if (($LASTEXITCODE -ne 0) -and ($LASTEXITCODE -ne 3010)) {
        throw "msiexec.exe exited with code $LASTEXITCODE."
    }
    Refresh-PrerequisitePath
}

function Install-NodeZipFallback {
    param([Parameter(Mandatory)][string]$Version)

    $root = Join-Path $env:LOCALAPPDATA 'CodexBar\release-toolchain\node'
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    $staging = Join-Path ([IO.Path]::GetTempPath()) 'codexbar-node-bootstrap'
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    $fileName = "node-$Version-win-x64.zip"
    $archive = Join-Path $staging $fileName
    $baseUri = "https://nodejs.org/dist/$Version"
    Invoke-PrerequisiteDownload "$baseUri/$fileName" $archive
    Assert-PrerequisiteDownloadHash "$baseUri/SHASUMS256.txt" $archive
    $installRoot = Join-Path $root $Version
    Expand-Archive -LiteralPath $archive -DestinationPath $installRoot -Force
    $nodeDirectory = Join-Path $installRoot "node-$Version-win-x64"
    Add-PrerequisitePath $nodeDirectory
}

function Install-NodeWithFallback {
    param(
        [Parameter(Mandatory)][string]$Version,
        [Parameter(Mandatory)][string]$WingetId,
        [Parameter(Mandatory)][int]$RequiredMajor
    )

    if ($AssertOnly) {
        throw "Node $RequiredMajor.x is required; AssertOnly mode does not install packages."
    }
    $attempts = New-Object System.Collections.Generic.List[string]
    if (Get-CommandPath 'winget') {
        try {
            Install-WingetPackage $WingetId -Upgrade | Out-Null
            $info = Get-NodeInfoFallback
            if ($info -and $info.Major -eq $RequiredMajor) { return }
            $found = if ($info) { $info.Version } else { 'unavailable' }
            $attempts.Add("winget: installed $found, expected major $RequiredMajor")
        } catch {
            $attempts.Add("winget: $($_.Exception.Message)")
        }
    } else {
        $attempts.Add('winget: unavailable')
    }
    try {
        Install-NodeMsiFallback $Version | Out-Null
        $info = Get-NodeInfoFallback
        if ($info -and $info.Major -eq $RequiredMajor) { return }
        $found = if ($info) { $info.Version } else { 'unavailable' }
        $attempts.Add("MSI: installed $found, expected major $RequiredMajor")
    } catch {
        $attempts.Add("MSI: $($_.Exception.Message)")
    }
    if (Get-CommandPath 'choco') {
        try {
            Install-ChocoPackageFallback 'nodejs-lts' | Out-Null
            $info = Get-NodeInfoFallback
            if ($info -and $info.Major -eq $RequiredMajor) { return }
            $found = if ($info) { $info.Version } else { 'unavailable' }
            $attempts.Add("choco: installed $found, expected major $RequiredMajor")
        } catch {
            $attempts.Add("choco: $($_.Exception.Message)")
        }
    } else {
        $attempts.Add('choco: unavailable')
    }
    try {
        Install-NodeZipFallback $Version | Out-Null
        $info = Get-NodeInfoFallback
        if ($info -and $info.Major -eq $RequiredMajor) { return }
        $found = if ($info) { $info.Version } else { 'unavailable' }
        $attempts.Add("zip: installed $found, expected major $RequiredMajor")
    } catch {
        $attempts.Add("zip: $($_.Exception.Message)")
    }
    throw "Unable to provision Node $RequiredMajor.x. $($attempts -join '; ')"
}

function Install-InnoSetupFallback {
    if ($AssertOnly) {
        throw 'Inno Setup 6 ISCC.exe is unavailable.'
    }
    $attempts = New-Object System.Collections.Generic.List[string]
    if (Get-CommandPath 'winget') {
        try {
            Install-WingetPackage 'JRSoftware.InnoSetup' | Out-Null
            $iscc = Get-InnoSetupCompiler
            if ($iscc) { return $iscc }
            $attempts.Add('winget: ISCC.exe was still unavailable')
        } catch {
            $attempts.Add("winget: $($_.Exception.Message)")
        }
    } else {
        $attempts.Add('winget: unavailable')
    }
    if (Get-CommandPath 'choco') {
        try {
            Install-ChocoPackageFallback 'innosetup' | Out-Null
            $iscc = Get-InnoSetupCompiler
            if ($iscc) { return $iscc }
            $attempts.Add('choco: ISCC.exe was still unavailable')
        } catch {
            $attempts.Add("choco: $($_.Exception.Message)")
        }
    } else {
        $attempts.Add('choco: unavailable')
    }
    $innoMaxAttempts = 3
    for ($innoAttempt = 1; $innoAttempt -le $innoMaxAttempts; $innoAttempt++) {
        try {
            $root = Join-Path ([IO.Path]::GetTempPath()) 'codexbar-inno-bootstrap'
            New-Item -ItemType Directory -Force -Path $root | Out-Null
            $installer = Join-Path $root 'innosetup.exe'
            Invoke-PrerequisiteDownload 'https://jrsoftware.org/download.php/is.exe' $installer
            Write-Host 'Installing Inno Setup 6 from jrsoftware.org (attempt $innoAttempt/$innoMaxAttempts)'
            & $installer '/VERYSILENT' '/SUPPRESSMSGBOXES' '/NORESTART' '/SP-'
            if ($LASTEXITCODE -ne 0) {
                throw "Inno Setup installer exited with code $LASTEXITCODE."
            }
            Refresh-PrerequisitePath
            $iscc = Get-InnoSetupCompiler
            if ($iscc) { return $iscc }
            $attempts.Add('official installer: ISCC.exe was still unavailable')
            break
        } catch {
            $attempts.Add("official installer (attempt $innoAttempt): $($_.Exception.Message)")
            if ($innoAttempt -lt $innoMaxAttempts) {
                Write-Warning "Inno Setup install failed (attempt $innoAttempt/$innoMaxAttempts), retrying in 10s"
                Start-Sleep -Seconds 10
            }
        }
    }
    throw "Unable to provision Inno Setup 6. $($attempts -join '; ')"
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

Require-PrerequisiteCommand 'git' 'Git.Git' 'git' | Out-Null
$requiredNodeMajor = 24
$nodePackageId = 'OpenJS.NodeJS.LTS'
$nodeFallbackVersion = 'v24.18.0'
$nodeInfo = Get-NodeInfoFallback
if (-not $nodeInfo -or $nodeInfo.Major -ne $requiredNodeMajor) {
    if ($AssertOnly) {
        $found = if ($nodeInfo) { $nodeInfo.Version } else { 'unavailable' }
        throw "Node $requiredNodeMajor.x is required; found $found."
    }
    Install-NodeWithFallback $nodeFallbackVersion $nodePackageId $requiredNodeMajor
    $nodeInfo = Get-NodeInfoFallback
}
if (-not $nodeInfo) {
    throw "Required command 'node' is still unavailable after provisioning '$nodePackageId'."
}
Assert-NodeMajor $nodeInfo.Version $requiredNodeMajor | Out-Null
$nodeDirectory = Split-Path -Parent $nodeInfo.Path
Add-PrerequisitePath $nodeDirectory
Write-Host "[ok] Node $($nodeInfo.Version) (major $requiredNodeMajor)"

$corepack = Get-CommandPath 'corepack'
if (-not $corepack) {
    $nodeDirectory = Split-Path -Parent $nodeInfo.Path
    foreach ($candidate in @(
        (Join-Path $nodeDirectory 'corepack.cmd'),
        (Join-Path $nodeDirectory 'corepack.ps1'),
        (Join-Path $nodeDirectory 'corepack')
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $corepack = $candidate
            break
        }
    }
}
if (-not $corepack) {
    throw "Corepack is required with Node $requiredNodeMajor to activate pinned pnpm $expectedPnpm."
}
$pnpmShimDir = Join-Path $env:LOCALAPPDATA 'CodexBar\release-toolchain\pnpm'
if (-not $AssertOnly) {
    New-Item -ItemType Directory -Force -Path $pnpmShimDir | Out-Null
    Invoke-Native $corepack @('enable', '--install-directory', $pnpmShimDir)
    Add-PrerequisitePath $pnpmShimDir
    Invoke-Native $corepack @('prepare', "pnpm@$expectedPnpm", '--activate')
}
$pnpm = Get-CommandPath 'pnpm'
if (-not $pnpm) {
    $pnpmCandidate = Join-Path $pnpmShimDir 'pnpm.cmd'
    if (Test-Path -LiteralPath $pnpmCandidate -PathType Leaf) {
        $pnpm = $pnpmCandidate
    }
}
if (-not $pnpm) {
    throw "pnpm $expectedPnpm is unavailable after Corepack provisioning."
}
$pnpmVersion = (& $pnpm --version).Trim()
if ($pnpmVersion -ne $expectedPnpm) {
    throw "pnpm $pnpmVersion is active; expected exact $expectedPnpm."
}
Write-Host "[ok] pnpm $pnpmVersion"

Require-PrerequisiteCommand 'cargo' 'Rustlang.Rustup' 'rustup.install' | Out-Null
Require-PrerequisiteCommand 'rustc' 'Rustlang.Rustup' 'rustup.install' | Out-Null
$rustup = Require-PrerequisiteCommand 'rustup' 'Rustlang.Rustup' 'rustup.install'
$toolchain = 'stable-x86_64-pc-windows-msvc'
$target = 'x86_64-pc-windows-msvc'
if (-not $AssertOnly) {
    & $rustup default $toolchain
    if ($LASTEXITCODE -ne 0) {
        throw "$rustup default $toolchain exited with code $LASTEXITCODE"
    }
}
$installedTargets = @(& $rustup target list --installed --toolchain $toolchain)
if ($LASTEXITCODE -ne 0) {
    throw "$rustup target list --installed --toolchain $toolchain exited with code $LASTEXITCODE"
}
if ($installedTargets -notcontains $target) {
    if ($AssertOnly) {
        throw "Rust target $target is not installed for $toolchain."
    }
    & $rustup target add $target --toolchain $toolchain
    if ($LASTEXITCODE -ne 0) {
        throw "$rustup target add $target --toolchain $toolchain exited with code $LASTEXITCODE"
    }
}
Write-Host "[ok] Rust target $target ($toolchain)"

$iscc = Get-InnoSetupCompiler
if (-not $iscc) {
    $iscc = Install-InnoSetupFallback
}
if (-not $iscc) {
    throw 'Inno Setup 6 ISCC.exe is unavailable after provisioning.'
}
$innoVersion = (Get-Item -LiteralPath $iscc).VersionInfo.FileVersion
if ($innoVersion -notmatch '^6\.') {
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $compilerHelp = (& $iscc '/?' 2>&1 | Out-String)
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($compilerHelp -notmatch '(?i)(Inno Setup 6|Usage:\s+iscc)') {
        throw "Inno Setup 6.x is required; found '$innoVersion'."
    }
    $innoVersion = '6.x (compiler verified)'
}
Write-Host "[ok] Inno Setup $innoVersion ($iscc)"

Write-Host ''
Write-Host "Release prerequisites passed (Git, Node $requiredNodeMajor, pnpm 10.18.1, Rust MSVC target, Inno Setup 6)."

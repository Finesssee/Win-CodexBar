#Requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('pnpm-11.24','pnpm-11.25','bun')][string]$Manager,
    [string]$RepoRoot = '',
    [string]$CacheRoot = '',
    [string]$OutputDir = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$PnpmCurrentVersion = '11.24.0'
$PnpmLatestVersion = '11.25.0'
$BunVersion = '1.4.2'
$BunWindowsX64Sha256 = 'ce4c17497b2f29712a99d3d53f028de28cd42e3bacb8589599e7f000e49b6405'
$NodeVersion = '24.18.0'
$NodeMajor = 24

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = Split-Path -Parent $PSScriptRoot
}
if ([string]::IsNullOrWhiteSpace($CacheRoot)) {
    $CacheRoot = Join-Path $RepoRoot '.ci-pm-cache'
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $RepoRoot 'benchmark-results'
}

function Invoke-Native {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [Parameter()][string[]]$ArgumentList = @()
    )
    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath exited with code $LASTEXITCODE"
    }
}


function Get-DirectoryStats {
    param([Parameter(Mandatory)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        return [pscustomobject]@{ bytes = 0; files = 0 }
    }
    $measure = Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum
    return [pscustomobject]@{
        bytes = if ($null -eq $measure.Sum) { [int64]0 } else { [int64]$measure.Sum }
        files = [int]$measure.Count
    }
}

function Ensure-Node24 {
    $nodeCommand = Get-Command node.exe -ErrorAction SilentlyContinue
    $nodeVersion = if ($nodeCommand) { (& $nodeCommand.Source --version).Trim() } else { '' }
    $activeMajor = 0
    if ($nodeVersion -match '^v(\d+)\.') { $activeMajor = [int]$Matches[1] }
    if ($activeMajor -eq $NodeMajor) {
        return $nodeCommand.Source
    }

    $msiName = "node-v$NodeVersion-x64.msi"
    $nodeMsi = Join-Path $env:TEMP $msiName
    $sumsPath = Join-Path $env:TEMP "node-v$NodeVersion-SHASUMS256.txt"
    Invoke-Native -FilePath 'curl.exe' -ArgumentList @('-sSfL','-o',$nodeMsi,"https://nodejs.org/dist/v$NodeVersion/$msiName")
    Invoke-Native -FilePath 'curl.exe' -ArgumentList @('-sSfL','-o',$sumsPath,"https://nodejs.org/dist/v$NodeVersion/SHASUMS256.txt")
    $line = Get-Content -LiteralPath $sumsPath | Where-Object { $_ -match "\s+$([regex]::Escape($msiName))$" } | Select-Object -First 1
    if (-not $line -or $line -notmatch '^([a-fA-F0-9]{64})\s+') { throw "Missing checksum for $msiName" }
    $expected = $Matches[1].ToLowerInvariant()
    $actual = (Get-FileHash -LiteralPath $nodeMsi -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { throw "Node MSI SHA-256 mismatch: $actual != $expected" }

    $nodeDir = "C:\node-v$NodeVersion"
    $proc = Start-Process msiexec.exe -ArgumentList @('/i',$nodeMsi,'/qn','/norestart',"INSTALLDIR=$nodeDir") -Wait -PassThru
    if ($proc.ExitCode -ne 0) { throw "Node MSI install exited with code $($proc.ExitCode)" }
    $nodeExe = Join-Path $nodeDir 'node.exe'
    if (-not (Test-Path -LiteralPath $nodeExe -PathType Leaf)) { throw 'node.exe missing after MSI install' }
    $env:Path = "$nodeDir;$env:Path"
    return $nodeExe
}

function Install-Bun {
    $toolRoot = Join-Path $env:LOCALAPPDATA "CodexBar\pm-benchmark\bun-$BunVersion"
    $bunExe = Join-Path $toolRoot 'bun-windows-x64\bun.exe'
    if (-not (Test-Path -LiteralPath $bunExe -PathType Leaf)) {
        New-Item -ItemType Directory -Force -Path $toolRoot | Out-Null
        $zip = Join-Path $env:TEMP "bun-v$BunVersion-windows-x64.zip"
        Invoke-Native -FilePath 'curl.exe' -ArgumentList @('-sSfL','-o',$zip,"https://github.com/oven-sh/bun/releases/download/bun-v$BunVersion/bun-windows-x64.zip")
        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $BunWindowsX64Sha256) { throw "Bun ZIP SHA-256 mismatch: $actual != $BunWindowsX64Sha256" }
        Expand-Archive -LiteralPath $zip -DestinationPath $toolRoot -Force
    }
    $version = (& $bunExe --version).Trim()
    if ($version -ne $BunVersion) { throw "Bun $version active; expected $BunVersion" }
    return $bunExe
}

New-Item -ItemType Directory -Force -Path $CacheRoot, $OutputDir | Out-Null
$appDir = Join-Path $RepoRoot 'apps\desktop-tauri'
$packageJsonPath = Join-Path $appDir 'package.json'
if (-not (Test-Path -LiteralPath $packageJsonPath -PathType Leaf)) { throw 'desktop package.json missing' }
if (Test-Path -LiteralPath (Join-Path $appDir 'node_modules')) { throw 'Benchmark requires a fresh checkout without node_modules' }

$pnpmVersion = $null
if ($Manager -eq 'pnpm-11.24') { $pnpmVersion = $PnpmCurrentVersion }
if ($Manager -eq 'pnpm-11.25') {
    $pnpmVersion = $PnpmLatestVersion
    $packageJson = [IO.File]::ReadAllText($packageJsonPath)
    $expected = '"packageManager": "pnpm@11.24.0"'
    $replacement = '"packageManager": "pnpm@11.25.0"'
    if (-not $packageJson.Contains($expected)) { throw 'Expected pnpm 11.24.0 packageManager pin before 11.25 benchmark' }
    [IO.File]::WriteAllText($packageJsonPath, $packageJson.Replace($expected, $replacement), [Text.UTF8Encoding]::new($false))
}

$overall = [Diagnostics.Stopwatch]::StartNew()
$nodeExe = ''
$managerExe = ''
$managerVersion = ''
$sw = [Diagnostics.Stopwatch]::StartNew()
$nodeExe = Ensure-Node24
if ($null -ne $pnpmVersion) {
    $corepack = Get-Command corepack.cmd -ErrorAction SilentlyContinue
    if (-not $corepack) { $corepack = Get-Command corepack.exe -ErrorAction SilentlyContinue }
    if (-not $corepack) { throw 'Corepack is required for the pnpm benchmark' }
    $shimDir = Join-Path $env:LOCALAPPDATA "CodexBar\pm-benchmark\pnpm-$pnpmVersion"
    New-Item -ItemType Directory -Force -Path $shimDir | Out-Null
    Invoke-Native -FilePath $corepack.Source -ArgumentList @('enable','--install-directory',$shimDir)
    Invoke-Native -FilePath $corepack.Source -ArgumentList @('prepare',"pnpm@$pnpmVersion",'--activate')
    $env:Path = "$shimDir;$env:Path"
    $managerExe = Join-Path $shimDir 'pnpm.cmd'
    if (-not (Test-Path -LiteralPath $managerExe -PathType Leaf)) { throw 'pnpm shim missing after Corepack enable' }
    $managerVersion = (& $managerExe --version).Trim()
    if ($managerVersion -ne $pnpmVersion) { throw "pnpm $managerVersion active; expected $pnpmVersion" }
} else {
    $managerExe = Install-Bun
    $managerVersion = (& $managerExe --version).Trim()
}
$sw.Stop()
$provisionSeconds = [Math]::Round($sw.Elapsed.TotalSeconds, 3)

Push-Location $appDir
try {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    if ($null -ne $pnpmVersion) {
        Invoke-Native -FilePath $managerExe -ArgumentList @('install','--frozen-lockfile','--store-dir',(Join-Path $CacheRoot 'pnpm-store'))
    } else {
        Invoke-Native -FilePath $managerExe -ArgumentList @('ci','--cache-dir',(Join-Path $CacheRoot 'bun-cache'))
    }
    $sw.Stop()
    $installSeconds = [Math]::Round($sw.Elapsed.TotalSeconds, 3)

    $vitest = Join-Path $appDir 'node_modules\vitest\vitest.mjs'
    $tsc = Join-Path $appDir 'node_modules\typescript\bin\tsc'
    $vite = Join-Path $appDir 'node_modules\vite\bin\vite.js'
    foreach ($tool in @($vitest,$tsc,$vite)) {
        if (-not (Test-Path -LiteralPath $tool -PathType Leaf)) { throw "Missing installed tool: $tool" }
    }

    $sw = [Diagnostics.Stopwatch]::StartNew()
    Invoke-Native -FilePath $nodeExe -ArgumentList @($vitest,'run')
    $sw.Stop()
    $testSeconds = [Math]::Round($sw.Elapsed.TotalSeconds, 3)

    $sw = [Diagnostics.Stopwatch]::StartNew()
    Invoke-Native -FilePath $nodeExe -ArgumentList @('scripts/check-locale-drift.mjs')
    Invoke-Native -FilePath $nodeExe -ArgumentList @($tsc,'--noEmit')
    Invoke-Native -FilePath $nodeExe -ArgumentList @($vite,'build')
    $sw.Stop()
    $buildSeconds = [Math]::Round($sw.Elapsed.TotalSeconds, 3)
} finally {
    Pop-Location
}
$overall.Stop()

$cacheStats = Get-DirectoryStats -Path $CacheRoot
$nodeModulesStats = Get-DirectoryStats -Path (Join-Path $appDir 'node_modules')
$lockPath = if ($null -ne $pnpmVersion) { Join-Path $appDir 'pnpm-lock.yaml' } else { Join-Path $appDir 'bun.lock' }
$lockHash = (Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash.ToLowerInvariant()
$nodeVersionFinal = (& $nodeExe --version).Trim()

$result = [ordered]@{
    schema_version = 1
    manager = $Manager
    manager_version = $managerVersion
    node_version = $nodeVersionFinal
    circle_build_num = $env:CIRCLE_BUILD_NUM
    circle_workflow_id = $env:CIRCLE_WORKFLOW_ID
    circle_sha1 = $env:CIRCLE_SHA1
    circle_branch = $env:CIRCLE_BRANCH
    provision_seconds = $provisionSeconds
    install_seconds = $installSeconds
    test_seconds = $testSeconds
    build_seconds = $buildSeconds
    benchmark_wall_seconds = [Math]::Round($overall.Elapsed.TotalSeconds, 3)
    cache_bytes = $cacheStats.bytes
    cache_files = $cacheStats.files
    node_modules_bytes = $nodeModulesStats.bytes
    node_modules_files = $nodeModulesStats.files
    lock_sha256 = $lockHash
}

$outPath = Join-Path $OutputDir "$Manager.json"
$result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $outPath -Encoding UTF8
Write-Host 'PM_BENCHMARK_RESULT_BEGIN'
$result | ConvertTo-Json -Depth 4
Write-Host 'PM_BENCHMARK_RESULT_END'

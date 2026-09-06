#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$RepoRoot = '',
    [string]$CacheRoot = '',
    [string]$OutputDir = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$BunVersion = '1.4.2'
$BunWindowsX64Sha256 = 'ce4c17497b2f29712a99d3d53f028de28cd42e3bacb8589599e7f000e49b6405'

if ([string]::IsNullOrWhiteSpace($RepoRoot)) { $RepoRoot = Split-Path -Parent $PSScriptRoot }
if ([string]::IsNullOrWhiteSpace($CacheRoot)) { $CacheRoot = Join-Path $RepoRoot '.ci-bun-runtime-cache' }
if ([string]::IsNullOrWhiteSpace($OutputDir)) { $OutputDir = Join-Path $RepoRoot 'bun-runtime-results' }

function Invoke-RequiredNative {
    param([Parameter(Mandatory)][string]$FilePath, [string[]]$ArgumentList = @())
    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) { throw "$FilePath exited with code $LASTEXITCODE" }
}

function Install-Bun {
    $toolRoot = Join-Path $env:LOCALAPPDATA "CodexBar\bun-runtime-benchmark\bun-$BunVersion"
    $bunExe = Join-Path $toolRoot 'bun-windows-x64\bun.exe'
    if (-not (Test-Path -LiteralPath $bunExe -PathType Leaf)) {
        New-Item -ItemType Directory -Force -Path $toolRoot | Out-Null
        $zip = Join-Path $env:TEMP "bun-v$BunVersion-windows-x64.zip"
        Invoke-RequiredNative 'curl.exe' @('-sSfL','-o',$zip,"https://github.com/oven-sh/bun/releases/download/bun-v$BunVersion/bun-windows-x64.zip")
        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $BunWindowsX64Sha256) { throw "Bun ZIP SHA-256 mismatch: $actual != $BunWindowsX64Sha256" }
        Expand-Archive -LiteralPath $zip -DestinationPath $toolRoot -Force
    }
    $version = (& $bunExe --version).Trim()
    if ($version -ne $BunVersion) { throw "Bun $version active; expected $BunVersion" }
    return $bunExe
}

function Remove-NodeToolchainFromPath {
    param([Parameter(Mandatory)][string]$BunDir)
    $kept = New-Object System.Collections.Generic.List[string]
    foreach ($entry in ($env:Path -split ';')) {
        if ([string]::IsNullOrWhiteSpace($entry)) { continue }
        $trimmed = $entry.Trim()
        $hasNode = Test-Path -LiteralPath (Join-Path $trimmed 'node.exe') -PathType Leaf
        $hasNpm = Test-Path -LiteralPath (Join-Path $trimmed 'npm.cmd') -PathType Leaf
        $hasCorepack = Test-Path -LiteralPath (Join-Path $trimmed 'corepack.cmd') -PathType Leaf
        $hasPnpm = Test-Path -LiteralPath (Join-Path $trimmed 'pnpm.cmd') -PathType Leaf
        if ($hasNode -or $hasNpm -or $hasCorepack -or $hasPnpm) { continue }
        $kept.Add($trimmed)
    }
    $env:Path = [string]::Join(';', @($BunDir) + $kept.ToArray())
}

function Assert-CommandAbsent {
    param([Parameter(Mandatory)][string]$Name)
    if (Get-Command $Name -ErrorAction SilentlyContinue) { throw "$Name is still reachable after Node toolchain removal" }
}

function Invoke-ProbeProcess {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$FilePath,
        [Parameter()][string[]]$ArgumentList = @(),
        [Parameter(Mandatory)][string]$WorkingDirectory
    )
    $logDir = Join-Path $OutputDir 'logs'
    New-Item -ItemType Directory -Force -Path $logDir | Out-Null
    $stdoutPath = Join-Path $logDir "$Name.stdout.log"
    $stderrPath = Join-Path $logDir "$Name.stderr.log"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $proc = Start-Process -FilePath $FilePath -ArgumentList $ArgumentList -WorkingDirectory $WorkingDirectory -Wait -PassThru -NoNewWindow -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
    $sw.Stop()
    $seconds = [Math]::Round($sw.Elapsed.TotalSeconds, 3)
    Write-Host "--- $Name exit=$($proc.ExitCode) seconds=$seconds ---"
    if (Test-Path -LiteralPath $stdoutPath) {
        Get-Content -LiteralPath $stdoutPath | Select-Object -Last 20 | ForEach-Object { Write-Host $_ }
    }
    if (Test-Path -LiteralPath $stderrPath) {
        Get-Content -LiteralPath $stderrPath | Select-Object -Last 30 | ForEach-Object { Write-Host $_ }
    }
    return [pscustomobject]@{ name = $Name; exit_code = [int]$proc.ExitCode; seconds = $seconds }
}

New-Item -ItemType Directory -Force -Path $CacheRoot, $OutputDir | Out-Null
$appDir = Join-Path $RepoRoot 'apps\desktop-tauri'
$packageJsonPath = Join-Path $appDir 'package.json'
$tauriConfigPath = Join-Path $appDir 'src-tauri\tauri.conf.json'
if (-not (Test-Path $packageJsonPath -PathType Leaf)) { throw 'package.json missing' }
if (-not (Test-Path $tauriConfigPath -PathType Leaf)) { throw 'tauri.conf.json missing' }
if (Test-Path (Join-Path $appDir 'node_modules')) { throw 'Benchmark requires a fresh checkout without node_modules' }

$utf8 = New-Object System.Text.UTF8Encoding($false)
$packageJson = [IO.File]::ReadAllText($packageJsonPath)
$replacements = @{
    '"packageManager": "pnpm@11.24.0"' = '"packageManager": "bun@1.4.2"'
    '"check-locale": "node scripts/check-locale-drift.mjs"' = '"check-locale": "bun scripts/check-locale-drift.mjs"'
    '"prebuild": "pnpm run check-locale"' = '"prebuild": "bun run check-locale"'
}
foreach ($pair in $replacements.GetEnumerator()) {
    if (-not $packageJson.Contains($pair.Key)) { throw "Expected package.json text missing: $($pair.Key)" }
    $packageJson = $packageJson.Replace($pair.Key, $pair.Value)
}
[IO.File]::WriteAllText($packageJsonPath, $packageJson, $utf8)

$tauriConfig = [IO.File]::ReadAllText($tauriConfigPath)
foreach ($pair in @(
    @('"beforeDevCommand": "pnpm run dev"','"beforeDevCommand": "bun run dev"'),
    @('"beforeBuildCommand": "pnpm run build"','"beforeBuildCommand": "bun run build"')
)) {
    if (-not $tauriConfig.Contains($pair[0])) { throw "Expected Tauri config text missing: $($pair[0])" }
    $tauriConfig = $tauriConfig.Replace($pair[0], $pair[1])
}
[IO.File]::WriteAllText($tauriConfigPath, $tauriConfig, $utf8)

$overall = [Diagnostics.Stopwatch]::StartNew()
$provisionSw = [Diagnostics.Stopwatch]::StartNew()
$bunExe = Install-Bun
$bunDir = Split-Path -Parent $bunExe
Remove-NodeToolchainFromPath -BunDir $bunDir
foreach ($name in @('node.exe','npm.cmd','npx.cmd','corepack.cmd','pnpm.cmd')) { Assert-CommandAbsent $name }
$provisionSw.Stop()

Push-Location $appDir
try {
    $install = Invoke-ProbeProcess -Name 'bun-ci' -FilePath $bunExe -ArgumentList @('ci','--cache-dir',(Join-Path $CacheRoot 'bun-cache')) -WorkingDirectory $appDir
    if ($install.exit_code -ne 0) { throw "bun ci exited with code $($install.exit_code)" }

    $vitest = Invoke-ProbeProcess -Name 'vitest-under-bun-runtime' -FilePath $bunExe -ArgumentList @('--bun','run','test') -WorkingDirectory $appDir
    $build = Invoke-ProbeProcess -Name 'production-build-under-bun-runtime' -FilePath $bunExe -ArgumentList @('--bun','run','build') -WorkingDirectory $appDir
    $tauri = Invoke-ProbeProcess -Name 'tauri-cli-under-bun-toolchain' -FilePath $bunExe -ArgumentList @('--bun','x','tauri','--version') -WorkingDirectory $appDir
} finally {
    Pop-Location
}
$overall.Stop()

$nodeStillAbsent = -not [bool](Get-Command node.exe -ErrorAction SilentlyContinue)
$npmStillAbsent = -not [bool](Get-Command npm.cmd -ErrorAction SilentlyContinue)
$corepackStillAbsent = -not [bool](Get-Command corepack.cmd -ErrorAction SilentlyContinue)
$pnpmStillAbsent = -not [bool](Get-Command pnpm.cmd -ErrorAction SilentlyContinue)
$compatible = $nodeStillAbsent -and $npmStillAbsent -and $corepackStillAbsent -and $pnpmStillAbsent -and $vitest.exit_code -eq 0 -and $build.exit_code -eq 0 -and $tauri.exit_code -eq 0

$result = [ordered]@{
    schema_version = 1
    bun_version = (& $bunExe --version).Trim()
    circle_build_num = $env:CIRCLE_BUILD_NUM
    circle_workflow_id = $env:CIRCLE_WORKFLOW_ID
    circle_sha1 = $env:CIRCLE_SHA1
    circle_branch = $env:CIRCLE_BRANCH
    node_absent = $nodeStillAbsent
    npm_absent = $npmStillAbsent
    corepack_absent = $corepackStillAbsent
    pnpm_absent = $pnpmStillAbsent
    provision_seconds = [Math]::Round($provisionSw.Elapsed.TotalSeconds, 3)
    install_seconds = $install.seconds
    vitest_under_bun_exit = $vitest.exit_code
    vitest_under_bun_seconds = $vitest.seconds
    build_under_bun_exit = $build.exit_code
    build_under_bun_seconds = $build.seconds
    tauri_cli_exit = $tauri.exit_code
    tauri_cli_seconds = $tauri.seconds
    benchmark_wall_seconds = [Math]::Round($overall.Elapsed.TotalSeconds, 3)
    fully_bun_compatible = $compatible
}

$outPath = Join-Path $OutputDir 'bun-only-runtime.json'
$result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $outPath -Encoding UTF8
Write-Host 'BUN_ONLY_RUNTIME_RESULT_BEGIN'
$result | ConvertTo-Json -Depth 4
Write-Host 'BUN_ONLY_RUNTIME_RESULT_END'

# A failed compatibility probe is data, not a harness failure. The step succeeds
# if all stages executed and the JSON result was emitted.
exit 0

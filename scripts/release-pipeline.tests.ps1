#Requires -Version 5.1
<##
Focused, dependency-free checks for the pure release pipeline helpers.
Run with: powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\release-pipeline.tests.ps1
#>

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $scriptRoot 'release-pipeline-common.ps1')

function Assert-True {
    param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
    if (-not $Condition) { throw "Assertion failed: $Message" }
}

function Assert-Equal {
    param([Parameter(Mandatory)]$Actual, [Parameter(Mandatory)]$Expected, [Parameter(Mandatory)][string]$Message)
    if ($Actual -ne $Expected) { throw "Assertion failed: $Message (actual '$Actual', expected '$Expected')" }
}

function Assert-Throws {
    param([Parameter(Mandatory)][scriptblock]$Block, [Parameter(Mandatory)][string]$Message)
    $thrown = $false
    try { & $Block } catch { $thrown = $true }
    Assert-True $thrown $Message
}
Assert-Equal (Get-NodeMajor 'v24.18.0') 24 'Node 24 major parsing'
Assert-Equal (Assert-NodeMajor 'v24.18.0' 24) 24 'Node 24 requirement'
Assert-Throws { Assert-NodeMajor 'v23.11.0' 24 } 'non-24 Node major rejected by release prerequisite'

$prerequisiteText = Get-Content -Raw -LiteralPath (Join-Path $scriptRoot 'install-release-prerequisites.ps1')
Assert-True ($prerequisiteText -match '\$requiredNodeMajor\s*=\s*24') 'release prerequisite pins Node major 24'
Assert-True ($prerequisiteText -match '11\\\.25\\\.0') 'release prerequisite keeps pnpm 11.25.0 pinned'

Assert-Equal (Normalize-GitHubRepository 'https://github.com/nesszer/Win-CodexBar.git') 'nesszer/win-codexbar' 'HTTPS canonical URL'
Assert-Equal (Normalize-GitHubRepository 'git@github.com:nesszer/Win-CodexBar.git') 'nesszer/win-codexbar' 'SSH canonical URL'
Assert-True (Test-CanonicalReleaseTag 'v1.2.3') 'canonical release tag accepted'
Assert-True (-not (Test-CanonicalReleaseTag 'v1.2.3-rc.1')) 'prerelease tag rejected'
Assert-True (-not (Test-CanonicalReleaseTag 'v01.2.3')) 'leading-zero tag rejected'
Assert-Equal (Get-ReleaseVersionFromTag 'v0.48.0') '0.48.0' 'version extraction'
Assert-Throws { Get-ReleaseVersionFromTag 'v0.48.0+build' } 'invalid version extraction fails'

$assetNames = Get-RequiredReleaseAssets '0.48.0'
Assert-Equal $assetNames.Count 6 'exactly six release asset names including sidecars'
Assert-Equal $assetNames[0] 'CodexBar-0.48.0-Setup.exe' 'installer name'
Assert-Equal $assetNames[3] 'CodexBar-0.48.0-portable.exe.sha256' 'portable sidecar name'
Assert-Equal $assetNames[4] 'CodexBarCLI-v0.48.0-windows-x64.zip' 'CLI archive name'
Assert-Equal $assetNames[5] 'CodexBarCLI-v0.48.0-windows-x64.zip.sha256' 'CLI sidecar name'

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('win-codexbar-release-tests-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
try {
    $asset = Join-Path $testRoot 'CodexBar-0.48.0-Setup.exe'
    [IO.File]::WriteAllText($asset, 'deterministic fixture')
    $hash = Get-AssetSha256 $asset
    [IO.File]::WriteAllText("$asset.sha256", "$hash  $(Split-Path $asset -Leaf)`n")
    Assert-Equal (Get-SidecarSha256 $asset) $hash 'sidecar parser'
    Assert-AssetMatchesSidecar $asset
    [IO.File]::WriteAllText("$asset.sha256", ('0' * 64) + "  bad`n")
    Assert-Throws { Assert-AssetMatchesSidecar $asset } 'sidecar mismatch fails'
} finally {
    if ([IO.Directory]::Exists($testRoot)) { [IO.Directory]::Delete($testRoot, $true) }
}

$builderText = Get-Content -Raw -LiteralPath (Join-Path $scriptRoot 'windows-release-build.ps1')
$legacySwitch = 'Upload' + 'Release'
$clobberFlag = '--' + 'clobber'
Assert-True ($builderText -notmatch $legacySwitch) 'legacy upload parameter removed'
Assert-True ($builderText -notmatch $clobberFlag) 'builder has no clobber upload path'
$publisherText = Get-Content -Raw -LiteralPath (Join-Path $scriptRoot 'publish-github-release.ps1')
Assert-True ($publisherText -notmatch $clobberFlag) 'publisher has no clobber flag'

Write-Host 'Release pipeline focused tests passed.'

#Requires -Version 5.1
<##
.SYNOPSIS
    Validate release assets and emit the workspace manifest.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$AssetsDir,
    [Parameter(Mandatory)][string]$OutputDir,
    [Parameter(Mandatory)][string]$Tag,
    [Parameter(Mandatory)][string]$Sha,
    [string]$Repository = 'nesszer/Win-CodexBar'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-pipeline-common.ps1')

if (-not (Test-CanonicalReleaseTag $Tag)) {
    throw "Cannot emit a release manifest for non-canonical tag '$Tag'."
}
if ($Sha -notmatch '^[0-9a-fA-F]{40}$') {
    throw "Manifest requires a full immutable commit SHA; received '$Sha'."
}
if ((Normalize-GitHubRepository $Repository) -ne 'nesszer/win-codexbar') {
    throw "Manifest repository must be canonical nesszer/Win-CodexBar."
}
$version = Get-ReleaseVersionFromTag $Tag
if (-not (Test-Path -LiteralPath $AssetsDir -PathType Container)) {
    throw "Missing build assets directory: $AssetsDir"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$expectedPaths = Get-ExpectedReleaseAssetPaths $AssetsDir $version
foreach ($path in $expectedPaths) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing expected release asset: $path"
    }
}
Assert-AssetMatchesSidecar (Join-Path $AssetsDir "CodexBar-$version-Setup.exe")
Assert-AssetMatchesSidecar (Join-Path $AssetsDir "CodexBar-$version-portable.exe")
Assert-AssetMatchesSidecar (Join-Path $AssetsDir "CodexBarCLI-v$version-windows-x64.zip")

# Copy only the six publishable assets and the build logs into the persisted bundle.
foreach ($path in $expectedPaths) {
    Copy-Item -LiteralPath $path -Destination (Join-Path $OutputDir (Split-Path $path -Leaf)) -Force
}
Get-ChildItem -LiteralPath $AssetsDir -Filter '*.log' -File -ErrorAction SilentlyContinue |
    Copy-Item -Destination $OutputDir -Force

$assetRecords = @(
    foreach ($path in $expectedPaths) {
        [ordered]@{
            name = Split-Path $path -Leaf
            bytes = (Get-Item -LiteralPath $path).Length
            sha256 = Get-AssetSha256 $path
        }
    }
)
$manifest = [ordered]@{
    repository = 'nesszer/Win-CodexBar'
    tag = $Tag
    version = $version
    commit = $Sha.ToLowerInvariant()
    generated_at_utc = [DateTime]::UtcNow.ToString('o')
    assets = $assetRecords
}
$manifestPath = Join-Path $OutputDir 'release-manifest.json'
(ConvertTo-JsonString $manifest) | Set-Content -LiteralPath $manifestPath -Encoding UTF8
Write-Host "[ok] release manifest: $manifestPath"
Write-Host "[ok] persisted assets: $($expectedPaths.Count)"

#Requires -Version 5.1
<##
.SYNOPSIS
    Publish verified Windows assets to a draft GitHub Release without clobbering.

.DESCRIPTION
    This script is the only release publisher. It requires GH_TOKEN from the
    restricted CircleCI context, creates a draft release when needed, compares
    every existing asset by SHA-256, and uploads only missing assets. A mismatch
    or a non-draft release is a hard failure. The script never changes draft to
    published and is safe to rerun after a partial upload.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$AssetsDir,
    [Parameter(Mandatory)][string]$Tag,
    [string]$Repository = 'nesszer/Win-CodexBar'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'release-pipeline-common.ps1')

function Remove-TempFile {
    param([Parameter(Mandatory)][string]$Path)

    if ([IO.File]::Exists($Path)) { [IO.File]::Delete($Path) }
}

function Invoke-GhJson {
    param([Parameter(Mandatory)][string[]]$Arguments)

    $gh = Get-Command gh -ErrorAction Stop
    $stdoutPath = [IO.Path]::GetTempFileName()
    $stderrPath = [IO.Path]::GetTempFileName()
    try {
        & $gh.Source @Arguments 1>$stdoutPath 2>$stderrPath
        $exitCode = $LASTEXITCODE
        if ($exitCode -ne 0) {
            $errorText = Get-Content -Raw -LiteralPath $stderrPath -ErrorAction SilentlyContinue
            throw "gh $($Arguments -join ' ') failed with exit code $($exitCode): $errorText"
        }
        return (Get-Content -Raw -LiteralPath $stdoutPath | ConvertFrom-Json)
    } finally {
        Remove-TempFile $stdoutPath
        Remove-TempFile $stderrPath
    }
}

function Invoke-GhUpload {
    param([Parameter(Mandatory)][string]$Path)

    $gh = Get-Command gh -ErrorAction Stop
    & $gh.Source release upload $Tag $Path '--repo' $Repository
    if ($LASTEXITCODE -ne 0) {
        throw "gh release upload failed for $(Split-Path $Path -Leaf) with exit code $LASTEXITCODE"
    }
}

function Get-Release {
    $gh = Get-Command gh -ErrorAction Stop
    $stdoutPath = [IO.Path]::GetTempFileName()
    $stderrPath = [IO.Path]::GetTempFileName()
    try {
        & $gh.Source api "repos/$Repository/releases/tags/$Tag" 1>$stdoutPath 2>$stderrPath
        if ($LASTEXITCODE -eq 0) {
            return Get-Content -Raw -LiteralPath $stdoutPath | ConvertFrom-Json
        }
        $errorText = Get-Content -Raw -LiteralPath $stderrPath -ErrorAction SilentlyContinue
        if ($errorText -match 'Not Found|404') {
            return $null
        }
        throw "Could not query GitHub Release for $($Tag): $errorText"
    } finally {
        Remove-TempFile $stdoutPath
        Remove-TempFile $stderrPath
    }
}

function New-DraftRelease {
    return Invoke-GhJson @(
        'api', '--method', 'POST', "repos/$Repository/releases",
        '--field', "tag_name=$Tag",
        '--field', "target_commitish=$env:RELEASE_SHA",
        '--field', "name=$Tag",
        '--field', 'draft=true',
        '--field', 'prerelease=false',
        '--field', 'generate_release_notes=false'
    )
}

function Get-RemoteAssetSha256 {
    param([Parameter(Mandatory)]$Asset)

    $digest = ''
    $digestProperty = $Asset.PSObject.Properties['digest']
    if ($digestProperty) { $digest = [string]$digestProperty.Value }
    if ($digest -match '^sha256:([0-9a-fA-F]{64})$') {
        return $Matches[1].ToLowerInvariant()
    }

    $downloadPath = [IO.Path]::GetTempFileName()
    try {
        $gh = Get-Command gh -ErrorAction Stop
        & $gh.Source api '--header' 'Accept: application/octet-stream' "repos/$Repository/releases/assets/$($Asset.id)" '--output' $downloadPath
        if ($LASTEXITCODE -ne 0) {
            throw "Could not download existing GitHub asset '$($Asset.name)' to verify its hash."
        }
        return Get-AssetSha256 $downloadPath
    } finally {
        Remove-TempFile $downloadPath
    }
}
function Assert-ManifestAndAssets {
    param(
        [Parameter(Mandatory)][string]$ManifestPath,
        [Parameter(Mandatory)][string]$ExpectedVersion,
        [Parameter(Mandatory)][string]$ExpectedTag
    )

    if (-not (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) {
        throw "Missing release manifest: $ManifestPath"
    }
    $manifest = Get-Content -Raw -LiteralPath $ManifestPath | ConvertFrom-Json
    if (-not $manifest.PSObject.Properties['repository'] -or [string]$manifest.repository -ne 'nesszer/Win-CodexBar') {
        throw 'Manifest repository is not canonical nesszer/Win-CodexBar.'
    }
    if (-not $manifest.PSObject.Properties['tag'] -or [string]$manifest.tag -ne $ExpectedTag) {
        throw "Manifest tag does not match $ExpectedTag."
    }
    if (-not $manifest.PSObject.Properties['version'] -or [string]$manifest.version -ne $ExpectedVersion) {
        throw "Manifest version does not match $ExpectedVersion."
    }
    if (-not $env:CIRCLE_SHA1 -or $env:CIRCLE_SHA1 -notmatch '^[0-9a-fA-F]{40}$') {
        throw 'CIRCLE_SHA1 must be a full commit SHA in the publisher job.'
    }
    $expectedCommit = ($env:CIRCLE_SHA1).ToLowerInvariant()
    if (-not $manifest.PSObject.Properties['commit'] -or [string]$manifest.commit -cne $expectedCommit) {
        throw "Manifest commit must equal lowercase CIRCLE_SHA1 ($expectedCommit)."
    }
    if (-not $env:RELEASE_SHA -or $env:RELEASE_SHA -notmatch '^[0-9a-fA-F]{40}$' -or ($env:RELEASE_SHA).ToLowerInvariant() -ne $expectedCommit) {
        throw 'RELEASE_SHA must equal lowercase CIRCLE_SHA1 before publication.'
    }

    $expectedNames = @(Get-RequiredReleaseAssets $ExpectedVersion | Sort-Object)
    if (-not $manifest.PSObject.Properties['assets']) {
        throw 'Manifest is missing its assets list.'
    }
    $manifestAssets = @($manifest.assets)
    if ($manifestAssets.Count -ne 4) {
        throw "Manifest must contain exactly four release assets; found $($manifestAssets.Count)."
    }
    $manifestNames = @($manifestAssets | ForEach-Object { [string]$_.name } | Sort-Object)
    if (($manifestNames -join '|') -ne ($expectedNames -join '|')) {
        throw "Manifest assets must be exactly: $($expectedNames -join ', ')."
    }

    $publishFiles = @(Get-ChildItem -LiteralPath $AssetsDir -File | Where-Object {
        $_.Name -ne 'release-manifest.json' -and $_.Name -notmatch '\.log$'
    } | Select-Object -ExpandProperty Name | Sort-Object)
    if (($publishFiles -join '|') -ne ($expectedNames -join '|')) {
        throw "Persisted bundle contains unexpected publishable files: $($publishFiles -join ', ')."
    }

    foreach ($name in $expectedNames) {
        $path = Join-Path $AssetsDir $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Missing expected release asset: $path"
        }
        $actualHash = Get-AssetSha256 $path
        $record = @($manifestAssets | Where-Object { [string]$_.name -eq $name }) | Select-Object -First 1
        if (-not $record -or [string]$record.sha256 -cne $actualHash) {
            throw "Manifest SHA-256 mismatch for $name."
        }
        if ([int64]$record.bytes -ne (Get-Item -LiteralPath $path).Length) {
            throw "Manifest byte count mismatch for $name."
        }
    }
    Assert-AssetMatchesSidecar (Join-Path $AssetsDir "CodexBar-$ExpectedVersion-Setup.exe")
    Assert-AssetMatchesSidecar (Join-Path $AssetsDir "CodexBar-$ExpectedVersion-portable.exe")
    Write-Host '[ok] manifest, exact four asset names, SHA-256 values, and sidecars verified before API access'
}

$env:GH_TOKEN = if ($env:GH_TOKEN) { $env:GH_TOKEN } elseif ($env:gh_token) { $env:gh_token } else { '' }
if ([string]::IsNullOrWhiteSpace($env:GH_TOKEN)) {
    throw 'GH_TOKEN is required and must be provided only by the restricted CircleCI publisher context.'
}
if (-not (Test-CanonicalReleaseTag $Tag)) {
    throw "Publisher accepts only canonical vX.Y.Z tags; received '$Tag'."
}
if ((Normalize-GitHubRepository $Repository) -ne 'nesszer/win-codexbar') {
    throw "Publisher repository must be canonical nesszer/Win-CodexBar."
}
if (-not (Test-Path -LiteralPath $AssetsDir -PathType Container)) {
    throw "Missing persisted release assets directory: $AssetsDir"
}
$version = Get-ReleaseVersionFromTag $Tag
$assetPaths = Get-ExpectedReleaseAssetPaths $AssetsDir $version
Assert-ManifestAndAssets (Join-Path $AssetsDir 'release-manifest.json') $version $Tag

$release = Get-Release
if ($null -eq $release) {
    if (-not $env:RELEASE_SHA -or $env:RELEASE_SHA -notmatch '^[0-9a-fA-F]{40}$') {
        throw 'RELEASE_SHA must be a full commit SHA when creating a draft release.'
    }
    Write-Host "Creating draft GitHub Release $Tag"
    $release = New-DraftRelease
}
if (-not [bool]$release.draft) {
    throw "GitHub Release $Tag already exists and is not a draft; refusing to modify a final release."
}
if ([string]$release.tag_name -ne $Tag) {
    throw "GitHub returned release tag '$($release.tag_name)' while publishing '$Tag'."
}

foreach ($path in $assetPaths) {
    $name = Split-Path $path -Leaf
    $localHash = Get-AssetSha256 $path
    $existing = @($release.assets | Where-Object { $_.name -eq $name }) | Select-Object -First 1
    if ($existing) {
        $remoteHash = Get-RemoteAssetSha256 $existing
        if ($remoteHash -ne $localHash) {
            throw "Existing GitHub asset '$name' has SHA-256 $remoteHash, expected $localHash."
        }
        Write-Host "[skip] $name already matches SHA-256 $localHash"
        continue
    }

    Write-Host "[upload] $name ($localHash)"
    try {
        Invoke-GhUpload $path
    } catch {
        # A retry may race a successful upload. Re-read and accept only an exact match.
        $latest = Get-Release
        $raced = if ($latest) { @($latest.assets | Where-Object { $_.name -eq $name }) | Select-Object -First 1 } else { $null }
        if ($raced) {
            $remoteHash = Get-RemoteAssetSha256 $raced
            if ($remoteHash -eq $localHash) {
                Write-Host "[skip] $name was uploaded by a concurrent retry"
                $release = $latest
                continue
            }
        }
        throw
    }
    $release = Get-Release
    if (-not $release -or -not [bool]$release.draft) {
        throw "Release $Tag disappeared or is no longer draft after uploading $name."
    }
}

Write-Host "Draft GitHub Release $Tag contains all four verified Windows assets."
Write-Host 'No release publication/finalization was performed.'

#Requires -Version 5.1
<##
Shared, side-effect-free helpers for the hosted Windows release pipeline.
This file is dot-sourced by the preflight, manifest, publisher, and focused tests.
#>

Set-StrictMode -Version Latest

function Get-NodeMajor {
    param([Parameter(Mandatory)][string]$Version)

    if ($Version -notmatch '^v(\d+)\.') {
        throw "Could not parse Node version '$Version'."
    }
    return [int]$Matches[1]
}

function Assert-NodeMajor {
    param(
        [Parameter(Mandatory)][string]$Version,
        [Parameter(Mandatory)][int]$ExpectedMajor
    )

    $actualMajor = Get-NodeMajor $Version
    if ($actualMajor -ne $ExpectedMajor) {
        throw "Node $ExpectedMajor.x is required; found $Version."
    }
    return $actualMajor
}

function Normalize-GitHubRepository {
    param([Parameter(Mandatory)][string]$Url)

    $value = $Url.Trim()
    $value = $value -replace '^git@github\.com:', ''
    $value = $value -replace '^ssh://git@github\.com/', ''
    $value = $value -replace '^https?://github\.com/', ''
    $value = $value.TrimEnd('/')
    $value = $value -replace '\.git$', ''
    return $value.ToLowerInvariant()
}

function Test-CanonicalReleaseTag {
    param([AllowNull()][string]$Tag)

    return -not [string]::IsNullOrWhiteSpace($Tag) -and $Tag -match '^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$'
}

function Get-ReleaseVersionFromTag {
    param([Parameter(Mandatory)][string]$Tag)

    if (-not (Test-CanonicalReleaseTag $Tag)) {
        throw "Tag '$Tag' is not a canonical vX.Y.Z release tag."
    }
    return $Tag.Substring(1)
}

function Get-RequiredReleaseAssets {
    param([Parameter(Mandatory)][string]$Version)

    return @(
        "CodexBar-$Version-Setup.exe",
        "CodexBar-$Version-Setup.exe.sha256",
        "CodexBar-$Version-portable.exe",
        "CodexBar-$Version-portable.exe.sha256"
    )
}

function Get-AssetSha256 {
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing release asset: $Path"
    }
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-SidecarSha256 {
    param([Parameter(Mandatory)][string]$AssetPath)

    $sidecarPath = "$AssetPath.sha256"
    if (-not (Test-Path -LiteralPath $sidecarPath -PathType Leaf)) {
        throw "Missing SHA-256 sidecar: $sidecarPath"
    }
    $line = Get-Content -LiteralPath $sidecarPath | Select-Object -First 1
    if (-not $line -or $line -notmatch '^([0-9a-fA-F]{64})\s+') {
        throw "Invalid SHA-256 sidecar: $sidecarPath"
    }
    return $Matches[1].ToLowerInvariant()
}

function Assert-AssetMatchesSidecar {
    param([Parameter(Mandatory)][string]$AssetPath)

    $expected = Get-SidecarSha256 $AssetPath
    $actual = Get-AssetSha256 $AssetPath
    if ($actual -ne $expected) {
        throw "SHA-256 sidecar mismatch for $(Split-Path $AssetPath -Leaf): expected $expected, got $actual"
    }
}

function Get-ExpectedReleaseAssetPaths {
    param([Parameter(Mandatory)][string]$AssetsDir, [Parameter(Mandatory)][string]$Version)

    return @(
        Get-RequiredReleaseAssets $Version | ForEach-Object { Join-Path $AssetsDir $_ }
    )
}

function ConvertTo-JsonString {
    param([Parameter(Mandatory)]$Value)

    return ($Value | ConvertTo-Json -Depth 8)
}

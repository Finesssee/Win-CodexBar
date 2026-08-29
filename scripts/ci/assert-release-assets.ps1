#Requires -Version 5.1

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$versionLine = Get-Content rust\Cargo.toml | Where-Object { $_ -match '^version = "([^"]+)"' } | Select-Object -First 1
if ($versionLine -notmatch '^version = "([^"]+)"') {
    Write-Host "Failed to determine version from rust\Cargo.toml"
    [Environment]::Exit(1)
}

$version = $Matches[1]
$assetsDir = "C:\code\Win-CodexBar-release\assets"

foreach ($name in @(
    "CodexBar-$version-Setup.exe",
    "CodexBar-$version-Setup.exe.sha256",
    "CodexBar-$version-portable.exe",
    "CodexBar-$version-portable.exe.sha256",
    "CodexBarCLI-v$version-windows-x64.zip",
    "CodexBarCLI-v$version-windows-x64.zip.sha256"
)) {
    $path = Join-Path $assetsDir $name
    if (-not (Test-Path -LiteralPath $path)) {
        Write-Host "Missing release artifact: $path"
        [Environment]::Exit(1)
    }
    Write-Host "Found $path"
}

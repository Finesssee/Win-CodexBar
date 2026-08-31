#Requires -Version 5.1
<##
Pure, side-effect-free helpers shared by the CircleCI pr-check scripts.
Dot-source only (from scripts\circleci-pr-gates.ps1,
scripts\run-circleci-pr-check.ps1, and scripts\circleci-pr.tests.ps1);
this file must never execute work on its own.
#>

Set-StrictMode -Version Latest

<#
.SYNOPSIS
    Docs-only diff test mirroring the GitHub workflow's paths-ignore set
    (docs/**, **/*.md, CONTEXT.md, .github/CI.md). CONTEXT.md and
    .github/CI.md end in .md, so the docs/ prefix plus .md suffix covers the
    exact ignore set.
#>
function Test-DocsOnlyDiff {
    param([AllowEmptyCollection()][string[]]$ChangedFiles)

    $codeFiles = @(
        $ChangedFiles | Where-Object { $_ -and ($_ -notmatch '^(docs/.*|.*\.md)$') }
    )
    return ($codeFiles.Count -eq 0)
}

<#
.SYNOPSIS
    Gates 1 and 2 of the hosted pr-check: budget emergency stop and
    branch/PR scope. Gate 3 (docs-only) needs a diff and lives in
    scripts\circleci-pr-gates.ps1; main/master pushes never reach it.
#>
function Get-TriggerGateDecision {
    param(
        [AllowEmptyString()][string]$BudgetMode,
        [AllowEmptyString()][string]$Branch,
        [AllowEmptyString()][string]$PrUrl
    )

    # Gate 1 - budget: CI_BUDGET_MODE=off is the emergency stop
    # (unset/empty = normal).
    if ($BudgetMode -eq 'off') {
        return [pscustomobject]@{
            Skip = $true
            Reason = 'CI_BUDGET_MODE is off: hosted pr-check skips (emergency stop).'
        }
    }

    # Gate 2 - scope: PR pipelines and main/master pushes run the checks;
    # every other branch push skips. CircleCI delivers same-repo PR builds
    # as branch pipelines, so PR association comes from the compile-time
    # GitHub App pipeline value pipeline.event.context.github.pr_url.
    $isPr = -not [string]::IsNullOrWhiteSpace($PrUrl)
    $isMainPush = $Branch -in @('main', 'master')
    if (-not $isPr -and -not $isMainPush) {
        return [pscustomobject]@{
            Skip = $true
            Reason = "Branch push to '$Branch' (not a PR, not main/master): hosted pr-check skips."
        }
    }

    return [pscustomobject]@{
        Skip = $false
        Reason = "Trigger gate passed (PR: $isPr, branch: '$Branch')."
    }
}

<#
.SYNOPSIS
    Extract a SHA-256 digest for FileName from official checksum text.
    Accepts either a bare 64-hex digest (rustup-init.exe.sha256 style) or a
    SHASUMS256.txt-style list of "<digest>  <filename>" lines (an optional
    GNU binary-mode '*' before the filename is tolerated). Throws instead of
    guessing on anything empty, malformed, or missing.
#>
function Get-ExpectedSha256 {
    param(
        [Parameter(Mandatory)][string]$ChecksumText,
        [Parameter(Mandatory)][string]$FileName
    )

    $lines = @(
        $ChecksumText -split "`r?`n" |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_ }
    )
    if ($lines.Count -eq 0) { throw 'Checksum source is empty.' }

    if ($lines.Count -eq 1 -and $lines[0] -match '^[0-9a-fA-F]{64}$') {
        return $lines[0].ToLowerInvariant()
    }

    foreach ($line in $lines) {
        if ($line -notmatch '^([0-9a-fA-F]{64})\s+\*?(.+)$') { continue }
        if ($Matches[2].Trim() -eq $FileName) {
            return $Matches[1].ToLowerInvariant()
        }
    }
    throw "No SHA-256 entry for '$FileName' in the checksum source."
}

<#
.SYNOPSIS
    Compare a downloaded file's SHA-256 with the expected digest and throw
    before the file is executed on mismatch.
#>
function Assert-FileSha256 {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$ExpectedSha256
    )

    if ($ExpectedSha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw "Expected checksum '$ExpectedSha256' is not a 64-hex SHA-256 digest."
    }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($Path)
        try {
            $hashBytes = $sha.ComputeHash($stream)
        } finally {
            $stream.Dispose()
        }
    } finally {
        $sha.Dispose()
    }
    $actual = ([System.BitConverter]::ToString($hashBytes) -replace '-', '').ToLowerInvariant()
    if ($actual -ne $ExpectedSha256.ToLowerInvariant()) {
        throw "SHA-256 mismatch for '$Path': expected $ExpectedSha256, got $actual."
    }
    return $actual
}

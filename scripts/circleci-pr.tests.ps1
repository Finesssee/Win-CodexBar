#Requires -Version 5.1
<##
Focused, dependency-free checks for the CircleCI pr-check helpers: checksum
parsing, gate decisions, script parsing, and YAML validation. Runs entirely
without CircleCI or network access.

Run with: powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\circleci-pr.tests.ps1
#>

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptRoot
. (Join-Path $scriptRoot 'circleci-pr-common.ps1')

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

Write-Host '==> Checksum parsing (Get-ExpectedSha256)'
$goodDigest = 'd8f2a0d5d8ba5d4d5d7c6e5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c'
$shasums = "a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4  node-v24.18.0-x64.msi`n$goodDigest  rustup-init.exe`n"
Assert-Equal (Get-ExpectedSha256 -ChecksumText $shasums -FileName 'rustup-init.exe') $goodDigest 'SHASUMS256 entry for rustup-init'
Assert-Throws { Get-ExpectedSha256 -ChecksumText $shasums -FileName 'node-v99.0.0-x64.msi' } 'missing SHASUMS256 entry throws'
$binary = "$goodDigest *node-v24.18.0-x64.msi`n"
Assert-Equal (Get-ExpectedSha256 -ChecksumText $binary -FileName 'node-v24.18.0-x64.msi') $goodDigest 'binary-mode SHASUMS256 entry'
$crlf = "$goodDigest  node-v24.18.0-x64.msi`r`n"
Assert-Equal (Get-ExpectedSha256 -ChecksumText $crlf -FileName 'node-v24.18.0-x64.msi') $goodDigest 'CRLF SHASUMS256 entry'
Assert-Equal (Get-ExpectedSha256 -ChecksumText "$goodDigest`n" -FileName 'rustup-init.exe') $goodDigest 'bare adjacent .sha256 digest'
Assert-Equal (Get-ExpectedSha256 -ChecksumText $goodDigest -FileName 'ignored') $goodDigest 'bare digest ignores filename'
Assert-Throws { Get-ExpectedSha256 -ChecksumText '' -FileName 'x' } 'empty checksum text throws'
Assert-Throws { Get-ExpectedSha256 -ChecksumText 'not-a-digest' -FileName 'x' } 'malformed checksum text throws'
Assert-Throws { Get-ExpectedSha256 -ChecksumText 'abc123  node.msi' -FileName 'node.msi' } 'short digest entry throws'

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('win-codexbar-circleci-tests-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
$fixture = Join-Path $testRoot 'fixture.bin'
[IO.File]::WriteAllText($fixture, 'deterministic fixture')
$shaForTest = [System.Security.Cryptography.SHA256]::Create()
try {
    $streamForTest = [System.IO.File]::OpenRead($fixture)
    try {
        $hashBytesForTest = $shaForTest.ComputeHash($streamForTest)
    } finally {
        $streamForTest.Dispose()
    }
} finally {
    $shaForTest.Dispose()
}
$actualSha = ([System.BitConverter]::ToString($hashBytesForTest) -replace '-', '').ToLowerInvariant()
Assert-Equal (Assert-FileSha256 -Path $fixture -ExpectedSha256 $actualSha) $actualSha 'matching digest passes'
Assert-Throws { Assert-FileSha256 -Path $fixture -ExpectedSha256 $goodDigest } 'mismatching digest throws'
Assert-Throws { Assert-FileSha256 -Path $fixture -ExpectedSha256 'tooshort' } 'non-64-hex expected digest throws'

Write-Host '==> Docs-only diff test (Test-DocsOnlyDiff)'
Assert-True (Test-DocsOnlyDiff -ChangedFiles @('docs/release/ci-cd.md', 'CONTEXT.md', 'notes.md')) 'docs-only diff detected'
Assert-True (-not (Test-DocsOnlyDiff -ChangedFiles @('docs/x.md', 'Cargo.toml'))) 'code file breaks docs-only'
Assert-True (Test-DocsOnlyDiff -ChangedFiles @()) 'empty diff treated as docs-only'

Write-Host '==> Trigger trigger gate decisions (Get-TriggerGateDecision)'
$decision = Get-TriggerGateDecision -BudgetMode 'off' -Branch 'feature/x' -PrUrl 'https://github.com/nesszer/Win-CodexBar/pull/405'
Assert-True $decision.Skip 'budget off skips'
Assert-True ($decision.Reason -match 'emergency') 'budget off reason'
$decision = Get-TriggerGateDecision -BudgetMode '' -Branch 'main' -PrUrl ''
Assert-True (-not $decision.Skip) 'main push runs (budget normal)'
$decision = Get-TriggerGateDecision -BudgetMode 'normal' -Branch 'master' -PrUrl ''
Assert-True (-not $decision.Skip) 'master push runs'
$decision = Get-TriggerGateDecision -BudgetMode 'normal' -Branch 'main' -PrUrl 'https://github.com/nesszer/Win-CodexBar/pull/405'
Assert-True (-not $decision.Skip) 'PR on main runs'
$decision = Get-TriggerGateDecision -BudgetMode 'normal' -Branch 'codex/topic' -PrUrl ''
Assert-True $decision.Skip 'non-PR topic branch skips'
Assert-True ($decision.Reason -match 'codex/topic') 'topic branch skip reason'
$decision = Get-TriggerGateDecision -BudgetMode 'off' -Branch 'main' -PrUrl ''
Assert-True $decision.Skip 'budget off wins over main push'

Write-Host 'CircleCI focused tests passed.'
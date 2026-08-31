#Requires -Version 5.1
<##
.SYNOPSIS
    Hosted pr-check trigger gates for the CircleCI Windows executor.

.DESCRIPTION
    Owns the three skip decisions that used to live inline in
    .circleci/config.yml:

    1. Budget gate    - CI_BUDGET_MODE=off is the emergency stop
                        (unset/empty = normal).
    2. Scope gate     - PR pipelines and main/master pushes run the checks;
                        any other branch push skips.
    3. Docs-only gate - PRs whose diff touches only docs/**, **/*.md,
                        CONTEXT.md, and .github/CI.md skip.

    Each true skip calls `circleci-agent step halt`. main/master pushes never
    reach the docs-only gate, so they can never be skipped by a multi-commit
    docs-only diff. Unknown bases fail open (the checks run).

    Pure decision logic lives in scripts\circleci-pr-common.ps1 and is
    exercised by scripts\circleci-pr.tests.ps1 without CircleCI.
#>
[CmdletBinding()]
param(
    # CI_BUDGET_MODE project variable value; unset/empty = normal.
    [AllowEmptyString()][string]$BudgetMode = $env:CI_BUDGET_MODE,

    # CIRCLE_BRANCH.
    [AllowEmptyString()][string]$Branch = $env:CIRCLE_BRANCH,

    # Compile-time pipeline value pipeline.event.context.github.pr_url,
    # exported as CBX_PR_URL by the job environment (empty on push
    # pipelines). Delivered via environment, never as a command-line
    # argument: an empty-string argument does not survive the Windows argv
    # handoff to a child powershell.exe -File invocation (MissingArgument).
    [AllowEmptyString()][string]$PrUrl = $env:CBX_PR_URL,

    # Compile-time pipeline value
    # pipeline.event.github.pull_request.base.sha, exported as CBX_PR_BASE_SHA
    # by the job environment (populated only on pull request events; empty on
    # push pipelines).
    [AllowEmptyString()][string]$PrBaseSha = $env:CBX_PR_BASE_SHA,

    # CIRCLE_SHA1.
    [AllowEmptyString()][string]$Sha = $env:CIRCLE_SHA1,

    # Print the decision and exit without calling circleci-agent, for
    # local proof of the gate logic.
    [switch]$PlanOnly,

    # Repository root; defaults to the checkout this script lives in.
    [AllowEmptyString()][string]$RepoRoot = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = Split-Path -Parent $PSScriptRoot
}

. (Join-Path $PSScriptRoot 'circleci-pr-common.ps1')

function Invoke-GateHalt {
    param([Parameter(Mandatory)][string]$Reason)

    Write-Host $Reason
    if ($PlanOnly) { return }
    & circleci-agent step halt
    if ($LASTEXITCODE -ne 0) {
        throw "circleci-agent step halt exited with code $LASTEXITCODE"
    }
}

# Gates 1 and 2: budget emergency stop, then PR/main-master scope.
$trigger = Get-TriggerGateDecision -BudgetMode $BudgetMode -Branch $Branch -PrUrl $PrUrl
if ($trigger.Skip) {
    Invoke-GateHalt -Reason $trigger.Reason
    exit 0
}
Write-Host "CI_BUDGET_MODE is '$BudgetMode': hosted pr-check passes budget gate."
Write-Host $trigger.Reason

# Gate 3 - docs-only: mirror paths-ignore (docs/**, **/*.md, CONTEXT.md,
# .github/CI.md). Applies to PR pipelines only; main/master pushes always
# run the checks, so a multi-commit docs-only history can never suppress
# them. When the base cannot be determined the gate fails open.
$isMainPush = $Branch -in @('main', 'master')
$base = $PrBaseSha
if ($isMainPush) {
    Write-Host "Push to '$Branch' runs the full checks (docs-only skip never applies to main/master)."
} elseif ([string]::IsNullOrWhiteSpace($base)) {
    # PR association without a populated event value (e.g. api trigger):
    # resolve the base from the public GitHub pulls API.
    try {
        $prNumber = ''
        if ($PrUrl -match '/pull/(\d+)') { $prNumber = $Matches[1] }
        if ([string]::IsNullOrWhiteSpace($prNumber)) { throw 'PR number not available from pipeline values.' }
        if ([string]::IsNullOrWhiteSpace($env:CIRCLE_PROJECT_USERNAME) -or [string]::IsNullOrWhiteSpace($env:CIRCLE_PROJECT_REPONAME)) {
            throw 'CIRCLE_PROJECT_USERNAME/REPONAME unavailable for pulls API fallback.'
        }
        $repo = "$($env:CIRCLE_PROJECT_USERNAME)/$($env:CIRCLE_PROJECT_REPONAME)"
        $pr = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/pulls/$prNumber" -Headers @{ 'User-Agent' = 'codexbar-pr-check' }
        $baseSha = [string]$pr.base.sha
        if ([string]::IsNullOrWhiteSpace($baseSha)) { throw 'pulls API returned no base.sha.' }
        & git fetch origin $baseSha --depth=1
        if ($LASTEXITCODE -ne 0) { throw "git fetch base.sha exited with code $LASTEXITCODE" }
        $base = $baseSha
        Write-Host "Docs gate base resolved from GitHub pulls API: $baseSha"
    } catch {
        Write-Host "Docs gate base resolution failed ($($_.Exception.Message)): running full checks (fail open)."
        $base = ''
    }
}

if ($isMainPush -or -not [string]::IsNullOrWhiteSpace($base)) {
    if (-not $isMainPush) {
        # Two-dot tree diff: the base fetched at depth 1 may lack a merge
        # base for a three-dot diff.
        $changed = & git -C $RepoRoot diff --name-only "$base..$Sha"
        if ($LASTEXITCODE -ne 0) {
            Write-Host 'git diff against base failed: cannot evaluate docs-only gate; running full checks (fail open).'
            $changed = $null
        }
        if ($null -ne $changed) {
            $changed = @($changed)
            if (Test-DocsOnlyDiff -ChangedFiles $changed) {
                Invoke-GateHalt -Reason "All $($changed.Count) changed file(s) match paths-ignore (docs/**, **/*.md, CONTEXT.md, .github/CI.md): hosted pr-check skips."
                exit 0
            }
            Write-Host "$($changed.Count) changed file(s), at least one outside paths-ignore: hosted pr-check passes docs gate."
        }
    }
}

exit 0

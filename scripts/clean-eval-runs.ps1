[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
param(
    [ValidateRange(1, 8760)]
    [int]$IncompleteMinimumAgeHours = 24
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$evalRootCandidate = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'target\evals'))
if (-not (Test-Path -LiteralPath $evalRootCandidate -PathType Container)) {
    Write-Host 'No target/evals directory exists. Nothing to clean.'
    return
}
$evalRoot = (Resolve-Path -LiteralPath $evalRootCandidate).Path
$evalRootItem = Get-Item -LiteralPath $evalRoot -Force
if (($evalRootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Refusing to clean a reparse-point eval root: $evalRoot"
}

function Get-SafeRunInventory {
    param([Parameter(Mandatory)][string]$RunPath)

    $bytes = 0L
    $latestWriteUtc = [System.IO.DirectoryInfo]::new($RunPath).LastWriteTimeUtc
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $pending.Push($RunPath)
    while ($pending.Count -gt 0) {
        $directoryPath = $pending.Pop()
        $directory = Get-Item -LiteralPath $directoryPath -Force
        if (($directory.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Refusing to clean a run containing a reparse point: $($directory.FullName)"
        }
        if ($directory.LastWriteTimeUtc -gt $latestWriteUtc) {
            $latestWriteUtc = $directory.LastWriteTimeUtc
        }
        foreach ($child in Get-ChildItem -LiteralPath $directory.FullName -Force) {
            if (($child.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Refusing to clean a run containing a reparse point: $($child.FullName)"
            }
            if ($child.LastWriteTimeUtc -gt $latestWriteUtc) {
                $latestWriteUtc = $child.LastWriteTimeUtc
            }
            if ($child.PSIsContainer) {
                $pending.Push($child.FullName)
            } else {
                $bytes += $child.Length
            }
        }
    }

    [pscustomobject]@{
        Bytes = $bytes
        LatestWriteUtc = $latestWriteUtc
    }
}

function Get-HumanReviewDisposition {
    param([Parameter(Mandatory)][string]$RunPath)

    $disposition = [pscustomobject]@{
        Reviewed = $false
        Pending = $false
        Unreadable = $false
    }

    foreach ($fileName in @('human-review.json', 'human-score.json')) {
        $path = Join-Path $RunPath $fileName
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            continue
        }
        try {
            $review = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
            if ($fileName -eq 'human-score.json') {
                $reviewed = $review.PSObject.Properties['tasks_reviewed']
                $pending = $review.PSObject.Properties['tasks_pending']
                if ($null -eq $reviewed -and $null -eq $pending) {
                    $disposition.Unreadable = $true
                    continue
                }
                $reviewedCount = if ($null -eq $reviewed) { 0 } else { [int]$reviewed.Value }
                $pendingCount = if ($null -eq $pending) { 0 } else { [int]$pending.Value }
                $disposition.Reviewed = $disposition.Reviewed -or ($reviewedCount -gt 0)
                $disposition.Pending = $disposition.Pending -or ($pendingCount -gt 0)
                continue
            }

            $tasksProperty = $review.PSObject.Properties['tasks']
            if ($null -eq $tasksProperty) {
                $disposition.Unreadable = $true
                continue
            }
            foreach ($task in @($tasksProperty.Value)) {
                $acceptedProperty = $task.PSObject.Properties['accepted']
                if ($null -eq $acceptedProperty -or $null -eq $acceptedProperty.Value) {
                    $disposition.Pending = $true
                } else {
                    $disposition.Reviewed = $true
                }
            }
        } catch {
            $disposition.Unreadable = $true
        }
    }

    return $disposition
}

$removedRuns = 0
$preservedRuns = 0
$recoveredBytes = 0L

foreach ($run in Get-ChildItem -LiteralPath $evalRoot -Directory) {
    if (-not $run.Name.StartsWith('kinewright-eval-', [System.StringComparison]::Ordinal)) {
        Write-Host "Preserving unrecognized eval directory: $($run.Name)"
        $preservedRuns += 1
        continue
    }

    $resolvedRun = (Resolve-Path -LiteralPath $run.FullName).Path
    $parent = [System.IO.Path]::GetDirectoryName($resolvedRun)
    if (-not $parent.Equals($evalRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to clean a path outside the eval root: $resolvedRun"
    }
    $inventory = Get-SafeRunInventory -RunPath $resolvedRun
    $humanReview = Get-HumanReviewDisposition -RunPath $resolvedRun
    if ($humanReview.Unreadable) {
        Write-Warning "Preserving eval run with an unreadable human-review artifact: $resolvedRun"
        $preservedRuns += 1
        continue
    }
    if ($humanReview.Reviewed) {
        Write-Host "Preserving human-reviewed eval run: $($run.Name)"
        $preservedRuns += 1
        continue
    }

    $machinePassed = $false
    $reportComplete = $false
    $reportPath = Join-Path $resolvedRun 'machine-report.json'
    if (Test-Path -LiteralPath $reportPath -PathType Leaf) {
        try {
            $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
            $machinePassed = $report.machine_passed -eq $true
            $reportComplete =
                $null -ne $report.PSObject.Properties['schema_version'] -and
                $null -ne $report.PSObject.Properties['run_id'] -and
                $report.run_id -eq $run.Name -and
                $null -ne $report.PSObject.Properties['machine_passed']
        } catch {
            Write-Warning "Machine report is unreadable; treating the run as incomplete and applying the age cutoff: $reportPath"
        }
    }

    if ($machinePassed) {
        $preservedRuns += 1
        continue
    }

    if (-not $reportComplete -or $humanReview.Pending) {
        $minimumAge = [TimeSpan]::FromHours($IncompleteMinimumAgeHours)
        $age = [DateTime]::UtcNow - $inventory.LatestWriteUtc
        if ($age -lt $minimumAge) {
            $activeMessage = "Preserving active or incomplete eval run {0}; newest content is {1:N1} hours old and the safety cutoff is {2} hours." -f $run.Name, $age.TotalHours, $IncompleteMinimumAgeHours
            Write-Host $activeMessage
            $preservedRuns += 1
            continue
        }
    }
    $runBytes = $inventory.Bytes

    if ($PSCmdlet.ShouldProcess(
        $resolvedRun,
        "Remove failed, partial, or rerender-only eval run ($([math]::Round($runBytes / 1MB, 1)) MiB)"
    )) {
        Remove-Item -LiteralPath $resolvedRun -Recurse -Force
        $removedRuns += 1
        $recoveredBytes += $runBytes
    }
}

Write-Host "Preserved $preservedRuns eval runs (passing, active, or unrecognized)."
Write-Host "Removed $removedRuns failed, partial, or rerender-only eval runs."
Write-Host ("Recovered approximately {0:N1} MiB." -f ($recoveredBytes / 1MB))

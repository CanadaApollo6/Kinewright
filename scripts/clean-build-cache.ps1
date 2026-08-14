[CmdletBinding(SupportsShouldProcess)]
param(
    [ValidateRange(0.1, 1024)]
    [double]$MaximumGiB
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$targetRoot = Join-Path $repoRoot "target"
if (-not (Test-Path -LiteralPath $targetRoot)) {
    Write-Host "No target directory exists. Nothing to clean."
    return
}

$resolvedTarget = (Resolve-Path -LiteralPath $targetRoot).Path.TrimEnd("\")
$buildDirectories = @("debug", "release", "m8-build", "m8-libclang", "tooling")
$resolvedBuildDirectories = @()
$removed = 0

foreach ($name in $buildDirectories) {
    $candidate = Join-Path $resolvedTarget $name
    if (-not (Test-Path -LiteralPath $candidate)) {
        continue
    }
    $resolved = (Resolve-Path -LiteralPath $candidate).Path
    $parent = [System.IO.Path]::GetDirectoryName($resolved).TrimEnd("\")
    if (-not $parent.Equals($resolvedTarget, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to clean a path outside the workspace target root: $resolved"
    }
    $resolvedBuildDirectories += $resolved
}

$cacheBytes = 0L
foreach ($directory in $resolvedBuildDirectories) {
    foreach ($file in [System.IO.Directory]::EnumerateFiles(
        $directory,
        '*',
        [System.IO.SearchOption]::AllDirectories
    )) {
        $cacheBytes += [System.IO.FileInfo]::new($file).Length
    }
}

if ($PSBoundParameters.ContainsKey('MaximumGiB')) {
    $maximumBytes = [long]($MaximumGiB * 1GB)
    $cacheGiB = $cacheBytes / 1GB
    if ($cacheBytes -lt $maximumBytes) {
        Write-Host ("Cargo build cache is {0:N2} GiB; automatic cleanup begins at {1:N2} GiB." -f $cacheGiB, $MaximumGiB)
        return
    }
    Write-Host ("Cargo build cache reached {0:N2} GiB (limit {1:N2} GiB); pruning regenerable outputs." -f $cacheGiB, $MaximumGiB)
}

foreach ($resolved in $resolvedBuildDirectories) {
    if ($PSCmdlet.ShouldProcess($resolved, "Remove regenerable Cargo build cache")) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
        $removed += 1
    }
}

Write-Host "Removed $removed build-cache directories."
if ($removed -gt 0) {
    Write-Host ("Recovered approximately {0:N2} GiB." -f ($cacheBytes / 1GB))
}
Write-Host "Preserved target/evals, target/eval-fixtures, and other benchmark artifacts."

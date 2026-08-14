[CmdletBinding(SupportsShouldProcess)]
param()

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$targetRoot = Join-Path $repoRoot "target"
if (-not (Test-Path -LiteralPath $targetRoot)) {
    Write-Host "No target directory exists. Nothing to clean."
    exit 0
}

$resolvedTarget = (Resolve-Path -LiteralPath $targetRoot).Path.TrimEnd("\")
$buildDirectories = @("debug", "release", "m8-build", "m8-libclang", "tooling")
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
    if ($PSCmdlet.ShouldProcess($resolved, "Remove regenerable Cargo build cache")) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
        $removed += 1
    }
}

Write-Host "Removed $removed build-cache directories."
Write-Host "Preserved target/evals, target/eval-fixtures, and other benchmark artifacts."

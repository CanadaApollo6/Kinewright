[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$BundleDir,

    [ValidateRange(3, 30)]
    [int]$StartupTimeoutSeconds = 8
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$BundleDir = [System.IO.Path]::GetFullPath($BundleDir)
$sourceExe = Join-Path $BundleDir 'Kinewright.exe'
if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) {
    throw "Staged executable not found: $sourceExe"
}

$requiredDllPatterns = @(
    'avcodec-*.dll',
    'avformat-*.dll',
    'avutil-*.dll',
    'swresample-*.dll',
    'swscale-*.dll'
)
foreach ($pattern in $requiredDllPatterns) {
    if (-not (Get-ChildItem -LiteralPath $BundleDir -File -Filter $pattern)) {
        throw "Bundle is missing required DLL pattern: $pattern"
    }
}

$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$smokeDir = [System.IO.Path]::GetFullPath((Join-Path $tempRoot "Kinewright-bundle-smoke-$([guid]::NewGuid().ToString('N'))"))
$tempPrefix = $tempRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
if (-not $smokeDir.StartsWith($tempPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to use smoke-test directory outside the system temp directory: $smokeDir"
}

$process = $null
try {
    New-Item -ItemType Directory -Path $smokeDir | Out-Null
    Copy-Item -Path (Join-Path $BundleDir '*') -Destination $smokeDir -Recurse -Force

    $smokeExe = Join-Path $smokeDir 'Kinewright.exe'
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $smokeExe
    $startInfo.WorkingDirectory = $smokeDir
    $startInfo.UseShellExecute = $false

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $originalPath = $env:Path
    try {
        $env:Path = "$env:SystemRoot\System32;$env:SystemRoot"
        if (-not $process.Start()) {
            throw 'Kinewright.exe did not start.'
        }
    } finally {
        $env:Path = $originalPath
    }

    if ($process.WaitForExit($StartupTimeoutSeconds * 1000)) {
        throw "Kinewright.exe exited during the startup smoke window with code $($process.ExitCode)."
    }

    Write-Host "Kinewright.exe stayed running for $StartupTimeoutSeconds seconds from an isolated directory with a system-only PATH."
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        if (-not $process.WaitForExit(5000)) {
            throw "Smoke-test process $($process.Id) did not stop within five seconds."
        }
    }
    if (Test-Path -LiteralPath $smokeDir) {
        Remove-Item -LiteralPath $smokeDir -Recurse -Force
    }
}

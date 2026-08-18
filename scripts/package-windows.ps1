[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')]
    [string]$Version,

    [string]$FfmpegDir = $env:FFMPEG_DIR,
    [string]$TargetDir,
    [string]$OutputDir,
    [string]$IsccPath,
    [switch]$StageOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($FfmpegDir)) {
    $FfmpegDir = Join-Path $repoRoot 'third_party\ffmpeg'
}
if ([string]::IsNullOrWhiteSpace($TargetDir)) {
    $TargetDir = Join-Path $repoRoot 'target\release'
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $repoRoot 'dist\installer'
}

$FfmpegDir = [System.IO.Path]::GetFullPath($FfmpegDir)
$TargetDir = [System.IO.Path]::GetFullPath($TargetDir)
$OutputDir = [System.IO.Path]::GetFullPath($OutputDir)
$stageDir = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'dist\windows-x64'))
$repoPrefix = $repoRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
if (-not $stageDir.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to reset staging directory outside the repository: $stageDir"
}

$sourceExe = Join-Path $TargetDir 'kinewright-app.exe'
if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) {
    throw "Release executable not found: $sourceExe"
}

$ffmpegBin = Join-Path $FfmpegDir 'bin'
if (-not (Test-Path -LiteralPath $ffmpegBin -PathType Container)) {
    throw "FFmpeg bin directory not found: $ffmpegBin"
}

$dlls = @(Get-ChildItem -LiteralPath $ffmpegBin -File -Filter '*.dll' | Sort-Object Name)
if ($dlls.Count -eq 0) {
    throw "No FFmpeg shared DLLs found under $ffmpegBin"
}

$requiredDllPatterns = @(
    'avcodec-*.dll',
    'avdevice-*.dll',
    'avfilter-*.dll',
    'avformat-*.dll',
    'avutil-*.dll',
    'swresample-*.dll',
    'swscale-*.dll'
)
foreach ($pattern in $requiredDllPatterns) {
    if (-not ($dlls.Name -like $pattern)) {
        throw "Pinned FFmpeg build is missing required DLL pattern: $pattern"
    }
}

$ffmpegLicense = Join-Path $FfmpegDir 'LICENSE.txt'
if (-not (Test-Path -LiteralPath $ffmpegLicense -PathType Leaf)) {
    throw "Pinned FFmpeg build license not found: $ffmpegLicense"
}

if (Test-Path -LiteralPath $stageDir) {
    Remove-Item -LiteralPath $stageDir -Recurse -Force
}
$licensesDir = Join-Path $stageDir 'LICENSES'
New-Item -ItemType Directory -Path $licensesDir -Force | Out-Null
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null

$stagedExe = Join-Path $stageDir 'Kinewright.exe'
Copy-Item -LiteralPath $sourceExe -Destination $stagedExe
foreach ($dll in $dlls) {
    Copy-Item -LiteralPath $dll.FullName -Destination $stageDir
}

# The FFmpeg CLI powers in-editor recording (screen/camera/mic capture).
$ffmpegCli = Join-Path $ffmpegBin 'ffmpeg.exe'
if (-not (Test-Path -LiteralPath $ffmpegCli -PathType Leaf)) {
    throw "Pinned FFmpeg build is missing ffmpeg.exe (required for in-editor recording)"
}
Copy-Item -LiteralPath $ffmpegCli -Destination $stageDir

Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination (Join-Path $licensesDir 'Kinewright-GPL-3.0.txt')
Copy-Item -LiteralPath $ffmpegLicense -Destination (Join-Path $licensesDir 'FFmpeg-GPL.txt')
Copy-Item -LiteralPath (Join-Path $repoRoot 'crates\kinewright-app\assets\licenses\Inter-OFL.txt') -Destination $licensesDir
Copy-Item -LiteralPath (Join-Path $repoRoot 'crates\kinewright-app\assets\licenses\JetBrains-Mono-OFL.txt') -Destination $licensesDir
Copy-Item -LiteralPath (Join-Path $repoRoot 'packaging\windows\LICENSES\README.txt') -Destination $licensesDir

$ffmpegBuildInfo = Join-Path $FfmpegDir 'BUILD_INFO'
if (Test-Path -LiteralPath $ffmpegBuildInfo -PathType Leaf) {
    Copy-Item -LiteralPath $ffmpegBuildInfo -Destination (Join-Path $licensesDir 'FFmpeg-BUILD-INFO.txt')
}

$fileInfo = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($stagedExe)
if ($fileInfo.ProductName -ne 'Kinewright') {
    throw "Kinewright.exe does not contain the expected product resource; ProductName was '$($fileInfo.ProductName)'"
}
if ($fileInfo.ProductVersion -ne $Version) {
    throw "Kinewright.exe product version '$($fileInfo.ProductVersion)' does not match package version '$Version'"
}

$icon = Get-ChildItem -LiteralPath (Join-Path $TargetDir 'build') -Recurse -File -Filter 'Kinewright.ico' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
if (-not $icon) {
    throw "Generated Kinewright.ico was not found under $(Join-Path $TargetDir 'build')"
}

if ($StageOnly) {
    Write-Host "Staged Kinewright.exe plus $($dlls.Count) FFmpeg DLLs in $stageDir"
    Write-Host 'Stage-only mode: Inno Setup compilation was skipped.'
    return
}

if ([string]::IsNullOrWhiteSpace($IsccPath)) {
    $isccCommand = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
    if ($isccCommand) {
        $IsccPath = $isccCommand.Source
    } else {
        $isccCandidates = @(
            (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
            (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 7\ISCC.exe'),
            (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
            (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 7\ISCC.exe')
        )
        $IsccPath = $isccCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    }
}
if ([string]::IsNullOrWhiteSpace($IsccPath) -or -not (Test-Path -LiteralPath $IsccPath -PathType Leaf)) {
    throw 'ISCC.exe was not found. GitHub windows-latest includes Inno Setup; local builds require Inno Setup 6 or 7.'
}
$IsccPath = [System.IO.Path]::GetFullPath($IsccPath)
$isccVersion = (Get-Item -LiteralPath $IsccPath).VersionInfo.ProductVersion
Write-Host "Inno Setup compiler: $IsccPath ($isccVersion)"

$installerName = "Kinewright-$Version-windows-x64-setup.exe"
$installerPath = Join-Path $OutputDir $installerName
if (Test-Path -LiteralPath $installerPath -PathType Leaf) {
    Remove-Item -LiteralPath $installerPath -Force
}

$environmentNames = @(
    'KINEWRIGHT_APP_VERSION',
    'KINEWRIGHT_NUMERIC_VERSION',
    'KINEWRIGHT_STAGE_DIR',
    'KINEWRIGHT_OUTPUT_DIR',
    'KINEWRIGHT_REPO_ROOT',
    'KINEWRIGHT_APP_ICON'
)
$previousEnvironment = @{}
foreach ($name in $environmentNames) {
    $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

try {
    $env:KINEWRIGHT_APP_VERSION = $Version
    $env:KINEWRIGHT_NUMERIC_VERSION = $Version.Split('-')[0]
    $env:KINEWRIGHT_STAGE_DIR = $stageDir
    $env:KINEWRIGHT_OUTPUT_DIR = $OutputDir
    $env:KINEWRIGHT_REPO_ROOT = $repoRoot
    $env:KINEWRIGHT_APP_ICON = $icon.FullName

    & $IsccPath (Join-Path $repoRoot 'packaging\windows\Kinewright.iss')
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup failed with exit code $LASTEXITCODE"
    }
} finally {
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process')
    }
}

if (-not (Test-Path -LiteralPath $installerPath -PathType Leaf)) {
    throw "Inno Setup did not produce the expected installer: $installerPath"
}

Write-Host "Staged Kinewright.exe plus $($dlls.Count) FFmpeg DLLs in $stageDir"
Write-Host "Installer: $installerPath"

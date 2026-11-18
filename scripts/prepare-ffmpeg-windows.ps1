$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$deps = Join-Path $root 'third_party/ffmpeg-deps'
$pin = '0df93a05b9de080129e653ec4776d7e14a1b2e4f'
if (-not (Test-Path "$deps/.git")) {
    git clone https://github.com/microsoft/vcpkg.git $deps
    if ($LASTEXITCODE -ne 0) { throw 'vcpkg clone failed' }
}
git -C $deps checkout --detach $pin
if ($LASTEXITCODE -ne 0) { throw 'vcpkg pin failed' }
& "$deps/bootstrap-vcpkg.bat" -disableMetrics
if ($LASTEXITCODE -ne 0) { throw 'vcpkg bootstrap failed' }
$env:VCPKG_MAX_CONCURRENCY = '4'
# vcpkg keeps each built package here under its own hash of compiler, triplet and
# port. CI restores this folder, so an unchanged pin installs without fetching a
# source archive from any upstream host.
if (-not $env:VCPKG_DEFAULT_BINARY_CACHE) {
    $env:VCPKG_DEFAULT_BINARY_CACHE = Join-Path $root 'third_party/vcpkg-archives'
}
New-Item -ItemType Directory -Force -Path $env:VCPKG_DEFAULT_BINARY_CACHE | Out-Null
# Same choice as setup-ffmpeg.ps1: when the newest Visual Studio also carries a
# 2022 toolset (14.3x or 14.4x), vcpkg builds with it. The 2026 toolset rejects
# the pinned dav1d. The triplet keeps its name, so the installed path is unchanged.
$tripletArguments = @()
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (Test-Path -LiteralPath $vswhere) {
    $visualStudioRoot = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $toolsRoot = if ($visualStudioRoot) { Join-Path $visualStudioRoot 'VC\Tools\MSVC' } else { $null }
    if ($toolsRoot -and (Test-Path -LiteralPath $toolsRoot -PathType Container) -and
        (Get-ChildItem -LiteralPath $toolsRoot -Directory | Where-Object { $_.Name -match '^14\.[34]\d\.\d+$' })) {
        $triplets = Join-Path $deps 'kinewright-triplets'
        New-Item -ItemType Directory -Force -Path $triplets | Out-Null
        Set-Content -LiteralPath (Join-Path $triplets 'x64-windows-static.cmake') -Encoding ascii -Value @(
            'set(VCPKG_TARGET_ARCHITECTURE x64)',
            'set(VCPKG_CRT_LINKAGE static)',
            'set(VCPKG_LIBRARY_LINKAGE static)',
            'set(VCPKG_PLATFORM_TOOLSET v143)'
        )
        $tripletArguments = @("--overlay-triplets=$triplets")
    }
}
# An upstream source host sometimes returns a damaged archive (vcpkg then reports
# "unexpected hash"). vcpkg never uses such a file; a later attempt downloads it
# again and keeps the packages already built.
$attempts = 3
for ($attempt = 1; ; $attempt++) {
    & "$deps/vcpkg.exe" install --triplet x64-windows-static @tripletArguments x264 x265 dav1d zlib libvpx freetype harfbuzz
    if ($LASTEXITCODE -eq 0) { break }
    if ($attempt -ge $attempts) { throw 'FFmpeg dependencies failed' }
    Write-Host "vcpkg install failed (attempt $attempt of $attempts); trying again in 30 seconds."
    Start-Sleep -Seconds 30
}

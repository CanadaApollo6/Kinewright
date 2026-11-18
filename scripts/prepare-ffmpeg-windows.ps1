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
& "$deps/vcpkg.exe" install --triplet x64-windows-static @tripletArguments x264 x265 dav1d zlib libvpx freetype harfbuzz
if ($LASTEXITCODE -ne 0) { throw 'FFmpeg dependencies failed' }

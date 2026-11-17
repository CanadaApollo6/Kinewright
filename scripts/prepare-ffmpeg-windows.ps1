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
& "$deps/vcpkg.exe" install --triplet x64-windows-static x264 x265 dav1d zlib libvpx freetype harfbuzz
if ($LASTEXITCODE -ne 0) { throw 'FFmpeg dependencies failed' }

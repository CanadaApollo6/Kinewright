[CmdletBinding()]
param(
    [ValidateRange(0.1, 1024)]
    [double]$BuildCacheLimitGiB = 6,
    [switch]$SkipBuildCachePrune
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$originalPath = $env:Path

$FfmpegUrl = 'https://github.com/System233/ffmpeg-msvc-prebuilt/releases/download/ffmpeg-8.0.1-r3/ffmpeg-8.0.1-r3_x64-windows-shared-gpl.zip'
$FfmpegSha256 = '3399afab045f6bc64301001d4f5ca1aba3d6df96948cc1799028cf2f24ede433'
$PkgconfVersion = '3.0.1.post0'
$LibclangVersion = '18.1.1'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$thirdParty = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'third_party'))
$ffmpegRoot = [System.IO.Path]::GetFullPath((Join-Path $thirdParty 'ffmpeg'))
$pkgconfRoot = [System.IO.Path]::GetFullPath((Join-Path $thirdParty 'pkgconf'))
$libclangRoot = [System.IO.Path]::GetFullPath((Join-Path $thirdParty 'libclang'))
$archive = [System.IO.Path]::GetFullPath((Join-Path $thirdParty 'ffmpeg.zip'))

if (-not $SkipBuildCachePrune) {
    & (Join-Path $PSScriptRoot 'clean-build-cache.ps1') -MaximumGiB $BuildCacheLimitGiB
}

foreach ($path in @($ffmpegRoot, $pkgconfRoot, $libclangRoot, $archive)) {
    if (-not $path.StartsWith($thirdParty, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to modify a path outside $thirdParty`: $path"
    }
}

$python = Get-Command python -ErrorAction SilentlyContinue
if (-not $python) {
    throw 'Python 3 is required to download FFmpeg and install the pinned build helpers.'
}

$programFilesX86 = ${env:ProgramFiles(x86)}
$vswhere = Join-Path $programFilesX86 'Microsoft Visual Studio\Installer\vswhere.exe'
$vcvars = $null
if (Test-Path -LiteralPath $vswhere) {
    $visualStudioRoot = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($visualStudioRoot) {
        $candidate = Join-Path $visualStudioRoot 'VC\Auxiliary\Build\vcvars64.bat'
        if (Test-Path -LiteralPath $candidate) {
            $vcvars = $candidate
        }
    }
}
if (-not $vcvars) {
    $visualStudioSearchRoots = @(
        (Join-Path $env:ProgramFiles 'Microsoft Visual Studio'),
        (Join-Path $programFilesX86 'Microsoft Visual Studio')
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Container }
    $vcvars = Get-ChildItem -Path $visualStudioSearchRoots -Filter 'vcvars64.bat' -Recurse -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $vcvars) {
    throw 'Visual Studio C++ Build Tools were not found. Install Desktop development with C++.'
}

# Import the environment emitted by vcvars64.bat into this PowerShell process.
$vcvarsCommand = "call `"$vcvars`" >nul 2>nul && set"
$vcvarsPath = $null
foreach ($line in (& $env:ComSpec /d /c $vcvarsCommand)) {
    if ($line -match '^([^=]+)=(.*)$') {
        if ($Matches[1] -ieq 'PATH') {
            if (-not $vcvarsPath -or $Matches[2] -match '\\VC\\Tools\\MSVC\\') {
                $vcvarsPath = $Matches[2]
            }
        } else {
            [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
        }
    }
}
$env:Path = "$vcvarsPath;$originalPath"
if (-not $vcvarsPath -or -not $env:INCLUDE -or -not $env:LIB) {
    throw "Failed to import the MSVC build environment from $vcvars"
}

New-Item -ItemType Directory -Path $thirdParty -Force | Out-Null

$marker = Join-Path $ffmpegRoot '.archive-sha256'
$installedHash = if (Test-Path -LiteralPath $marker) {
    (Get-Content -LiteralPath $marker -Raw).Trim()
} else {
    ''
}

if ($installedHash -ne $FfmpegSha256) {
    if (Test-Path -LiteralPath $ffmpegRoot) {
        Remove-Item -LiteralPath $ffmpegRoot -Recurse -Force
    }
    if (Test-Path -LiteralPath $archive) {
        Remove-Item -LiteralPath $archive -Force
    }

    Write-Host 'Downloading pinned FFmpeg 8.0.1 MSVC shared GPL build...'
    $download = @'
import sys
import urllib.request

request = urllib.request.Request(sys.argv[1], headers={'User-Agent': 'OpenReel-M0'})
with urllib.request.urlopen(request, timeout=60) as source, open(sys.argv[2], 'wb') as target:
    while chunk := source.read(1024 * 1024):
        target.write(chunk)
'@
    & $python.Source -c $download $FfmpegUrl $archive
    if ($LASTEXITCODE -ne 0) {
        throw 'FFmpeg download failed.'
    }

    $actualHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $FfmpegSha256) {
        throw "FFmpeg archive SHA-256 mismatch. Expected $FfmpegSha256, got $actualHash."
    }

    New-Item -ItemType Directory -Path $ffmpegRoot -Force | Out-Null
    Expand-Archive -LiteralPath $archive -DestinationPath $ffmpegRoot -Force
    Set-Content -LiteralPath $marker -Value $FfmpegSha256 -NoNewline
    Remove-Item -LiteralPath $archive -Force
}

$pkgConfig = Join-Path $pkgconfRoot 'pkgconf\.bin\pkgconf.exe'
$libclang = Get-ChildItem -LiteralPath $libclangRoot -Filter 'libclang.dll' -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1

if (-not (Test-Path -LiteralPath $pkgConfig)) {
    if (Test-Path -LiteralPath $pkgconfRoot) {
        Remove-Item -LiteralPath $pkgconfRoot -Recurse -Force
    }
    Write-Host "Installing pkgconf $PkgconfVersion under third_party..."
    & $python.Source -m pip install --disable-pip-version-check --no-deps --no-cache-dir --target $pkgconfRoot "pkgconf==$PkgconfVersion"
    if ($LASTEXITCODE -ne 0) {
        throw 'pkgconf installation failed.'
    }
} else {
    Write-Host "Using existing pkgconf under third_party."
}

if (-not $libclang) {
    if (Test-Path -LiteralPath $libclangRoot) {
        Remove-Item -LiteralPath $libclangRoot -Recurse -Force
    }
    Write-Host "Installing libclang $LibclangVersion under third_party..."
    & $python.Source -m pip install --disable-pip-version-check --no-deps --no-cache-dir --target $libclangRoot "libclang==$LibclangVersion"
    if ($LASTEXITCODE -ne 0) {
        throw 'libclang installation failed.'
    }
    $libclang = Get-ChildItem -LiteralPath $libclangRoot -Filter 'libclang.dll' -Recurse | Select-Object -First 1
} else {
    Write-Host "Using existing libclang under third_party."
}

$pkgConfigBin = Split-Path -Parent $pkgConfig
$pkgConfigPath = Join-Path $ffmpegRoot 'lib\pkgconfig'
$ffmpegBin = Join-Path $ffmpegRoot 'bin'

foreach ($required in @(
    (Join-Path $ffmpegRoot 'include\libavcodec\avcodec.h'),
    (Join-Path $ffmpegRoot 'lib\avcodec.lib'),
    (Join-Path $ffmpegBin 'avcodec-62.dll'),
    $pkgConfig
)) {
    if (-not (Test-Path -LiteralPath $required)) {
        throw "Provisioning did not produce required file: $required"
    }
}
if (-not $libclang) {
    throw "Provisioning did not produce libclang.dll under $libclangRoot"
}

# ffmpeg-sys-next discovers headers and import libraries through pkg-config and
# bindgen discovers libclang through LIBCLANG_PATH. PATH is also required when
# its build-time feature probe and the Rust tests load FFmpeg's DLLs.
$env:PKG_CONFIG = $pkgConfig
$env:PKG_CONFIG_PATH = $pkgConfigPath
$env:LIBCLANG_PATH = $libclang.DirectoryName
$env:FFMPEG_DIR = $ffmpegRoot
$env:Path = "$ffmpegBin;$pkgConfigBin;$env:Path"
$env:BINDGEN_EXTRA_CLANG_ARGS = (($env:INCLUDE -split ';' | Where-Object { $_ }) | ForEach-Object { '-I"{0}"' -f $_ }) -join ' '

$codecVersion = & $pkgConfig --modversion libavcodec
if ($LASTEXITCODE -ne 0) {
    throw 'pkg-config could not resolve libavcodec.'
}
$ffmpegVersion = (& (Join-Path $ffmpegBin 'ffmpeg.exe') -version | Select-Object -First 1)

Write-Host "FFmpeg root: $ffmpegRoot"
Write-Host "libavcodec: $codecVersion"
Write-Host $ffmpegVersion
Write-Host 'FFmpeg build environment is active in this PowerShell process.'

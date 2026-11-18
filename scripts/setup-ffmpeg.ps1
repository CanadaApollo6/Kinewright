[CmdletBinding()]
param(
    [ValidateRange(0.1, 1024)]
    [double]$BuildCacheLimitGiB = 6,
    [switch]$SkipBuildCachePrune,
    [switch]$NemoCacheKeyOnly,
    [switch]$ToolsOnly,
    [switch]$NoBinaryCache,
    # The CI FFmpeg producer builds only the FFmpeg profiles; consumers add NeMo-Speech.
    [switch]$FfmpegOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$originalPath = $env:Path

$PkgconfVersion = '3.0.1.post0'
$LibclangVersion = '18.1.1'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$thirdParty = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'third_party'))
$ffmpegRoot = [System.IO.Path]::GetFullPath((Join-Path $thirdParty 'ffmpeg'))
$pkgconfRoot = [System.IO.Path]::GetFullPath((Join-Path $thirdParty 'pkgconf'))
$libclangRoot = [System.IO.Path]::GetFullPath((Join-Path $thirdParty 'libclang'))

if (-not $ToolsOnly -and -not $SkipBuildCachePrune -and -not $NemoCacheKeyOnly) {
    & (Join-Path $PSScriptRoot 'clean-build-cache.ps1') -MaximumGiB $BuildCacheLimitGiB
}

foreach ($path in @($ffmpegRoot, $pkgconfRoot, $libclangRoot)) {
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

# Visual Studio 2026 (toolset 14.5x) rejects the pinned dav1d, and the Windows
# test pins were measured with the 2022 toolset. When the installation also
# carries a 2022 toolset (14.3x or 14.4x), every C and Rust build uses the
# newest one. prepare-ffmpeg-windows.ps1 makes the same choice for vcpkg.
$vcvarsToolset = ''
$toolsRoot = Join-Path (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $vcvars))) 'Tools\MSVC'
if (Test-Path -LiteralPath $toolsRoot -PathType Container) {
    $toolset2022 = Get-ChildItem -LiteralPath $toolsRoot -Directory |
        Where-Object { $_.Name -match '^14\.[34]\d\.\d+$' -and (Test-Path -LiteralPath (Join-Path $_.FullName 'bin\Hostx64\x64\cl.exe')) } |
        Sort-Object { [version]$_.Name } |
        Select-Object -Last 1
    if ($toolset2022) {
        $vcvarsToolset = " -vcvars_ver=$($toolset2022.Name)"
    }
}

# Import the environment emitted by vcvars64.bat into this PowerShell process.
$vcvarsCommand = "call `"$vcvars`"$vcvarsToolset >nul 2>nul && set"
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
    throw "Failed to import the MSVC build environment from $vcvars$vcvarsToolset"
}

if ($NemoCacheKeyOnly) {
    & $python.Source -I (Join-Path $PSScriptRoot 'nemo_speech.py') --print-key
    if ($LASTEXITCODE -ne 0) { throw 'NeMo cache key failed.' }
    return
}

New-Item -ItemType Directory -Path $thirdParty -Force | Out-Null

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

# Build with MSVC in MSYS2, linking the explicit static vcpkg dependencies.
$deps = Join-Path $thirdParty 'ffmpeg-deps/installed/x64-windows-static'
$env:PKG_CONFIG = $pkgConfig
$env:PKG_CONFIG_PATH = "$deps/lib/pkgconfig"
$env:INCLUDE = "$deps/include;$env:INCLUDE"
$env:LIB = "$deps/lib;$env:LIB"
$msvcBin = Split-Path -Parent (Get-Command cl.exe -ErrorAction Stop).Source
# MSVC cl/link first, then one coherent MSYS make/sh/awk toolset, before inherited tools.
$msysRoot = if ($env:KINEWRIGHT_MSYS_ROOT) { $env:KINEWRIGHT_MSYS_ROOT } else { 'C:\msys64' }
$msysBin = Join-Path $msysRoot 'usr\bin'
$env:Path = "$msvcBin;$msysBin;$vcvarsPath;$(Split-Path -Parent $pkgConfig);$originalPath"
# Repeated ToolsOnly/setup calls must preserve search order without growing PATH.
foreach ($name in @('Path', 'INCLUDE', 'LIB', 'LIBPATH')) {
    $value = [Environment]::GetEnvironmentVariable($name, 'Process')
    if ($value) {
        $seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
        $unique = ($value -split ';' | Where-Object { $_ -and $seen.Add($_) }) -join ';'
        [Environment]::SetEnvironmentVariable($name, $unique, 'Process')
    }
}
$env:VSLANG = '1033'
function Export-BuildEnvironment {
    if ($env:GITHUB_ENV) {
        foreach ($name in @('Path','INCLUDE','LIB','LIBPATH','WindowsSdkDir','WindowsSDKVersion','VCToolsVersion','VSCMD_ARG_TGT_ARCH','VSLANG','PKG_CONFIG','PKG_CONFIG_PATH','LIBCLANG_PATH','FFMPEG_DIR','KINEWRIGHT_NEMO_DIR','BINDGEN_EXTRA_CLANG_ARGS')) {
            $value = [Environment]::GetEnvironmentVariable($name, 'Process')
            if ($value) { "$name=$value" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append }
        }
    }
}
if ($ToolsOnly) { Export-BuildEnvironment; return }
$cacheArguments = @()
if ($NoBinaryCache) { $cacheArguments += '--no-binary-cache' }
foreach ($profile in @('libraries', 'recorder')) {
    & $python.Source (Join-Path $PSScriptRoot 'build-ffmpeg.py') --profile $profile @cacheArguments
    if ($LASTEXITCODE -ne 0) { throw "FFmpeg $profile build failed" }
}

$pkgConfigBin = Split-Path -Parent $pkgConfig
$pkgConfigPath = Join-Path $ffmpegRoot 'lib\pkgconfig'
$ffmpegBin = Join-Path $thirdParty 'ffmpeg-recorder/bin'
$ffmpegDlls = Join-Path $ffmpegRoot 'bin'

foreach ($required in @(
    (Join-Path $ffmpegRoot 'include\libavcodec\avcodec.h'),
    (Join-Path $ffmpegRoot 'lib\avcodec.lib'),
    (Join-Path $ffmpegDlls 'avcodec-63.dll'),
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
$env:Path = "$ffmpegDlls;$ffmpegBin;$pkgConfigBin;$env:Path"
$env:BINDGEN_EXTRA_CLANG_ARGS = (($env:INCLUDE -split ';' | Where-Object { $_ }) | ForEach-Object { '-I"{0}"' -f $_ }) -join ' '

if (-not $FfmpegOnly) { & (Join-Path $PSScriptRoot 'setup-nemo-speech.ps1') }

if ($env:CI -eq 'true') {
    & $python.Source (Join-Path $PSScriptRoot 'build-ffmpeg.py') --log-digests
    if ($LASTEXITCODE -ne 0) { throw 'FFmpeg binary digest logging failed' }
}
Export-BuildEnvironment
$codecVersion = & $pkgConfig --modversion libavcodec
if ($LASTEXITCODE -ne 0) {
    throw 'pkg-config could not resolve libavcodec.'
}
$ffmpegVersion = (& (Join-Path $ffmpegBin 'ffmpeg.exe') -version | Select-Object -First 1)

Write-Host "FFmpeg root: $ffmpegRoot"
Write-Host "libavcodec: $codecVersion"
Write-Host $ffmpegVersion
Write-Host 'FFmpeg build environment is active in this PowerShell process.'

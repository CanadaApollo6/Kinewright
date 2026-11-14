$ErrorActionPreference = 'Stop'
# setup-ffmpeg.ps1 has already imported the x64 MSVC environment.
$nemoRoot = & python -I (Join-Path $PSScriptRoot 'nemo_speech.py')
if ($LASTEXITCODE -ne 0) { throw 'NeMo-Speech source provisioning failed.' }
$env:KINEWRIGHT_NEMO_DIR = "$nemoRoot".Trim()
$env:Path = "$(Join-Path $env:KINEWRIGHT_NEMO_DIR 'bin');$env:Path"
Write-Host "NeMo-Speech root: $env:KINEWRIGHT_NEMO_DIR"

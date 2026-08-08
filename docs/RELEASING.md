# Releasing OpenReel for Windows

OpenReel releases are built by `.github/workflows/release.yml`. A pushed tag matching `v*` runs the full Windows test suite, builds the release executable, bundles the pinned GPL FFmpeg shared DLLs, compiles the installer, smoke-tests the staged bundle with FFmpeg removed from `PATH`, and publishes both a GitHub Actions artifact and GitHub Release assets.

## Installer toolchain

The release uses Inno Setup. GitHub's current `windows-latest` image includes both Inno Setup and WiX, but Inno Setup is the smaller fit for one executable plus adjacent DLLs. It provides the GPLv3 license page, install-directory and Start Menu pages, per-user installation, shortcuts, and an uninstaller without MSI component bookkeeping.

The packaging script locates `ISCC.exe`, prints its path and version, and fails if it is absent. This is an intentional check against runner-image drift. The current runner inventory is documented at <https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md>.

The `.openreel` file association is deferred. The application does not yet accept a project path on its command line, so registering `%1` would create a shortcut that cannot open the selected project. Add the association after that CLI contract exists.

## Cut a release

1. Update `[workspace.package].version` in the root `Cargo.toml`. All package versions stay inherited from the workspace root.
2. Run the normal CI checks and merge the release commit.
3. Create and push the matching tag. The workflow rejects a tag that does not exactly match the workspace version.

```powershell
git tag v0.1.0
git push origin v0.1.0
```

4. Open the `Release` workflow run. Download `OpenReel-0.1.0-windows-x64` from the run summary if needed.
5. After the job succeeds, verify the GitHub Release contains:

   - `OpenReel-0.1.0-windows-x64-setup.exe`
   - `OpenReel-0.1.0-windows-x64-setup.exe.sha256`

The installer is currently unsigned. Windows SmartScreen may warn until a code-signing certificate is wired into the workflow. Signing is not required to build or install the artifact.

## What is packaged

`scripts/package-windows.ps1` stages:

- `target/release/openreel-app.exe` as `OpenReel.exe`;
- every shared DLL from the pinned `third_party/ffmpeg/bin` build, beside the executable;
- the OpenReel GPLv3 license;
- the FFmpeg license, build metadata when present, attribution, and source-availability links.

Windows searches an unpackaged desktop application's executable directory when resolving its DLL imports. Keeping the FFmpeg DLLs beside `OpenReel.exe` therefore avoids a machine-level `PATH` or FFmpeg installation. See Microsoft's [dynamic-link library search order](https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order).

The release build enables the MSVC static C runtime with `-C target-feature=+crt-static`, reducing clean-machine runtime dependencies. The workflow also prints `dumpbin /dependents` output for the executable in the build log.

## Local installer build

Do not run `scripts/setup-ffmpeg.ps1` against the shared local `third_party` tree. Use the already provisioned paths and a Visual Studio developer PowerShell so `rc.exe` is available for the executable's icon and version resources.

```powershell
$env:FFMPEG_DIR = 'C:\Users\Riel St Amand\Documents\GitHub\OpenReel\third_party\ffmpeg'
$env:PKG_CONFIG = 'C:\Users\Riel St Amand\Documents\GitHub\OpenReel\third_party\pkgconf\pkgconf\.bin\pkgconf.exe'
$env:PKG_CONFIG_PATH = 'C:\Users\Riel St Amand\Documents\GitHub\OpenReel\third_party\ffmpeg\lib\pkgconfig'
$env:LIBCLANG_PATH = 'C:\Users\Riel St Amand\Documents\GitHub\OpenReel\third_party\libclang\clang\native'
$env:CARGO_HOME = 'C:\Users\Riel St Amand\Documents\GitHub\OpenReel\.cargo-home'
$rustBin = 'C:\Users\Riel St Amand\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin'
$env:Path = "$env:FFMPEG_DIR\bin;$rustBin;$env:Path"

cargo test --workspace --locked
$env:RUSTFLAGS = '-C target-feature=+crt-static'
cargo build --package openreel-app --release --locked
.\scripts\package-windows.ps1 -Version 0.1.0
.\scripts\test-windows-bundle.ps1 -BundleDir .\dist\windows-x64
Get-FileHash .\dist\installer\OpenReel-0.1.0-windows-x64-setup.exe -Algorithm SHA256
```

Inno Setup 6 or 7 is required locally. Its own installer supports `/PORTABLE=1`, so it can be installed into a worktree-local tools directory without an uninstall entry. Pass its compiler explicitly when it is not on `PATH`:

```powershell
.\scripts\package-windows.ps1 -Version 0.1.0 -IsccPath .\.tools\innosetup\ISCC.exe
```

If local policy prevents running Inno Setup, stage and smoke-test the bundle without compiling the installer. The tag workflow remains the canonical installer builder.

```powershell
.\scripts\package-windows.ps1 -Version 0.1.0 -StageOnly
.\scripts\test-windows-bundle.ps1 -BundleDir .\dist\windows-x64
```

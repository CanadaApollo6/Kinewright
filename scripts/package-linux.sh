#!/usr/bin/env bash
# Stage OpenReel plus the pinned FFmpeg shared libraries for Linux x64.
set -euo pipefail

usage() {
    echo "Usage: $0 --version <semver> [--ffmpeg-dir DIR] [--target-dir DIR] [--output-dir DIR] [--stage-only]" >&2
    exit 2
}

version=""
ffmpeg_dir="${FFMPEG_DIR:-}"
target_dir=""
output_dir=""
stage_only=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            version="${2:-}"
            shift 2
            ;;
        --ffmpeg-dir)
            ffmpeg_dir="${2:-}"
            shift 2
            ;;
        --target-dir)
            target_dir="${2:-}"
            shift 2
            ;;
        --output-dir)
            output_dir="${2:-}"
            shift 2
            ;;
        --stage-only)
            stage_only=1
            shift
            ;;
        *)
            usage
            ;;
    esac
done

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
    echo "A semantic version is required, for example --version 0.1.0" >&2
    exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
if [[ -z "$ffmpeg_dir" ]]; then
    ffmpeg_dir="$repo_root/third_party/ffmpeg"
fi
if [[ -z "$target_dir" ]]; then
    target_dir="$repo_root/target/release"
fi
if [[ -z "$output_dir" ]]; then
    output_dir="$repo_root/dist/tarball"
fi

ffmpeg_dir="$(cd -- "$ffmpeg_dir" && pwd)"
target_dir="$(cd -- "$target_dir" && pwd)"
mkdir -p "$output_dir"
output_dir="$(cd -- "$output_dir" && pwd)"
stage_dir="$repo_root/dist/linux-x64"

source_bin="$target_dir/openreel-app"
if [[ ! -f "$source_bin" ]]; then
    echo "Release executable not found: $source_bin" >&2
    exit 1
fi

ffmpeg_bin_dir="$ffmpeg_dir/bin"
ffmpeg_lib_dir="$ffmpeg_dir/lib"
if [[ ! -d "$ffmpeg_bin_dir" ]]; then
    echo "FFmpeg bin directory not found: $ffmpeg_bin_dir" >&2
    exit 1
fi
if [[ ! -d "$ffmpeg_lib_dir" ]]; then
    echo "FFmpeg lib directory not found: $ffmpeg_lib_dir" >&2
    exit 1
fi

required_libs=(libavcodec.so libavdevice.so libavfilter.so libavformat.so libavutil.so libswresample.so libswscale.so)
for lib in "${required_libs[@]}"; do
    if [[ ! -e "$ffmpeg_lib_dir/$lib" ]]; then
        echo "Pinned FFmpeg build is missing required library: $lib" >&2
        exit 1
    fi
done

ffmpeg_cli="$ffmpeg_bin_dir/ffmpeg"
if [[ ! -f "$ffmpeg_cli" ]]; then
    echo "Pinned FFmpeg build is missing ffmpeg (required for in-editor recording)" >&2
    exit 1
fi

ffmpeg_license="$ffmpeg_dir/LICENSE.txt"
if [[ ! -f "$ffmpeg_license" ]]; then
    echo "Pinned FFmpeg build license not found: $ffmpeg_license" >&2
    exit 1
fi

rm -rf "$stage_dir"
licenses_dir="$stage_dir/LICENSES"
mkdir -p "$licenses_dir" "$stage_dir/lib" "$output_dir"

cp "$source_bin" "$stage_dir/OpenReel"
chmod +x "$stage_dir/OpenReel"
cp "$ffmpeg_cli" "$stage_dir/ffmpeg"
chmod +x "$stage_dir/ffmpeg"
if [[ -f "$ffmpeg_bin_dir/ffprobe" ]]; then
    cp "$ffmpeg_bin_dir/ffprobe" "$stage_dir/ffprobe"
    chmod +x "$stage_dir/ffprobe"
fi

# Copy the libav family and every SONAME symlink the loader needs.
find "$ffmpeg_lib_dir" -maxdepth 1 \( -name 'libav*.so*' -o -name 'libsw*.so*' \) -exec cp -a {} "$stage_dir/lib/" \;

if command -v patchelf >/dev/null 2>&1; then
    for binary in "$stage_dir/OpenReel" "$stage_dir/ffmpeg"; do
        patchelf --set-rpath '$ORIGIN/lib' "$binary"
    done
    if [[ -f "$stage_dir/ffprobe" ]]; then
        patchelf --set-rpath '$ORIGIN/lib' "$stage_dir/ffprobe"
    fi
    for lib in "$stage_dir/lib"/lib*.so*; do
        if [[ -f "$lib" && ! -L "$lib" ]]; then
            patchelf --set-rpath '$ORIGIN' "$lib"
        fi
    done
else
    echo 'patchelf was not found; the staged binaries keep their original RPATH.' >&2
fi

cp "$repo_root/LICENSE" "$licenses_dir/OpenReel-GPL-3.0.txt"
cp "$ffmpeg_license" "$licenses_dir/FFmpeg-GPL.txt"
cp "$repo_root/crates/openreel-app/assets/licenses/Inter-OFL.txt" "$licenses_dir/"
cp "$repo_root/crates/openreel-app/assets/licenses/JetBrains-Mono-OFL.txt" "$licenses_dir/"
cp "$repo_root/packaging/linux/LICENSES/README.txt" "$licenses_dir/"
cp "$repo_root/packaging/linux/openreel.desktop" "$stage_dir/openreel.desktop"
cp "$repo_root/crates/openreel-app/assets/openreel-icon.png" "$stage_dir/openreel.png"

tarball_name="OpenReel-$version-linux-x64.tar.gz"
tarball_path="$output_dir/$tarball_name"

if [[ "$stage_only" -eq 1 ]]; then
    echo "Staged OpenReel plus FFmpeg shared libraries in $stage_dir"
    echo 'Stage-only mode: tarball creation was skipped.'
    exit 0
fi

rm -f "$tarball_path"
tar -C "$(dirname "$stage_dir")" -czf "$tarball_path" "$(basename "$stage_dir")"
echo "Staged OpenReel plus FFmpeg shared libraries in $stage_dir"
echo "Tarball: $tarball_path"

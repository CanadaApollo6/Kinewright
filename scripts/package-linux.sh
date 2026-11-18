#!/usr/bin/env bash
# Stage Kinewright, its media sandbox, and the pinned FFmpeg libraries for Linux x64.
set -euo pipefail

usage() {
    echo "Usage: $0 --version <semver> [--ffmpeg-dir DIR] [--target-dir DIR] [--output-dir DIR] [--stage-only] --no-binary-cache" >&2
    echo "       $0 --check-binaries [--target-dir DIR]" >&2
    exit 2
}

version=""
ffmpeg_dir="${FFMPEG_DIR:-}"
target_dir=""
output_dir=""
stage_only=0
no_binary_cache=0
check_binaries=0

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
        --no-binary-cache)
            no_binary_cache=1
            shift
            ;;
        --stage-only)
            stage_only=1
            shift
            ;;
        --check-binaries)
            check_binaries=1
            shift
            ;;
        *)
            usage
            ;;
    esac
done

if [[ "$check_binaries" -eq 0 ]]; then
    if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
        echo "A semantic version is required, for example --version 0.1.0" >&2
        exit 1
    fi
    if [[ "$no_binary_cache" -ne 1 ]]; then
        echo 'Packaging requires --no-binary-cache: compile verified FFmpeg source, never CI binary-cache outputs.' >&2
        exit 1
    fi
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
target_dir="${target_dir:-$repo_root/target/release}"
runtime_binaries=(kinewright-app kinewright-media-helper kinewright-media-supervisor)
for binary in "${runtime_binaries[@]}"; do
    if [[ ! -f "$target_dir/$binary" || ! -x "$target_dir/$binary" ]]; then
        echo "Required bundle executable is missing or not executable: $target_dir/$binary" >&2
        exit 1
    fi
done
if [[ "$check_binaries" -eq 1 ]]; then
    exit 0
fi

if ! command -v patchelf >/dev/null 2>&1; then
    echo 'patchelf is required to set the bundled library paths.' >&2
    exit 1
fi

if [[ -n "$ffmpeg_dir" && "$(realpath -m "$ffmpeg_dir")" != "$repo_root/third_party/ffmpeg" ]]; then
    echo 'Source-only packaging uses the owned verified FFmpeg install.' >&2
    exit 1
fi
# Mandatory source rebuild: a CI marker or cache digest is not release provenance.
source "$script_dir/setup-ffmpeg.sh" --no-binary-cache
ffmpeg_dir="$FFMPEG_DIR"
if [[ -z "$output_dir" ]]; then
    output_dir="$repo_root/dist/tarball"
fi

ffmpeg_dir="$(cd -- "$ffmpeg_dir" && pwd)"
target_dir="$(cd -- "$target_dir" && pwd)"
mkdir -p "$output_dir"
output_dir="$(cd -- "$output_dir" && pwd)"
stage_dir="$repo_root/dist/linux-x64"

ffmpeg_lib_dir="$ffmpeg_dir/lib"
if [[ ! -d "$ffmpeg_lib_dir" ]]; then
    echo "FFmpeg lib directory not found: $ffmpeg_lib_dir" >&2
    exit 1
fi

required_libs=(libavcodec.so libavfilter.so libavformat.so libavutil.so libswresample.so libswscale.so)
nemo_dir="${KINEWRIGHT_NEMO_DIR:?Source scripts/setup-ffmpeg.sh before packaging}"
for lib in libnemo_speech_asr.so libnemo_speech_asr_c.so; do
    if [[ ! -f "$nemo_dir/lib/$lib" ]]; then
        echo "NeMo SDK is missing required library: $lib" >&2
        exit 1
    fi
done
for lib in "${required_libs[@]}"; do
    if [[ ! -e "$ffmpeg_lib_dir/$lib" ]]; then
        echo "Pinned FFmpeg build is missing required library: $lib" >&2
        exit 1
    fi
done

ffmpeg_cli="$repo_root/third_party/ffmpeg-recorder/bin/ffmpeg"
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

for binary in "${runtime_binaries[@]}"; do
    destination="$binary"
    if [[ "$binary" == kinewright-app ]]; then
        destination=Kinewright
    fi
    install -m 755 "$target_dir/$binary" "$stage_dir/$destination"
done
install -m 755 "$ffmpeg_cli" "$stage_dir/ffmpeg"
# Copy the libav family and every SONAME symlink the loader needs.
find "$ffmpeg_lib_dir" -maxdepth 1 \( -name 'libav*.so*' -o -name 'libsw*.so*' \) -exec cp -a {} "$stage_dir/lib/" \;
cp -a "$nemo_dir"/lib/libnemo_speech_asr*.so* "$stage_dir/lib/"

nice -n 19 python3 -I "$repo_root/scripts/copy-ffmpeg-deps.py" "$ffmpeg_dir" "$repo_root/third_party/ffmpeg-recorder" "$stage_dir/lib"

for binary in Kinewright kinewright-media-helper ffmpeg; do
    patchelf --set-rpath '$ORIGIN/lib' "$stage_dir/$binary"
done
for lib in "$stage_dir/lib"/lib*.so*; do
    if [[ -f "$lib" && ! -L "$lib" ]]; then
        patchelf --set-rpath '$ORIGIN' "$lib"
    fi
done

cp "$repo_root/LICENSE" "$licenses_dir/Kinewright-GPL-3.0.txt"
cp "$ffmpeg_license" "$licenses_dir/FFmpeg-GPL.txt"
cp -a "$repo_root/LICENSES/." "$licenses_dir/"
cp "$ffmpeg_dir/configure.txt" "$licenses_dir/FFmpeg-libraries-configure.txt"
cp "$repo_root/third_party/ffmpeg-recorder/configure.txt" "$licenses_dir/FFmpeg-recorder-configure.txt"
cp "$repo_root/crates/kinewright-app/assets/licenses/Inter-OFL.txt" "$licenses_dir/"
cp "$repo_root/crates/kinewright-app/assets/licenses/JetBrains-Mono-OFL.txt" "$licenses_dir/"
cp "$repo_root/packaging/linux/LICENSES/README.txt" "$licenses_dir/"
cp "$repo_root/packaging/linux/kinewright.desktop" "$stage_dir/kinewright.desktop"
cp "$repo_root/crates/kinewright-app/assets/kinewright-icon.png" "$stage_dir/kinewright.png"
cp "$repo_root/packaging/linux/README.txt" "$stage_dir/README.txt"
mkdir -p "$stage_dir/scripts" "$stage_dir/packaging/linux/apparmor"
install -m 755 "$repo_root/scripts/install-apparmor-profile.sh" "$stage_dir/scripts/"
cp "$repo_root/packaging/linux/apparmor/kinewright-media-helper.in" "$stage_dir/packaging/linux/apparmor/"

tarball_name="Kinewright-$version-linux-x64.tar.gz"
tarball_path="$output_dir/$tarball_name"

if [[ "$stage_only" -eq 1 ]]; then
    echo "Staged Kinewright, its media sandbox, and FFmpeg in $stage_dir"
    echo 'Stage-only mode: tarball creation was skipped.'
    exit 0
fi

rm -f "$tarball_path"
tar -C "$(dirname "$stage_dir")" -czf "$tarball_path" "$(basename "$stage_dir")"
echo "Staged Kinewright, its media sandbox, and FFmpeg in $stage_dir"
echo "Tarball: $tarball_path"

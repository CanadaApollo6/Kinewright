#!/usr/bin/env bash
# Provision the pinned Linux FFmpeg 8.x shared GPL build into third_party/ffmpeg
# and export the environment ffmpeg-sys-next, tests, and the app need.
#
# Usage (same shell as cargo):
#   source scripts/setup-ffmpeg.sh
#
# Executing the script still downloads and verifies the archive, then prints
# the exports. Source it so cargo sees PKG_CONFIG_PATH, FFMPEG_DIR, and
# LD_LIBRARY_PATH in the current process — matching setup-ffmpeg.ps1.

script_path="${BASH_SOURCE[0]:-$0}"
script_dir="$(cd -- "$(dirname -- "$script_path")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
third_party="$repo_root/third_party"
ffmpeg_root="$third_party/ffmpeg"
archive="$third_party/ffmpeg.tar.xz"
sourced=0
if [[ "${BASH_SOURCE[0]:-}" != "$0" ]]; then
    sourced=1
fi
if [[ "$sourced" -eq 1 ]]; then
    _kinewright_setup_shopts="$(set +o)"
fi
set -euo pipefail

FFMPEG_URL='https://github.com/mifi/ffmpeg-builds/releases/download/8.0-1/ffmpeg-n8.0-latest-linux64-gpl-shared-8.0.tar.xz'
FFMPEG_SHA256='c201d31f5c8a3b169345101c63ca70f71442848a271bec4a16ca29a1876e5cb1'

fail() {
    echo "$*" >&2
    if [[ "$sourced" -eq 1 ]]; then
        return 1
    fi
    exit 1
}

if [[ "$(uname -s)" != "Linux" ]]; then
    fail 'scripts/setup-ffmpeg.sh provisions the Linux FFmpeg build. On Windows run scripts/setup-ffmpeg.ps1.'
    return 1 2>/dev/null || exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    fail 'Python 3 is required to download the pinned FFmpeg archive.'
    return 1 2>/dev/null || exit 1
fi
if ! command -v pkg-config >/dev/null 2>&1; then
    fail 'pkg-config is required. Run scripts/install-linux-deps.sh first.'
    return 1 2>/dev/null || exit 1
fi
if ! command -v tar >/dev/null 2>&1; then
    fail 'tar is required to extract the pinned FFmpeg archive.'
    return 1 2>/dev/null || exit 1
fi

mkdir -p "$third_party"
marker="$ffmpeg_root/.archive-sha256"
installed_hash=""
if [[ -f "$marker" ]]; then
    installed_hash="$(tr -d '[:space:]' < "$marker")"
fi

if [[ "$installed_hash" != "$FFMPEG_SHA256" ]]; then
    rm -rf "$ffmpeg_root"
    rm -f "$archive"
    echo 'Downloading pinned FFmpeg 8.0 Linux x64 shared GPL build...'
    python3 - "$FFMPEG_URL" "$archive" <<'PY'
import sys
import urllib.request

request = urllib.request.Request(sys.argv[1], headers={"User-Agent": "Kinewright-Linux"})
with urllib.request.urlopen(request, timeout=120) as source, open(sys.argv[2], "wb") as target:
    while chunk := source.read(1024 * 1024):
        target.write(chunk)
PY
    actual_hash="$(python3 - "$archive" <<'PY'
import hashlib, sys
hasher = hashlib.sha256()
with open(sys.argv[1], "rb") as archive:
    while chunk := archive.read(1024 * 1024):
        hasher.update(chunk)
print(hasher.hexdigest())
PY
)"
    if [[ "$actual_hash" != "$FFMPEG_SHA256" ]]; then
        fail "FFmpeg archive SHA-256 mismatch. Expected $FFMPEG_SHA256, got $actual_hash."
        return 1 2>/dev/null || exit 1
    fi
    mkdir -p "$ffmpeg_root"
    tar -xJf "$archive" -C "$ffmpeg_root" --strip-components=1
    printf '%s' "$FFMPEG_SHA256" > "$marker"
    rm -f "$archive"
fi

pkg_config_path="$ffmpeg_root/lib/pkgconfig"
ffmpeg_bin="$ffmpeg_root/bin"
ffmpeg_lib="$ffmpeg_root/lib"

for required in \
    "$ffmpeg_root/include/libavcodec/avcodec.h" \
    "$ffmpeg_lib/libavcodec.so" \
    "$ffmpeg_bin/ffmpeg" \
    "$pkg_config_path/libavcodec.pc"
do
    if [[ ! -e "$required" ]]; then
        fail "Provisioning did not produce required file: $required"
        return 1 2>/dev/null || exit 1
    fi
done

export PKG_CONFIG_PATH="$pkg_config_path${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export FFMPEG_DIR="$ffmpeg_root"
export PATH="$ffmpeg_bin${PATH:+:$PATH}"
export LD_LIBRARY_PATH="$ffmpeg_lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

if [[ -n "${GITHUB_ENV:-}" ]]; then
    {
        echo "PKG_CONFIG_PATH=$PKG_CONFIG_PATH"
        echo "FFMPEG_DIR=$FFMPEG_DIR"
        echo "LD_LIBRARY_PATH=$LD_LIBRARY_PATH"
        echo "PATH=$PATH"
    } >> "$GITHUB_ENV"
fi

codec_version="$(pkg-config --modversion libavcodec)"
ffmpeg_version="$("$ffmpeg_bin/ffmpeg" -version)"
ffmpeg_version="${ffmpeg_version%%$'\n'*}"

echo "FFmpeg root: $ffmpeg_root"
echo "libavcodec: $codec_version"
echo "$ffmpeg_version"
if [[ "$sourced" -eq 1 ]]; then
    echo 'FFmpeg build environment is active in this shell.'
    eval "${_kinewright_setup_shopts}"
    unset _kinewright_setup_shopts
else
    echo 'FFmpeg is provisioned. Source this script so cargo sees the environment:'
    echo "  source $script_path"
fi

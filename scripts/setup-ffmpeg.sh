#!/usr/bin/env bash
# Provision the pinned Linux FFmpeg 8.x shared GPL build into third_party/ffmpeg
# and export the environment ffmpeg-sys-next, tests, and the app need.
#
# Usage (same shell as cargo):
#   source scripts/setup-ffmpeg.sh
#
# Executing the script still downloads and verifies the archive, then prints
# the exports. Source it so cargo sees PKG_CONFIG_PATH, FFMPEG_DIR, RUSTFLAGS,
# and LD_LIBRARY_PATH in the current process — matching setup-ffmpeg.ps1.

script_path="${BASH_SOURCE[0]:-$0}"
script_dir="$(cd -- "$(dirname -- "$script_path")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
third_party="$repo_root/third_party"
ffmpeg_root="$third_party/ffmpeg"
sourced=0
if [[ "${BASH_SOURCE[0]:-}" != "$0" ]]; then
    sourced=1
fi
if [[ "$sourced" -eq 1 ]]; then
    # `$(set +o)` runs in a command-substitution subshell, and bash clears
    # `errexit` there, so the snapshot always read `set +o errexit` and
    # restoring it at the end silently turned `-e` OFF in the caller's shell.
    # That is how both CI jobs reported success while `cargo test --workspace`
    # failed: GitHub runs a `run:` block as `bash -e`, this script switched it
    # off, and the step's status became the last command's. `$-` is expanded by
    # the current shell, so it is an accurate snapshot.
    _kinewright_setup_flags="$-"
    _kinewright_setup_pipefail=off
    if [[ -o pipefail ]]; then
        _kinewright_setup_pipefail=on
    fi
fi
set -euo pipefail

FFMPEG_URL='https://github.com/mifi/ffmpeg-builds/releases/download/8.0-1/ffmpeg-n8.0-latest-linux64-gpl-shared-8.0.tar.xz'
FFMPEG_SHA256='c201d31f5c8a3b169345101c63ca70f71442848a271bec4a16ca29a1876e5cb1'
archive="${KINEWRIGHT_FFMPEG_ARCHIVE_CACHE:-$third_party/ffmpeg-archives}/$FFMPEG_SHA256.tar.xz"

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
    nice -n 19 python3 -I "$script_dir/ffmpeg_archive.py" "$FFMPEG_URL" "$archive" "$FFMPEG_SHA256"
    rm -rf "$ffmpeg_root"
    mkdir -p "$ffmpeg_root"
    tar -xJf "$archive" -C "$ffmpeg_root" --strip-components=1
    printf '%s' "$FFMPEG_SHA256" > "$marker"
elif [[ -f "$archive" ]]; then
    nice -n 19 python3 -I "$script_dir/ffmpeg_archive.py" "$FFMPEG_URL" "$archive" "$FFMPEG_SHA256"
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

source "$script_dir/setup-nemo-speech.sh" || { return 1 2>/dev/null || exit 1; }

# Native crates such as alsa-sys can publish /usr/lib as a global Cargo linker
# search path. On rolling distributions that ship a newer FFmpeg ABI, that path
# may otherwise win over ffmpeg-sys-next's pinned search path even though its
# headers and pkg-config metadata came from FFMPEG_DIR. Put the pinned directory
# first in rustc's native search order so one binary cannot mix those ABIs.
ffmpeg_rust_link_flag="-Lnative=$ffmpeg_lib"
case " ${RUSTFLAGS:-} " in
    *" $ffmpeg_rust_link_flag "*) ;;
    *) export RUSTFLAGS="$ffmpeg_rust_link_flag${RUSTFLAGS:+ $RUSTFLAGS}" ;;
esac

if [[ -n "${GITHUB_ENV:-}" ]]; then
    {
        echo "PKG_CONFIG_PATH=$PKG_CONFIG_PATH"
        echo "FFMPEG_DIR=$FFMPEG_DIR"
        echo "KINEWRIGHT_NEMO_DIR=$KINEWRIGHT_NEMO_DIR"
        echo "LD_LIBRARY_PATH=$LD_LIBRARY_PATH"
        echo "RUSTFLAGS=$RUSTFLAGS"
        echo "PATH=$PATH"
    } >> "$GITHUB_ENV"
fi

codec_version="$(pkg-config --modversion libavcodec)"
case "$codec_version" in
    62.*) ;;
    *)
        fail "Expected the pinned FFmpeg 8 libavcodec ABI 62, but pkg-config resolved $codec_version."
        return 1 2>/dev/null || exit 1
        ;;
esac
ffmpeg_version="$("$ffmpeg_bin/ffmpeg" -version)"
ffmpeg_version="${ffmpeg_version%%$'\n'*}"

echo "FFmpeg root: $ffmpeg_root"
echo "libavcodec: $codec_version"
echo "$ffmpeg_version"
if [[ "$sourced" -eq 1 ]]; then
    echo 'FFmpeg build environment is active in this shell.'
    case "$_kinewright_setup_flags" in
        *e*) set -e ;;
        *) set +e ;;
    esac
    case "$_kinewright_setup_flags" in
        *u*) set -u ;;
        *) set +u ;;
    esac
    if [[ "$_kinewright_setup_pipefail" == on ]]; then
        set -o pipefail
    else
        set +o pipefail
    fi
    unset _kinewright_setup_flags _kinewright_setup_pipefail
else
    echo 'FFmpeg is provisioned. Source this script so cargo sees the environment:'
    echo "  source $script_path"
fi

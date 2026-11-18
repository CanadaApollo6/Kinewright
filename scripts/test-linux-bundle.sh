#!/usr/bin/env bash
# Smoke-test a Linux bundle: loader resolution, confined admission, and GUI startup.
set -euo pipefail

usage() {
    echo "Usage: $0 --bundle-dir DIR [--scratch-dir DIR] [--admission-only] [--startup-timeout-seconds N]" >&2
    exit 2
}

bundle_dir=""
scratch_parent=""
startup_timeout_seconds=8
admission_only=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bundle-dir)
            [[ -n "${2:-}" && "$2" != --* ]] || usage
            bundle_dir="$2"
            shift 2
            ;;
        --scratch-dir)
            [[ -n "${2:-}" && "$2" != --* ]] || usage
            scratch_parent="$2"
            shift 2
            ;;
        --startup-timeout-seconds)
            [[ -n "${2:-}" && "$2" != --* ]] || usage
            startup_timeout_seconds="$2"
            shift 2
            ;;
        --admission-only)
            admission_only=1
            shift
            ;;
        *)
            usage
            ;;
    esac
done

if [[ -z "$bundle_dir" ]]; then
    usage
fi
if [[ ! "$startup_timeout_seconds" =~ ^[0-9]+$ ]] || [[ "$startup_timeout_seconds" -lt 3 ]] || [[ "$startup_timeout_seconds" -gt 30 ]]; then
    echo 'startup timeout must be an integer between 3 and 30 seconds' >&2
    exit 1
fi

bundle_dir="$(cd -- "$bundle_dir" && pwd -P)"
scratch_parent="$(cd -- "${scratch_parent:-$(dirname -- "$bundle_dir")}" && pwd -P)"
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"

shopt -s nullglob
for library in libavcodec.so libavformat.so libavutil.so libswresample.so libswscale.so libnemo_speech_asr.so libnemo_speech_asr_c.so; do
    matches=("$bundle_dir/lib/$library"*)
    if [[ "${#matches[@]}" -eq 0 ]]; then
        echo "Bundle is missing required library: $library" >&2
        exit 1
    fi
done
shopt -u nullglob
ffmpeg_dependency='^[[:space:]]*lib(avcodec|avdevice|avfilter|avformat|avutil|swresample|swscale|postproc)\.so[^[:space:]]*[[:space:]]+=>[[:space:]]+(.*[^[:space:]])[[:space:]]+\(0x[[:xdigit:]]+\)$'
for binary in Kinewright kinewright-media-helper kinewright-media-supervisor ffmpeg; do
    executable="$bundle_dir/$binary"
    if [[ ! -f "$executable" || ! -x "$executable" ]]; then
        echo "Bundle executable is missing or not executable: $executable" >&2
        exit 1
    fi
    dependencies="$(env -i PATH=/usr/bin:/bin LC_ALL=C ldd "$executable")"
    if [[ "$dependencies" == *'=> not found'* ]]; then
        printf '%s has unresolved libraries:\n%s\n' "$binary" "$dependencies" >&2
        exit 1
    fi
    ffmpeg_found=0
    while IFS= read -r line; do
        if [[ "$line" =~ $ffmpeg_dependency ]]; then
            resolved="$(realpath -e -- "${BASH_REMATCH[2]}")"
            if [[ "${resolved#"$bundle_dir"/lib/}" == "$resolved" ]]; then
                echo "$binary resolves FFmpeg outside the bundle: $resolved" >&2
                exit 1
            fi
            ffmpeg_found=1
        fi
    done <<< "$dependencies"
    case "$binary:$ffmpeg_found" in
        Kinewright:0|kinewright-media-helper:0)
            echo "$binary did not resolve any FFmpeg libraries." >&2
            exit 1
            ;;
        kinewright-media-supervisor:1)
            echo 'The media supervisor unexpectedly links FFmpeg.' >&2
            exit 1
            ;;
    esac
done
echo 'App and helper resolve their FFmpeg libraries from the bundle. The supervisor has no FFmpeg dependency.'

smoke_dir="$(mktemp -d "$scratch_parent/Kinewright-bundle-smoke-XXXXXX")"
trap 'rm -rf -- "$smoke_dir"' EXIT
smoke_env=(
    env -i HOME="$smoke_dir" TMPDIR="$smoke_dir" LOCALAPPDATA="$smoke_dir"
    XDG_STATE_HOME="$smoke_dir" PATH=/usr/bin:/bin
)
# Preserve the user bus so the bundled supervisor can manage its resource scope.
for variable in XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS; do
    if [[ -v "$variable" ]]; then
        smoke_env+=("$variable=${!variable}")
    fi
done
fixture="$repo_root/crates/kinewright-agent/tests/fixtures/te1/talking-head.wav"
cd -- "$smoke_dir"
session_dir="$smoke_dir/session"
mkdir -- "$session_dir"
# A file cannot host the probe's mount root. The diagnostic must use SESSION_DIR.
: > "$smoke_dir/tmp-is-file"
admission="$("${smoke_env[@]}" TMPDIR="$smoke_dir/tmp-is-file" \
    "$bundle_dir/Kinewright" --check-bundle-admission "$fixture" "$session_dir")"
if [[ "$admission" != "Admitted ${fixture##*/}" ]]; then
    echo "Kinewright did not confirm admission of ${fixture##*/}: $admission" >&2
    exit 1
fi
unexpected="$(find "$smoke_dir" -mindepth 1 -maxdepth 1 ! -name session ! -name tmp-is-file -print -quit)"
if [[ -n "$unexpected" ]]; then
    echo "Bundle admission wrote outside SESSION_DIR: $unexpected" >&2
    exit 1
fi
printf '%s\n' "$admission"
if [[ "$admission_only" -eq 1 ]]; then
    exit 0
fi

lavapipe=(/usr/share/vulkan/icd.d/lvp_icd*.json)
if [[ ! -f "${lavapipe[0]}" ]] || ! command -v xvfb-run >/dev/null 2>&1; then
    echo 'The GUI smoke test requires xvfb-run and the Mesa lavapipe Vulkan driver.' >&2
    exit 1
fi
status=0
"${smoke_env[@]}" VK_DRIVER_FILES="${lavapipe[0]}" LIBGL_ALWAYS_SOFTWARE=1 \
    xvfb-run -a -s '-screen 0 1280x720x24' timeout --kill-after=3s "$startup_timeout_seconds" \
    "$bundle_dir/Kinewright" || status=$?
if [[ "$status" -ne 124 ]]; then
    echo "Kinewright exited during the startup smoke window." >&2
    exit 1
fi

echo "Kinewright stayed running for ${startup_timeout_seconds} seconds under Xvfb with lavapipe."

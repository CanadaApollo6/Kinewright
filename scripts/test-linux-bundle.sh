#!/usr/bin/env bash
# Smoke-test a staged Linux bundle: layout, bundled FFmpeg, and GUI startup.
set -euo pipefail

usage() {
    echo "Usage: $0 --bundle-dir DIR [--startup-timeout-seconds N]" >&2
    exit 2
}

bundle_dir=""
startup_timeout_seconds=8

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bundle-dir)
            bundle_dir="${2:-}"
            shift 2
            ;;
        --startup-timeout-seconds)
            startup_timeout_seconds="${2:-}"
            shift 2
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

bundle_dir="$(cd -- "$bundle_dir" && pwd)"
source_bin="$bundle_dir/OpenReel"
if [[ ! -f "$source_bin" ]]; then
    echo "Staged executable not found: $source_bin" >&2
    exit 1
fi

for pattern in libavcodec.so libavformat.so libavutil.so libswresample.so libswscale.so; do
    if ! compgen -G "$bundle_dir/lib/${pattern}*" >/dev/null; then
        echo "Bundle is missing required library: $pattern" >&2
        exit 1
    fi
done
if [[ ! -f "$bundle_dir/ffmpeg" ]]; then
    echo 'Bundle is missing the FFmpeg CLI used for in-editor recording.' >&2
    exit 1
fi

smoke_dir="$(mktemp -d "${TMPDIR:-/tmp}/OpenReel-bundle-smoke-XXXXXX")"
cleanup() {
    if [[ -n "${smoke_pid:-}" ]] && kill -0 "$smoke_pid" 2>/dev/null; then
        kill "$smoke_pid" 2>/dev/null || true
        wait "$smoke_pid" 2>/dev/null || true
    fi
    rm -rf "$smoke_dir"
}
trap cleanup EXIT

cp -a "$bundle_dir/." "$smoke_dir/"
smoke_bin="$smoke_dir/OpenReel"

run_smoke() {
    env -i \
        HOME="$HOME" \
        DISPLAY="${DISPLAY:-}" \
        XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}" \
        WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-}" \
        XDG_SESSION_TYPE="${XDG_SESSION_TYPE:-}" \
        PATH="/usr/bin:/bin" \
        "$smoke_bin"
}

if command -v xvfb-run >/dev/null 2>&1 && [[ -z "${DISPLAY:-}" ]]; then
    xvfb-run -a -s '-screen 0 1280x720x24' env -i HOME="$HOME" PATH="/usr/bin:/bin" "$smoke_bin" &
    smoke_pid=$!
else
    run_smoke &
    smoke_pid=$!
fi

sleep "$startup_timeout_seconds"
if ! kill -0 "$smoke_pid" 2>/dev/null; then
    wait "$smoke_pid" || true
    echo "OpenReel exited during the startup smoke window." >&2
    exit 1
fi

echo "OpenReel stayed running for ${startup_timeout_seconds} seconds from an isolated directory with a system-only PATH."

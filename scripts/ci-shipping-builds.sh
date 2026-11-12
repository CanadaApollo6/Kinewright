#!/usr/bin/env bash
# Isolated shipping matrix. Invoke without an outer heavy-cargo lock.
# KINEWRIGHT_SHIPPING_TARGET selects an owned directory under target/.
# KINEWRIGHT_KEEP_SHIPPING_TARGET=1 retains it after success; failures retain it.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# -gt 1 || ( $# -eq 1 && ${1:-} != --windows-check ) ]]; then
    echo 'Usage: ci-shipping-builds.sh [--windows-check]' >&2
    exit 2
fi
if [[ ${KINEWRIGHT_SHIPPING_LEASE:-} != held ]]; then
    exec flock -o /tmp/kinewright-cargo-heavy.lock \
        env CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 KINEWRIGHT_SHIPPING_LEASE=held \
        nice -n 10 bash "$0" "$@"
fi
if [[ ${1:-} == --windows-check ]]; then
    shipping_default=target/shipping-windows
    shipping_cargo=(cargo xwin check --target x86_64-pc-windows-msvc)
else
    source scripts/setup-ffmpeg.sh
    shipping_default=target/shipping
    shipping_cargo=(cargo build)
fi
shipping_target=$(realpath -m -- "${KINEWRIGHT_SHIPPING_TARGET:-$shipping_default}")
case "$shipping_target" in
    "$PWD"/target/*) ;;
    *) echo 'Shipping target must be a dedicated directory under this worktree target/.' >&2; exit 2 ;;
esac
shipping_marker="$shipping_target/.kinewright-shipping-target"
if [[ -e "$shipping_target" ]]; then
    if [[ ! -f "$shipping_marker" || $(cat "$shipping_marker") != "$PWD" ]]; then
        echo 'Refusing to use a target directory not owned by this shipping gate.' >&2
        exit 2
    fi
else
    mkdir -p -- "$shipping_target"
    printf '%s\n' "$PWD" > "$shipping_marker"
fi
export CARGO_TARGET_DIR="$shipping_target"
finish_shipping() {
    local result=$?
    du -sh -- "$shipping_target"
    if [[ $result == 0 && ${KINEWRIGHT_KEEP_SHIPPING_TARGET:-0} != 1 ]]; then
        rm -rf -- "$shipping_target"
        echo 'Removed successful shipping target.'
    fi
    return "$result"
}
trap finish_shipping EXIT
echo "Shipping target: $shipping_target"
"${shipping_cargo[@]}" -j4 -p kinewright-agent --no-default-features --lib
"${shipping_cargo[@]}" -j4 -p kinewright-app --no-default-features --bin kinewright-app
"${shipping_cargo[@]}" -j4 -p kinewright-agent --no-default-features --bin kinewright

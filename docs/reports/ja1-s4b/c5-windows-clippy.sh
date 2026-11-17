#!/usr/bin/env bash
# C5 fix 12: the Windows clippy gate, run from Linux.
#
# It lints the whole workspace for x86_64-pc-windows-msvc with cargo-xwin, the
# same `cargo clippy --workspace --all-targets -- -D warnings` the Windows CI
# lane runs, plus Agent slow-tests and App default/slow-tests. It replaces
# docs/reports/ja1-s4/c2-windows-check.sh as the C5 Windows gate: that script
# lints only the guardian modules (job_guardian, guardian, execution_claim) in
# an isolated manifest, so no off-Linux arm elsewhere in the crate is linted.
#
# Needs cargo-xwin (its SDK cache in ~/.cache/cargo-xwin) and the
# x86_64-pc-windows-msvc target, and llvm-rc. The FFmpeg headers and import libraries
# come from a prebuilt FFmpeg 9.0.2 MSVC archive pinned here by SHA-256. It is a
# lint-only header source on this Linux desktop; setup, CI and packaging build
# FFmpeg from verified source (docs/MS1-MINIMAL-FFMPEG.md) and never use it.
# This is a compile and lint check, not Windows execution.
set -euo pipefail
cd "$(dirname "$0")/../../.."
printf 'Windows cross-clippy gate, HEAD %s\n' "$(git rev-parse HEAD)"
# Match CI's three feature-disabled builds on this Linux host before linting
# MSVC. This helper owns its lock; never wrap this script in another flock.
CARGO_TARGET_DIR="$PWD/target/shipping" bash scripts/ci-shipping-builds.sh
# Cross-clippy does not enumerate MSVC libtest names. Keep the platform-scoped
# manifest/accounting regression in this per-commit gate as well.
nice -n 19 python3 -I scripts/ci_shards_selftest.py
url='https://github.com/System233/ffmpeg-msvc-prebuilt/releases/download/ffmpeg-9.0.2/ffmpeg-9.0.2_x64-windows-shared-gpl.zip'
sha='ce3a3a052c9dfe5f941789e88dba0d14d10fbf9be6236cb0c4b71a2dfc0a00e5'
cache="$HOME/.cache/kinewright-winxbuild"
if [ "$(cat "$cache/ffmpeg/.archive-sha256" 2>/dev/null)" != "$sha" ]; then
    mkdir -p "$cache/dl"
    curl -fsSL --retry 3 -o "$cache/dl/ffmpeg.zip.part" "$url"
    echo "$sha  $cache/dl/ffmpeg.zip.part" | sha256sum -c -
    mv "$cache/dl/ffmpeg.zip.part" "$cache/dl/ffmpeg.zip"
    rm -rf "$cache/ffmpeg"
    mkdir -p "$cache/ffmpeg"
    unzip -q "$cache/dl/ffmpeg.zip" -d "$cache/ffmpeg"
    echo "$sha" > "$cache/ffmpeg/.archive-sha256"
fi
export FFMPEG_DIR="$cache/ffmpeg"
export TMPDIR="$PWD/target/t-winclippy"
mkdir -p "$TMPDIR/bin"
# The App's build script compiles its icon resource with `rc.exe`; llvm-rc
# takes the same /nologo /fo arguments.
printf '#!/bin/sh\nexec llvm-rc "$@"\n' > "$TMPDIR/bin/rc.exe"
chmod +x "$TMPDIR/bin/rc.exe"
export PATH="$TMPDIR/bin:$PATH"
test "$(free -g | awk '/^Mem:/{print $7}')" -ge 6
run() {
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 19 \
        cargo xwin clippy "$@" --target x86_64-pc-windows-msvc --target-dir target/win-xwin -- -D warnings
}
run --workspace --all-targets
run -p kinewright-agent --all-targets --features slow-tests
run -p kinewright-app --all-targets
run -p kinewright-app --all-targets --features slow-tests
# Compile the same three MSVC shipping graphs in a disposable isolated target.
bash scripts/ci-shipping-builds.sh --windows-check
rm -rf "$TMPDIR"

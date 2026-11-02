#!/usr/bin/env bash
# C5 fix 12: the Windows clippy gate, run from Linux.
#
# It lints the whole workspace for x86_64-pc-windows-msvc with cargo-xwin, the
# same `cargo clippy --workspace --all-targets -- -D warnings` the Windows CI
# lane runs, plus the Agent crate with `slow-tests`. It replaces
# docs/reports/ja1-s4/c2-windows-check.sh as the C5 Windows gate: that script
# lints only the guardian modules (job_guardian, guardian, execution_claim) in
# an isolated manifest, so no off-Linux arm elsewhere in the crate is linted.
#
# Needs cargo-xwin (its SDK cache in ~/.cache/cargo-xwin) and the
# x86_64-pc-windows-msvc target, and llvm-rc. The FFmpeg headers and import libraries are
# the archive scripts/setup-ffmpeg.ps1 pins, checked against its SHA-256.
# This is a compile and lint check, not Windows execution.
set -euo pipefail
cd "$(dirname "$0")/../../.."
url=$(sed -n "s/^\$FfmpegUrl = '\(.*\)'$/\1/p" scripts/setup-ffmpeg.ps1)
sha=$(sed -n "s/^\$FfmpegSha256 = '\(.*\)'$/\1/p" scripts/setup-ffmpeg.ps1)
test -n "$url" && test -n "$sha"
cache="$HOME/.cache/kinewright-winxbuild"
if [ "$(cat "$cache/ffmpeg/.archive-sha256" 2>/dev/null)" != "$sha" ]; then
    mkdir -p "$cache/dl"
    curl -sSL --max-time 300 -o "$cache/dl/ffmpeg.zip" "$url"
    echo "$sha  $cache/dl/ffmpeg.zip" | sha256sum -c -
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
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 \
        cargo xwin clippy "$@" --target x86_64-pc-windows-msvc --target-dir target/win-xwin -- -D warnings
}
run --workspace --all-targets
run -p kinewright-agent --all-targets --features slow-tests
rm -rf "$TMPDIR"

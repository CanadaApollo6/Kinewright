#!/usr/bin/env bash
set -eu
ROOT=/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/target/ja1-s3-c6a/speech-tools
export LD_LIBRARY_PATH="$ROOT/usr/lib:${LD_LIBRARY_PATH:-}"
exec "$ROOT/usr/bin/espeak-ng" --path="$ROOT/usr/share" "$@"

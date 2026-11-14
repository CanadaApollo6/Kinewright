#!/usr/bin/env bash
# Sourced by setup-ffmpeg.sh after its environment is active.
_kw_setup_nemo() {
    local nemo_script_dir nemo_root
    nemo_script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
    nemo_root="$(nice -n 19 python3 -I "$nemo_script_dir/nemo_speech.py")" || return 1
    export KINEWRIGHT_NEMO_DIR="$nemo_root"
    export LD_LIBRARY_PATH="$nemo_root/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    echo "NeMo-Speech root: $nemo_root"
}
_kw_setup_nemo

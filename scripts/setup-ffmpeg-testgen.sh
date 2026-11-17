#!/usr/bin/env bash
# Test-only provisioning. Never source this from app or packaging setup.
python3 "$(dirname "${BASH_SOURCE[0]}")/build-ffmpeg.py" --profile testgen

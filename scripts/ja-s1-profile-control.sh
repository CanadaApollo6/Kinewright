#!/usr/bin/env bash
# Profile the frozen executable without editing it, its fixture or its deadlines.
set -euo pipefail
binary="$(realpath "$1")"
fixture="$(realpath "$2")"
output="$3"
profile="$4"
repo="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$profile/probes"
for tool in git rustc ffmpeg nvidia-smi uptime; do
    original="$(command -v "$tool" || true)"
    [[ -n "$original" ]] || continue
    # The only variable substituted into these wrappers is a resolved executable path.
    printf '#!/usr/bin/env bash\nstart=$(date +%%s%%N)\n%q "$@"\nresult=$?\nend=$(date +%%s%%N)\nprintf '\''{"probe":"%s","start_ns":%%s,"end_ns":%%s,"exit":%%s}\\n'\'' "$start" "$end" "$result" >> "$JA_S1_SETUP_PROBES"\nexit "$result"\n' "$original" "$tool" > "$profile/probes/$tool"
    chmod +x "$profile/probes/$tool"
done
export JA_S1_SETUP_PROBES="$profile/probes.jsonl"
PATH="$profile/probes:$PATH" nice -n 10 "$binary" --mode baseline --fixture "$fixture" --output "$output" > "$profile/stdout.log" 2> "$profile/stderr.log" &
fixture_pid=$!
nice -n 19 python3 -I "$repo/scripts/ja-s1-setup-profile.py" "$fixture_pid" "$binary" "$profile/processes.json.gz" "$output" > "$profile/timings.log" &
sampler_pid=$!
set +e
wait "$fixture_pid"
result=$?
wait "$sampler_pid"
sampler_result=$?
set -e
cat "$profile/stdout.log"
cat "$profile/stderr.log" >&2
cat "$profile/timings.log"
[[ "$sampler_result" -eq 0 ]] || exit 2
exit "$result"

#!/usr/bin/env bash
# GitHub-hosted Ubuntu only. The developer installer keeps its normal behaviour.
set -euo pipefail
script_dir=$(cd "$(dirname "$0")" && pwd)
cd "$script_dir/.."
[[ ${GITHUB_ACTIONS:-} == true && ${RUNNER_OS:-} == Linux ]] || {
    echo 'ci-linux-deps.sh requires a Linux GitHub Actions job.' >&2
    exit 2
}
cache=${KINEWRIGHT_APT_CACHE:-$HOME/apt-archives}
mirrors=${KINEWRIGHT_APT_MIRRORS:-/etc/apt/apt-mirrors.txt}
sources=${KINEWRIGHT_APT_SOURCES:-/etc/apt/sources.list.d/ubuntu.sources}
state=${RUNNER_TEMP:?}/kinewright-apt
archives=$state/archives
mkdir -p "$state" "$cache"

as_root() {
    if (( EUID == 0 )); then "$@"; else sudo -n "$@"; fi
}

if [[ ${1:-} != --install ]]; then
    # Bound probes, resolver, downloads and maintainer scripts together. GNU timeout
    # owns this child/process group; the outer shell survives to explain the deadline.
    if [[ -r $mirrors ]]; then
        cp "$mirrors" "$state/mirrors-used.txt"
    else
        printf 'unavailable mirror file %s\n' "$mirrors" > "$state/mirrors-used.txt"
    fi
    result=0
    as_root env GITHUB_ACTIONS=true RUNNER_OS=Linux RUNNER_TEMP="$RUNNER_TEMP" \
        KINEWRIGHT_APT_CACHE="$cache" KINEWRIGHT_APT_MIRRORS="$mirrors" \
        KINEWRIGHT_APT_SOURCES="$sources" KINEWRIGHT_APT_SUITE="${KINEWRIGHT_APT_SUITE:-}" \
        timeout --kill-after=10s "${KINEWRIGHT_APT_DEADLINE:-300}s" \
        bash "$script_dir/ci-linux-deps.sh" --install || result=$?
    if (( result != 0 )); then
        if (( result == 124 || result == 137 )); then
            reason="timed out after ${KINEWRIGHT_APT_DEADLINE:-300}s"
        else
            reason="failed with exit $result"
        fi
        echo "::error::Native dependency installation $reason; apt mirrors: $(tr '\n' ' ' < "$state/mirrors-used.txt")" >&2
    fi
    exit "$result"
fi

mapfile -t packages < <(bash scripts/install-linux-deps.sh --print-apt-packages)
installed() {
    [[ $(dpkg-query -W -f='${db:Status-Status} ${db:Status-Eflag}' "$1" 2>/dev/null || true) == 'installed ok' ]]
}
missing=()
for package in "${packages[@]}"; do
    installed "$package" || missing+=("$package")
done
if (( ${#missing[@]} == 0 )); then
    echo 'All native dependencies are installed; no apt or mirror access needed.'
    exit 0
fi

# Explicit local candidates also work when the image has removed its apt indexes.
# Pick one native/all version per package and never downgrade an installed package.
declare -A files=() versions=()
architecture=$(dpkg --print-architecture)
shopt -s nullglob
as_root mkdir -p "$archives"
cached_debs=("$cache/"*.deb)
if (( ${#cached_debs[@]} > 0 )); then
    as_root cp "${cached_debs[@]}" "$archives/"
fi
for deb in "$cache/"*.deb; do
    # dpkg expands its own field names, not shell variables.
    # shellcheck disable=SC2016
    read -r package version arch < <(dpkg-deb --show --showformat='${Package} ${Version} ${Architecture}\n' "$deb") || continue
    [[ $arch == "$architecture" || $arch == all ]] || continue
    if [[ -z ${versions[$package]:-} ]] || dpkg --compare-versions "$version" gt "${versions[$package]}"; then
        versions[$package]=$version
        files[$package]=$deb
    fi
done
complete=true
for package in "${missing[@]}"; do
    [[ -n ${files[$package]:-} ]] || complete=false
done
local_debs=()
for package in "${!files[@]}"; do
    if installed "$package" && dpkg --compare-versions "$(dpkg-query -W -f='${Version}' "$package")" ge "${versions[$package]}"; then
        continue
    fi
    local_debs+=("${files[$package]}")
done
apt_options=(-o "Dir::Cache::archives=$archives" -o APT::Keep-Downloaded-Packages=true
    -o Acquire::http::Timeout=5 -o Acquire::https::Timeout=5 -o Acquire::Retries=0)
if $complete && as_root env DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" \
    -o Dir::Etc::sourcelist=/dev/null -o Dir::Etc::sourceparts=- \
    --no-download --no-remove install -y --no-install-recommends "${local_debs[@]}"; then
    echo 'Native dependency closure installed entirely from cached debs; no mirror access needed.'
    exit 0
fi

echo 'Apt cache is partial; probe mirrors, refresh Ubuntu indexes, and fetch missing dependencies.'
grep -qF "mirror+file:$mirrors" "$sources" || {
    echo "Expected the runner Ubuntu sources to use mirror+file:$mirrors" >&2
    exit 1
}
# Only the disposable CI job's mirror list changes; no developer sources are edited.
# shellcheck source=/etc/os-release
suite=${KINEWRIGHT_APT_SUITE:-$(. /etc/os-release; printf '%s' "${VERSION_CODENAME:?}")}
python3 scripts/ci_apt_mirrors.py "$mirrors" "$state/mirrors-selected.txt" \
    --suite "$suite"
as_root cp "$state/mirrors-selected.txt" "$mirrors"
cp "$state/mirrors-selected.txt" "$state/mirrors-used.txt"
mkdir -p "$state/sources"
cp "$sources" "$state/sources/ubuntu.sources"
# These dependencies all come from Ubuntu; unrelated image PPAs are unnecessary.
apt_options+=(-o Dir::Etc::sourcelist=/dev/null -o "Dir::Etc::sourceparts=$state/sources")
as_root env DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" update --error-on=any
as_root env DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" \
    --no-remove install -y --no-install-recommends "${missing[@]}"
echo 'Linux CI native dependencies are installed.'

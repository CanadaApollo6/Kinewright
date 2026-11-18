#!/usr/bin/env bash
# Install or remove the AppArmor profile that lets Kinewright's media helper
# create its sandbox on Ubuntu's restricted user namespaces (MS2, ruling 31).
# Run as root:
#   install-apparmor-profile.sh install /absolute/path/to/kinewright-media-helper
#   install-apparmor-profile.sh remove
set -euo pipefail

profile=/etc/apparmor.d/kinewright-media-helper
template="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/packaging/linux/apparmor/kinewright-media-helper.in"

# 0: the profile is loaded. 1: confirmed not loaded, or AppArmor is not
# enabled. 2: AppArmor is enabled and the loaded list cannot be read.
profile_state() {
    local enabled=/sys/module/apparmor/parameters/enabled
    local loaded=/sys/kernel/security/apparmor/profiles
    local found=0
    if [[ ! -r "$enabled" ]] || [[ "$(<"$enabled")" != Y ]]; then
        return 1
    fi
    [[ -r "$loaded" ]] || return 2
    grep -q '^kinewright-media-helper (' "$loaded" || found=$?
    case "$found" in
    0 | 1) return "$found" ;;
    *) return 2 ;;
    esac
}

case "${1:-}" in
install)
    requested="${2:?usage: $0 install /absolute/path/to/kinewright-media-helper}"
    if [[ "$requested" != /* ]] || ! IFS= read -r -d '' helper < <(realpath -ez -- "$requested") ||
        [[ ! -f "$helper" || ! -x "$helper" ]]; then
        echo "helper must be an existing absolute path to a regular executable file: $requested" >&2
        exit 2
    fi
    # AppArmor reads *?[]{}^ in an attachment as globs; admit plain paths only.
    if [[ ! "$helper" =~ ^/[A-Za-z0-9._/+\ -]+$ ]]; then
        echo "resolved helper path may contain only [A-Za-z0-9._/+ -]: $helper" >&2
        exit 2
    fi
    staged="$(mktemp -- "${profile%/*}/.${profile##*/}.XXXXXX")"
    trap 'rm -f -- "$staged"' EXIT
    sed "s|@HELPER@|${helper}|" "$template" > "$staged"
    chmod 0644 -- "$staged"
    # Load the staged text first. If the parser rejects it, an earlier
    # installation keeps both its loaded profile and its file.
    apparmor_parser -r -K "$staged"
    mv -T -- "$staged" "$profile"
    echo "loaded $profile for $helper"
    ;;
remove)
    before=0
    profile_state || before=$?
    if [[ "$before" -ne 1 ]]; then
        # Loaded, or cannot tell: unload by profile name, which works even if
        # the file has already been deleted.
        printf '%s\n' 'profile kinewright-media-helper {}' | apparmor_parser -R || true
    fi
    rm -f -- "$profile"
    after=0
    profile_state || after=$?
    if [[ "$after" -eq 0 ]]; then
        echo "deleted $profile, but the profile is still loaded" >&2
        exit 1
    fi
    if [[ "$after" -eq 2 ]]; then
        echo "deleted $profile, but could not confirm that the profile is unloaded" >&2
        exit 1
    fi
    echo "removed $profile"
    ;;
*)
    echo "usage: $0 install /absolute/path/to/kinewright-media-helper | remove" >&2
    exit 2
    ;;
esac

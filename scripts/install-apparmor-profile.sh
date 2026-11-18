#!/usr/bin/env bash
# Install or remove the AppArmor profile that lets Kinewright's media helper
# create its sandbox on Ubuntu's restricted user namespaces (MS2, ruling 31).
# Run as root:
#   install-apparmor-profile.sh install /absolute/path/to/kinewright-media-helper
#   install-apparmor-profile.sh remove
set -euo pipefail

profile=/etc/apparmor.d/kinewright-media-helper
template="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/packaging/linux/apparmor/kinewright-media-helper.in"

profile_is_loaded() {
    grep -qs '^kinewright-media-helper (' /sys/kernel/security/apparmor/profiles
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
    mv -T -- "$staged" "$profile"
    if apparmor_parser -r "$profile"; then
        echo "loaded $profile for $helper"
    else
        parser_status=$?
        rm -f -- "$profile"
        exit "$parser_status"
    fi
    ;;
remove)
    unload_status=0
    if profile_is_loaded; then
        # Removal uses the profile name, even if the file has already been deleted.
        printf '%s\n' 'profile kinewright-media-helper {}' | apparmor_parser -R || unload_status=$?
    fi
    rm -f -- "$profile"
    if [[ "$unload_status" -ne 0 ]] && profile_is_loaded; then
        exit "$unload_status"
    fi
    echo "removed $profile"
    ;;
*)
    echo "usage: $0 install /absolute/path/to/kinewright-media-helper | remove" >&2
    exit 2
    ;;
esac

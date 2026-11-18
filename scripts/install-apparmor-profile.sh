#!/usr/bin/env bash
# Install or remove the AppArmor profile that lets Kinewright's media helper
# create its sandbox on Ubuntu's restricted user namespaces (MS2, ruling 31).
# Run as root:
#   install-apparmor-profile.sh install /absolute/path/to/kinewright-media-helper
#   install-apparmor-profile.sh remove
set -euo pipefail

profile=/etc/apparmor.d/kinewright-media-helper
template="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/packaging/linux/apparmor/kinewright-media-helper.in"

case "${1:-}" in
install)
    helper="${2:?usage: $0 install /absolute/path/to/kinewright-media-helper}"
    # AppArmor reads *?[]{}^ in an attachment as globs; admit plain paths only.
    if [[ ! "$helper" =~ ^/[A-Za-z0-9._/+\ -]+$ || ! -x "$helper" ]]; then
        echo "helper must be an existing absolute executable path of [A-Za-z0-9._/+ -]: $helper" >&2
        exit 2
    fi
    sed "s|@HELPER@|${helper}|" "$template" > "$profile.tmp"
    mv "$profile.tmp" "$profile"
    apparmor_parser -r "$profile"
    echo "loaded $profile for $helper"
    ;;
remove)
    if [[ -f "$profile" ]]; then
        apparmor_parser -R "$profile"
        rm "$profile"
    fi
    echo "removed $profile"
    ;;
*)
    echo "usage: $0 install /absolute/path/to/kinewright-media-helper | remove" >&2
    exit 2
    ;;
esac

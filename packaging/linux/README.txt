Kinewright for Linux x64

Extract the tarball and keep its files together. Run ./Kinewright from the
extracted directory. The app starts the media helper and supervisor next to
it, and the helper loads FFmpeg from lib/.

If AppArmor restricts unprivileged user namespaces on your system, run this
command from the extracted directory before importing owned media:

    sudo ./scripts/install-apparmor-profile.sh install "$(pwd -P)/kinewright-media-helper"

The profile is attached to the helper's absolute path. Run the command again
if you move the extracted directory.

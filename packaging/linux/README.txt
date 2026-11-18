Kinewright for Linux x64

Extract the tarball and keep its files together. Run ./Kinewright from the
extracted directory. The app starts the media helper and supervisor next to
it, and the helper loads FFmpeg from lib/.

If AppArmor restricts unprivileged user namespaces on your system, run this
command from the extracted directory before importing owned media:

    sudo ./scripts/install-apparmor-profile.sh install "$(pwd -P)/kinewright-media-helper"

The profile is attached to the helper's resolved absolute path. That path
may contain only letters A-Z and a-z, digits, spaces, and . _ / + -.
If the path contains other characters, move the extracted directory to a
location with an allowed path and run the command there.

There is one Kinewright profile per machine. If you install it from a second
copy, it points to that copy. Run the install command again if you move the
copy you use.

To remove the profile, run this command from the extracted directory:

    sudo ./scripts/install-apparmor-profile.sh remove

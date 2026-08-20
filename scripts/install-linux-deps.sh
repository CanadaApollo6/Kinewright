#!/usr/bin/env bash
# Install the Debian/Ubuntu or Arch-family packages required to build Kinewright.
# The pacman path supports Arch Linux, CachyOS, Omarchy, and their derivatives.
# Run from a fresh clone before scripts/setup-ffmpeg.sh.
set -euo pipefail

run_as_root() {
    if [[ "${EUID}" -eq 0 ]]; then
        "$@"
    else
        sudo "$@"
    fi
}

if command -v apt-get >/dev/null 2>&1; then
    packages=(
        build-essential
        g++
        libstdc++-13-dev
        libstdc++-14-dev
        cmake
        pkg-config
        python3
        xz-utils
        libasound2-dev
        libclang-dev
        libgtk-3-dev
        libvulkan-dev
        mesa-vulkan-drivers
        libx11-dev
        libxrandr-dev
        libxi-dev
        libxcursor-dev
        libxkbcommon-dev
        libxkbcommon-x11-dev
        libwayland-dev
        libegl1-mesa-dev
        patchelf
        xvfb
        x11-xserver-utils
    )

    run_as_root apt-get update
    if [[ "${EUID}" -eq 0 ]]; then
        DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${packages[@]}"
    else
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${packages[@]}"
    fi
elif command -v pacman >/dev/null 2>&1; then
    distro_name='an Arch-family system'
    if [[ -r /etc/os-release ]]; then
        # PRETTY_NAME is informational only; package-manager detection controls
        # the installation path.
        distro_name="$(. /etc/os-release && printf '%s' "${PRETTY_NAME:-$distro_name}")"
    fi
    echo "Installing Linux build dependencies for $distro_name."

    packages=(
        base-devel
        clang
        cmake
        pkgconf
        python
        xz
        alsa-lib
        gtk3
        vulkan-headers
        vulkan-icd-loader
        vulkan-swrast
        libx11
        libxrandr
        libxi
        libxcursor
        libxkbcommon
        libxkbcommon-x11
        wayland
        mesa
        patchelf
        xorg-server-xvfb
        xorg-xrandr
    )

    run_as_root pacman -Syu --needed --noconfirm "${packages[@]}"
else
    echo 'install-linux-deps.sh supports Debian/Ubuntu (apt-get) and Arch Linux/CachyOS/Omarchy (pacman).' >&2
    exit 1
fi

echo 'Linux build dependencies are installed.'

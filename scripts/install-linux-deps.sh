#!/usr/bin/env bash
# Install the Debian/Ubuntu packages required to build OpenReel on Linux.
# Run from a fresh clone before scripts/setup-ffmpeg.sh.
set -euo pipefail

if ! command -v apt-get >/dev/null 2>&1; then
    echo 'install-linux-deps.sh currently supports Debian/Ubuntu (apt-get).' >&2
    exit 1
fi

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

if [[ "${EUID}" -eq 0 ]]; then
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${packages[@]}"
else
    sudo apt-get update
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${packages[@]}"
fi

echo 'Linux build dependencies are installed.'

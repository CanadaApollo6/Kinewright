#!/usr/bin/env python3
"""Install the pinned nextest release, checking its archive before extraction."""
import hashlib
import io
import os
from pathlib import Path
import platform
import tarfile
import urllib.request

VERSION = "0.9.149"
ASSETS = {
    "Linux": ("x86_64-unknown-linux-gnu", "32d1d8c0567b7a80bee2bad8bb09125c846da96bc009b2de11ec08557290c770"),
    "Windows": ("x86_64-pc-windows-msvc", "8e7609fffb5a938c9b48d8788a89699fc83c39d07a9d9dbc61554f661b51d9d8"),
}
target, expected = ASSETS[platform.system()]
url = f"https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-{VERSION}/cargo-nextest-{VERSION}-{target}.tar.gz"
with urllib.request.urlopen(url, timeout=120) as response:
    data = response.read()
if hashlib.sha256(data).hexdigest() != expected:
    raise SystemExit("nextest archive SHA-256 mismatch")
destination = Path(os.environ["RUNNER_TEMP"]) / "nextest-bin"
destination.mkdir(exist_ok=True)
name = "cargo-nextest.exe" if platform.system() == "Windows" else "cargo-nextest"
with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
    matches = [m for m in archive.getmembers() if Path(m.name).name == name and m.isfile()]
    if len(matches) != 1:
        raise SystemExit("expected exactly one nextest executable")
    with archive.extractfile(matches[0]) as source:
        (destination / name).write_bytes(source.read())
(destination / name).chmod(0o755)
with open(os.environ["GITHUB_PATH"], "a", encoding="utf-8") as output:
    output.write(str(destination) + "\n")
print(f"Installed cargo-nextest {VERSION}; SHA-256 {expected}")

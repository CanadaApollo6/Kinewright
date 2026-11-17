#!/usr/bin/env python3
"""Enforce ruling 34 without removing the app's accessibility dependencies."""

import argparse
import fnmatch
from pathlib import Path
import sys
import tomllib

SUPERVISOR = "kinewright-media-supervisor"
MANAGER = "org.freedesktop.systemd1"
DEPENDENCIES = ("dependencies", "build-dependencies", "dev-dependencies")


def dependency_tables(manifest):
    for section in DEPENDENCIES:
        yield manifest.get(section, {})
    for target in manifest.get("target", {}).values():
        for section in DEPENDENCIES:
            yield target.get(section, {})


def check(root):
    with (root / "Cargo.toml").open("rb") as source:
        root_manifest = tomllib.load(source)
    workspace = root_manifest["workspace"]
    manifests = set()
    for pattern in workspace["members"]:
        for member in root.glob(pattern):
            relative = member.relative_to(root).as_posix()
            if not any(fnmatch.fnmatchcase(relative, p) for p in workspace.get("exclude", [])):
                manifests.add(member / "Cargo.toml")
    if "package" in root_manifest:
        manifests.add(root / "Cargo.toml")
    if not manifests:
        raise ValueError("workspace has no members")
    shared = workspace.get("dependencies", {})
    errors = []
    pending = sorted(manifests)
    while pending:
        path = pending.pop()
        with path.open("rb") as source:
            manifest = tomllib.load(source)
        name = manifest["package"]["name"]
        for table in dependency_tables(manifest):
            for alias, specification in table.items():
                specification = specification if isinstance(specification, dict) else {}
                dependency_base = path.parent
                if specification.get("workspace"):
                    specification = shared[alias]
                    specification = specification if isinstance(specification, dict) else {}
                    dependency_base = root
                if specification.get("package", alias) == "zbus" and name != SUPERVISOR:
                    errors.append(f"{path.relative_to(root)}: {name} declares direct zbus ({alias})")
                # Cargo also makes in-workspace path dependencies implicit members.
                if "path" in specification:
                    directory = (dependency_base / specification["path"]).resolve()
                    member = directory / "Cargo.toml"
                    if directory.is_relative_to(root) and member not in manifests:
                        relative = directory.relative_to(root).as_posix()
                        if not any(fnmatch.fnmatchcase(relative, p) for p in workspace.get("exclude", [])):
                            with member.open("rb") as source:
                                dependency_manifest = tomllib.load(source)
                            if "workspace" not in dependency_manifest:
                                manifests.add(member)
                                pending.append(member)
        if name == "kinewright-app" or name.startswith("kinewright-app-") or name.endswith("-app"):
            for source in sorted(path.parent.rglob("*.rs")):
                for number, line in enumerate(source.read_text(encoding="utf-8").splitlines(), 1):
                    if MANAGER in line:
                        errors.append(f"{source.relative_to(root)}:{number}: app mentions {MANAGER}")
    with (root / "Cargo.lock").open("rb") as source:
        packages = tomllib.load(source)["package"]
    copies = [p["version"] for p in packages if p["name"] == "zbus"]
    if len(copies) != 1:
        errors.append(f"Cargo.lock: expected one zbus package, found {copies}")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    root = parser.parse_args().root.resolve()
    try:
        errors = check(root)
    except (OSError, ValueError, KeyError) as error:
        print(f"MS2 client boundary: {error}", file=sys.stderr)
        return 1
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("MS2 client boundary: supervisor-only direct zbus, no app systemd client, one locked zbus")
    return 0


if __name__ == "__main__":
    sys.exit(main())

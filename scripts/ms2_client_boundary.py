#!/usr/bin/env python3
"""Check the MS2 client dependency boundary and dev-only binary override."""

import argparse
import fnmatch
import json
from pathlib import Path
import subprocess
import sys
import tomllib

SUPERVISOR = "kinewright-media-supervisor"
MANAGER = "org.freedesktop.systemd1"
DEPENDENCIES = ("dependencies", "build-dependencies", "dev-dependencies")
PROJECT = "kinewright-project"
TEST_BIN_DIR = "ms2-test-bin-dir"


def check_test_bin_dir(root):
    # Inspect Cargo's normalized aliases and inherited dependencies for all
    # targets, even optional paths that aren't in today's default builds.
    cargo = subprocess.run(
        ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"],
        cwd=root, capture_output=True, text=True, check=False,
    )
    if cargo.returncode:
        raise ValueError(f"cargo metadata: {cargo.stderr.strip()}")
    for package in json.loads(cargo.stdout)["packages"]:
        name = package["name"]
        project_aliases = set()
        for dependency in package["dependencies"]:
            if dependency["name"] != PROJECT:
                continue
            alias = dependency["rename"] or PROJECT
            project_aliases.add(alias)
            if dependency["kind"] != "dev" and TEST_BIN_DIR in dependency["features"]:
                yield f"{name} dependency {alias} enables {TEST_BIN_DIR}"
        for feature, entries in package["features"].items():
            for entry in entries:
                alias, slash, target = entry.partition("/")
                if ((name == PROJECT and entry == TEST_BIN_DIR)
                        or (slash and target == TEST_BIN_DIR
                            and alias.removesuffix("?") in project_aliases)):
                    yield f"{name} feature {feature} enables {TEST_BIN_DIR}"


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
    if workspace.get("resolver") not in {"2", "3"}:
        errors.append("Cargo.toml: MS2 test override requires a resolver that isolates dev features")
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
                    errors.append(f"{path.relative_to(root).as_posix()}: {name} declares direct zbus ({alias})")
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
                        errors.append(f"{source.relative_to(root).as_posix()}:{number}: app mentions {MANAGER}")
    with (root / "Cargo.lock").open("rb") as source:
        packages = tomllib.load(source)["package"]
    copies = [p["version"] for p in packages if p["name"] == "zbus"]
    if len(copies) != 1:
        errors.append(f"Cargo.lock: expected one zbus package, found {copies}")
    errors.extend(check_test_bin_dir(root))
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
    print("MS2 client boundary: supervisor-only direct zbus, no app systemd client, "
          "one locked zbus, dev-only test binary override")
    return 0


if __name__ == "__main__":
    sys.exit(main())

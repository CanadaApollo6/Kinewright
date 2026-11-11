#!/usr/bin/env python3
"""Build and extract test archives."""
import argparse
import hashlib
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tarfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci_shards
import ci_display_inputs
import slow_tests as st

ROOT = Path(__file__).resolve().parent.parent
CARGO_CONFIG = ["--config", str(ROOT / ".cargo" / "ci-test.toml")]


def run(args, output=None):
    print("+ " + " ".join(map(str, args)), flush=True)
    started = time.monotonic()
    if output:
        with Path(output).open("w", encoding="utf-8") as log:
            subprocess.run(args, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                           check=True, encoding="utf-8")
    else:
        subprocess.run(args, cwd=ROOT, check=True)
    print(f"command wall seconds: {time.monotonic() - started:.3f}", flush=True)


def fetch_locked(features):
    host = next(line.removeprefix("host: ") for line in
                subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    args = ["cargo", "metadata", *CARGO_CONFIG, "--locked", "--filter-platform", host, "--format-version", "1", *features]
    cached = subprocess.run([*args, "--offline"], cwd=ROOT, capture_output=True, text=True)
    if cached.returncode:
        print("Dependency cache incomplete; resolve the locked workspace dependencies online.", flush=True)
        run(args, os.devnull)
    else:
        print("Locked workspace dependencies are cached; build without an index refresh.", flush=True)


def build(tier, count):
    directory = ROOT / "ci-artifacts"
    directory.mkdir(exist_ok=True)
    archive = directory / "tests.tar.zst"
    features = ["--features", st.feature_list(st.read_manifest())] if tier == "slow" else []
    fetch_locked(features)
    run(["cargo", "nextest", "archive", *CARGO_CONFIG, "--timings", "--frozen", "--workspace", *features,
         "--archive-file", str(archive)])
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    (directory / "tests.sha256").write_text(digest + "\n")
    listing = subprocess.check_output(["cargo", "nextest", "list", *CARGO_CONFIG, "--frozen", "--workspace",
                                       *features, "--message-format", "json"], cwd=ROOT, text=True)
    compiled = json.loads(listing)
    ci_shards.save(directory / "compiled-list.json", compiled)
    data = ci_shards.plan(compiled, tier, count)
    ci_shards.save(directory / "ci-plan.json", data)
    print(f"compiled inventory: {sum(len(s['tests']) for s in data['suites'])}; estimated shard seconds {data['estimated_seconds']}")
    for timing in (ROOT / "target" / "cargo-timings").glob("*.html"):
        shutil.copyfile(timing, directory / timing.name)
    if tier == "fast" and st.host_os() == "linux":
        pack_compiler_inputs(directory)
    if tier == "fast":
        run(["cargo", "test", *CARGO_CONFIG, "--frozen", "--workspace", "--doc", "--", "--list"], directory / "doc-list.log")
        docs = ci_shards.doc_inventory((directory / "doc-list.log").read_text(encoding="utf-8"))
        ci_shards.save(directory / "doc-list.json", docs)
        run(["cargo", "test", *CARGO_CONFIG, "--frozen", "--workspace", "--doc"], directory / "docs.log")
        st.verify_exact_rows(docs, st.parse((directory / "docs.log").read_text(encoding="utf-8")))
        if st.host_os() == "linux":
            dynamic = subprocess.check_output(["readelf", "-d", "target/debug/kinewright-eval"], text=True)
            if "[libavcodec.so.62]" not in dynamic:
                raise ValueError("kinewright-eval does not link pinned libavcodec.so.62")
            print(dynamic)


def pack_compiler_inputs(directory):
    archive = directory / "compiler-inputs.tar.gz"
    debug = ROOT / "target" / "debug"
    fingerprints = debug / ".fingerprint"
    libraries = sorted(p for p in (debug / "deps").iterdir() if p.suffix in (".rlib", ".rmeta", ".so", ".dll", ".lib", ".a"))
    if not fingerprints.is_dir() or not libraries:
        raise ValueError("compiler fixture inputs were not built")
    with tarfile.open(archive, "w:gz", compresslevel=1) as output:
        for path in [fingerprints, *libraries]:
            output.add(path, arcname=path.relative_to(ROOT))
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    archive.with_suffix(".sha256").write_text(digest + "\n")


def restore_test_environment(directory, units):
    (ROOT / "target" / "tmp").mkdir(parents=True, exist_ok=True)
    if any(u.get("compiler_inputs", False) for u in units):
        archive = directory / "compiler-inputs.tar.gz"
        with archive.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        if digest != archive.with_suffix(".sha256").read_text().strip():
            raise ValueError("compiler inputs SHA-256 mismatch")
        with tarfile.open(archive) as source:
            source.extractall(ROOT, filter="data")


def extract(index=None):
    directory = ROOT / "ci-artifacts"
    archive = directory / "tests.tar.zst"
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    if digest != (directory / "tests.sha256").read_text().strip():
        raise ValueError("test archive SHA-256 mismatch")
    with (directory / "extracted-list.json").open("w", encoding="utf-8") as listing:
        subprocess.run(["cargo", "nextest", "list", "--archive-file", str(archive),
                        "--extract-to", str(ROOT), "--extract-overwrite", "--workspace-remap", str(ROOT),
                        "--message-format", "json"], cwd=ROOT, stdout=listing, check=True)
    data = ci_shards.load(directory / "ci-plan.json")
    actual = ci_shards.inventory(ci_shards.load(directory / "extracted-list.json"), data["tier"])
    if actual != data["suites"]:
        raise ValueError("extracted test paths or inventory differ from builder")
    units = data["shards"][index] if index is not None else [u for shard in data["shards"] for u in shard]
    restore_test_environment(directory, units)
    if index is not None:
        ci_display_inputs.prepare(data, units, ROOT / "ci-results")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["build", "extract"])
    parser.add_argument("--tier", choices=["fast", "slow"])
    parser.add_argument("--count", type=int)
    parser.add_argument("--index", type=int)
    args = parser.parse_args()
    os.environ["CARGO_TERM_COLOR"] = "never"
    if args.command == "build":
        build(args.tier, args.count)
    elif args.command == "extract":
        extract(args.index)

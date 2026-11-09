#!/usr/bin/env python3
"""Build/extract test archives and transfer the sealed predecessor prerequisite."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci_shards
import slow_tests as st

ROOT = Path(__file__).resolve().parent.parent


def run(args, output=None):
    print("+ " + " ".join(map(str, args)), flush=True)
    if output:
        with Path(output).open("w", encoding="utf-8") as log:
            subprocess.run(args, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                           check=True, encoding="utf-8")
    else:
        subprocess.run(args, cwd=ROOT, check=True)


def build(tier, count):
    directory = ROOT / "ci-artifacts"
    directory.mkdir(exist_ok=True)
    archive = directory / "tests.tar.zst"
    features = ["--features", st.feature_list(st.read_manifest())] if tier == "slow" else []
    run(["cargo", "nextest", "archive", "--locked", "--workspace", *features,
         "--archive-file", str(archive)])
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    (directory / "tests.sha256").write_text(digest + "\n")
    listing = subprocess.check_output(["cargo", "nextest", "list", "--locked", "--workspace",
                                       *features, "--message-format", "json"], cwd=ROOT, text=True)
    compiled = json.loads(listing)
    ci_shards.save(directory / "compiled-list.json", compiled)
    data = ci_shards.plan(compiled, tier, count)
    ci_shards.save(directory / "ci-plan.json", data)
    print(f"compiled inventory: {sum(len(s['tests']) for s in data['suites'])}; estimated shard seconds {data['estimated_seconds']}")
    if tier == "fast":
        run(["cargo", "test", "--locked", "--workspace", "--doc", "--", "--list"], directory / "doc-list.log")
        docs = ci_shards.doc_inventory((directory / "doc-list.log").read_text(encoding="utf-8"))
        ci_shards.save(directory / "doc-list.json", docs)
        run(["cargo", "test", "--locked", "--workspace", "--doc"], directory / "docs.log")
        st.verify_exact_rows(docs, st.parse((directory / "docs.log").read_text(encoding="utf-8")))
        if st.host_os() == "linux":
            dynamic = subprocess.check_output(["readelf", "-d", "target/debug/kinewright-eval"], text=True)
            if "[libavcodec.so.62]" not in dynamic:
                raise ValueError("kinewright-eval does not link pinned libavcodec.so.62")
            print(dynamic)


def extract():
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


def predecessor(pack):
    scratch = ROOT / "target" / "ja-s1-predecessor"
    archive = ROOT / "predecessor.tar"
    if pack:
        with tarfile.open(archive, "w") as output:
            for name in ("sealed", "evidence", "predecessor-format.rs"):
                output.add(scratch / name, arcname=name)
    else:
        scratch.mkdir(parents=True, exist_ok=True)
        with tarfile.open(archive) as source:
            source.extractall(scratch, filter="data")
        base = "076a96c922b2f05984df4bb839ad7cf0900e6183"
        run(["git", "fetch", "--no-tags", "origin", base])
        run(["git", "worktree", "add", "--detach", str(scratch / "source"), base])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["build", "extract", "pack-predecessor", "restore-predecessor"])
    parser.add_argument("--tier", choices=["fast", "slow"])
    parser.add_argument("--count", type=int)
    args = parser.parse_args()
    os.environ["CARGO_TERM_COLOR"] = "never"
    if args.command == "build":
        build(args.tier, args.count)
    elif args.command == "extract":
        extract()
    else:
        predecessor(args.command == "pack-predecessor")

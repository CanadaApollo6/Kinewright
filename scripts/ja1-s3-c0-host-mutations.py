#!/usr/bin/env python3
"""Exercise C0 host admission refusals, one removed guard at a time."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import signal
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "crates/kinewright-agent/src/writer_host/owned_media.rs"
SELECTOR = "writer_host::s2h_tests::s3_host_owned_input_admission_rebinds_only_the_exact_editor_cut"
MUTATIONS = [
    ("editor-binding", ".filter(|lease| lease.client == client)", ".filter(|_lease| true)", "kinewright-agent", SELECTOR),
    ("exact-drained-cut", "if HostSnapshot::capture(self.core())? != *snapshot {", "if false {", "kinewright-agent", SELECTOR),
    ("unsaved-source-owner", "if source.session_id().is_none() || source.session_id() != old.session_id() {", "if false {", "kinewright-app", "s3_owned_unsaved_host_import_transfers_its_new_manifest_on_first_save"),
    ("forwarded-input-scope", "if !roots.permits(&pin.inspected.path, false) {", "if false {", "kinewright-agent", "writer_host::s2h_tests::s3_gui_owned_input_forwarded_scope_refuses_before_admission"),
]


def run(log, package, selector):
    command = """set -e
available_kb=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
if [ "$available_kb" -lt 6291456 ]; then echo 'Insufficient memory'; exit 75; fi
source scripts/setup-ffmpeg.sh
BUILD_HOST
available_kb=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
if [ "$available_kb" -lt 6291456 ]; then echo 'Insufficient memory'; exit 75; fi
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo test -p PACKAGE SELECTOR -- --test-threads=1
"""
    build = "flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo build -p kinewright-agent --bin kinewright" if package == "kinewright-app" else ""
    command = command.replace("BUILD_HOST", build)
    command = command.replace("PACKAGE", shlex.quote(package)).replace("SELECTOR", shlex.quote(selector))
    process = subprocess.Popen(["bash", "-c", command], cwd=ROOT, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, start_new_session=True)
    try:
        raw = process.communicate(timeout=600)[0]
    except BaseException:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate()
        raise
    output = raw.decode(errors="replace")
    with gzip.open(log, "wt") as stream:
        stream.write(output)
    return process.returncode, output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true")
    args = parser.parse_args()
    original = SOURCE.read_bytes()
    text = original.decode()
    for name, old, _new, _package, _selector in MUTATIONS:
        if text.count(old) != 1:
            raise RuntimeError(f"{name} requires one exact anchor")
    if args.list:
        for name, _old, _new, package, selector in MUTATIONS:
            print(f"{name}\t{package}\t{selector}")
        return
    evidence = ROOT / "docs/reports/ja1-s3/evidence/host-mutations"
    evidence.mkdir(parents=True, exist_ok=True)
    digest = hashlib.sha256(original).hexdigest()
    report = {"source": str(SOURCE.relative_to(ROOT)), "source_sha256": digest,
              "results": [], "restored": False}
    try:
        for name, old, new, package, selector in MUTATIONS:
            status, output = run(evidence / f"{name}-baseline.log.gz", package, selector)
            if status or "test result: ok. 1 passed; 0 failed; 0 ignored;" not in output:
                raise RuntimeError(f"{name} baseline witness did not pass")
            SOURCE.write_text(text.replace(old, new, 1))
            status, output = run(evidence / f"{name}-red.log.gz", package, selector)
            if status != 101 or "error[E" in output or "could not compile" in output:
                raise RuntimeError(f"{name} did not reach its runtime witness")
            if "test result: FAILED. 0 passed; 1 failed; 0 ignored;" not in output:
                raise RuntimeError(f"{name} did not produce one failing witness")
            if "assertion `left == right` failed" not in output:
                raise RuntimeError(f"{name} did not fail the refusal assertion")
            SOURCE.write_bytes(original)
            status, output = run(evidence / f"{name}-green.log.gz", package, selector)
            if status or "test result: ok. 1 passed; 0 failed; 0 ignored;" not in output:
                raise RuntimeError(f"{name} did not pass after restoration")
            report["results"].append({"guard": name, "selector": selector,
                                      "red": "1 assertion failure", "green": "1 passed",
                                      "restored_source_sha256": digest})
            print(f"{name}: assertion red, restored green", flush=True)
    finally:
        SOURCE.write_bytes(original)
        report["restored"] = hashlib.sha256(SOURCE.read_bytes()).hexdigest() == digest
        (evidence / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"{len(report['results'])} mutation pairs passed; restored={report['restored']}")


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    main()

#!/usr/bin/env python3
"""Bind C1 source and summarize the retained gate and mutation logs."""

import gzip
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "docs/reports/ja1-s3/c1-evidence"
PARENT = "146072543af79c48efb82273dbf06b5136059cc2"


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT).decode().strip()


paths = sorted(set(git("diff", "--name-only", PARENT).splitlines()
                   + git("ls-files", "--others", "--exclude-standard").splitlines()))
sources = {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest()
           for p in paths if (ROOT / p).is_file() and (
               p.startswith("crates/") or p.startswith("scripts/")
               or p in ("Cargo.toml", "Cargo.lock", ".gitattributes"))}
(EVIDENCE / "source.json").write_text(json.dumps({
    "parent": PARENT, "branch": git("branch", "--show-current"),
    "sha256": sources,
}, indent=2) + "\n")
gates = {}
for path in sorted(EVIDENCE.glob("*.log.gz")):
    text = gzip.open(path, "rt").read()
    gates[path.name] = {
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "exit": ((EVIDENCE / (path.name.removesuffix(".log.gz") + ".exit")).read_text().strip()
                 if (EVIDENCE / (path.name.removesuffix(".log.gz") + ".exit")).is_file() else None),
        "summaries": [line for line in text.splitlines()
                      if "test result:" in line or "Finished " in line
                      or re.search(r"(tests agree|allowlist|manifest)", line)],
    }
(EVIDENCE / "gate-summary.json").write_text(json.dumps(gates, indent=2) + "\n")
pairs = [json.loads(path.read_text()) for path in sorted((EVIDENCE / "mutations").glob("*.json"))]
(EVIDENCE / "mutation-summary.json").write_text(json.dumps({
    "accepted_pairs": sum(pair.get("accepted", False) for pair in pairs), "pairs": pairs,
}, indent=2) + "\n")

# Preserve literal run provenance while binding unchanged compiled witnesses to
# the requested test-only parent advance. Never rewrite a pair's original facts.
mutation_source = EVIDENCE / "mutation-source.json"
if mutation_source.is_file() and pairs and all(pair.get("accepted") for pair in pairs):
    original_bytes = mutation_source.read_bytes()
    original = json.loads(original_bytes)
    cases = {case["name"]: case for case in json.loads(
        (ROOT / "scripts/ja1-s3-c1-mutation-cases.json").read_text())}
    assert set(cases) == {pair["name"] for pair in pairs}
    for path, expected in original["sha256"].items():
        if path.startswith("crates/"):
            assert sources[path] == expected, f"compiled C1 source changed: {path}"
    upstream_changes = git("diff", "--name-only", original["parent"], PARENT).splitlines()
    assert upstream_changes == ["crates/kinewright-project/src/owned_media.rs"]
    old_media = subprocess.check_output([
        "git", "show", original["parent"] + ":" + upstream_changes[0]], cwd=ROOT)
    new_media = subprocess.check_output([
        "git", "show", PARENT + ":" + upstream_changes[0]], cwd=ROOT)
    marker = b"\nmod tests {"
    old_prefix, _old_tests = old_media.split(marker, 1)
    new_prefix, _new_tests = new_media.split(marker, 1)
    assert old_prefix == new_prefix
    assert old_prefix.endswith(b'#[cfg(all(test, target_os = "linux"))]')
    assert all(case["kind"] == "--test" for case in cases.values()
               if case["package"] == "kinewright-project")
    verified = []
    for pair in pairs:
        case = cases[pair["name"]]
        assert pair["baseline_manifest_sha256"] == hashlib.sha256(original_bytes).hexdigest()
        assert pair["restored"] and pair["red_exit"] == 101 and pair["green_exit"] == 0
        assert pair["changes"] == case["changes"]
        assert pair["expected_red"] == {"text": case["red_text"],
                                        "location": case.get("red_location")}
        for path, expected in pair["source_sha256"].items():
            assert hashlib.sha256((ROOT / path).read_bytes()).hexdigest() == expected, path
        assert hashlib.sha256((ROOT / case["witness_source"]).read_bytes()).hexdigest() == pair["witness_sha256"]
        changed = {path: (ROOT / path).read_text() for path in pair["source_sha256"]}
        for path, old, new in case["changes"]:
            assert changed[path].count(old) == 1
            changed[path] = changed[path].replace(old, new, 1)
        for path, text in changed.items():
            assert hashlib.sha256(text.encode()).hexdigest() == pair["mutated_source_sha256"][path]
        for phase in ("red", "green"):
            log = EVIDENCE / "mutations" / f"{pair['name']}-{phase}.log.gz"
            assert hashlib.sha256(log.read_bytes()).hexdigest() == pair[f"{phase}_log_sha256"]
            output = gzip.open(log, "rt").read()
            if phase == "red":
                assert "test result: FAILED. 0 passed; 1 failed; 0 ignored;" in output
                assert case["red_text"] in output and case.get("red_location", "") in output
                assert "could not compile" not in output and "error[E" not in output
            else:
                assert "test result: ok. 1 passed; 0 failed; 0 ignored;" in output
        verified.append(pair["name"])
    required_gates = {
        "build", "clippy", "project-affected", "project-store", "agent-affected",
        "app-affected", "s2-host", "m31-branches", "presenter", "agent-architecture",
        "app-architecture", "source-guard", "self-exec", "save-cost", "slow-lint",
        "fmt", "activation", "g05l_host_death_recovers_absent_and_committed_drag_suffix",
        "g05l_shared_floor_and_recovery_close_reply_drop",
        "g05a_switch_reply_drop_and_participant_death",
    }
    (EVIDENCE / "final-binding.json").write_text(json.dumps({
        "final_parent": PARENT, "mutation_parent": original["parent"],
        "mutation_manifest_sha256": hashlib.sha256(original_bytes).hexdigest(),
        "upstream_changes": upstream_changes, "project_non_test_prefix_unchanged": True,
        "mutation_targets_compile_project_as_non_test_dependency": True,
        "all_c1_crate_source_hashes_match": True,
        "all_target_witness_mutant_and_log_hashes_match": True,
        "verified_cases": sorted(verified),
        "final_gate_complete": all(
            gates.get(name + ".log.gz", {}).get("exit") == "0" for name in required_gates),
    }, indent=2) + "\n")
print(f"{len(sources)} source hashes; {len(gates)} gate logs; "
      f"{sum(pair.get('accepted', False) for pair in pairs)} accepted mutation pairs")

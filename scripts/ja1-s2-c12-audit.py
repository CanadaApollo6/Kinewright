#!/usr/bin/env python3
"""Bind C12 fixture bytes, executed gates, mutations and the single local commit."""
import hashlib
import gzip
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
PARENT = "2aa6633db61912a416948edb580e2df8a3ddd58f"
REPORTS = {"docs/reports/ja1-s2/c12.md", "docs/reports/ja1-s2/README.md"}
APP_SIMPLIFICATION = {'crates/kinewright-app/src/ja1_attachment.rs': ('50baaea3dfb706f559e5cc8c0ea5931027d784ab1979b00f5bb157cbbce529ce', 'e70b349aaadc999630199aa29020eaf7f51fdd61f6c207586e380b9034f003f5'), 'crates/kinewright-app/src/ja1_attachment_tests.rs': ('469dfc3c4de45dbac68f86357f541a4d0dbfc47d19a19cabb9b34dad53d749a8', '89a3641e976ab77c53fd14f8cc666d1b7db021e32a7160e3ca150252ab5a3076')}


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT)


def content(path):
    return path.read_text() if path.exists() else gzip.decompress(Path(str(path) + ".gz").read_bytes()).decode()

def read(path):
    return json.loads(content(path))


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def fixtures(evidence):
    prefix = "crates/kinewright-agent/tests/fixtures/ja/"
    names = git("ls-tree", "-r", "--name-only", PARENT, "--", prefix).decode().splitlines()
    hashes = {}
    for name in names:
        original = git("show", PARENT + ":" + name)
        assert (ROOT / name).read_bytes() == original, name
        hashes[name] = sha(ROOT / name)
    original = json.loads(git("show", "cb4f6e2:" + prefix + "fixtures-index.json"))
    current = read(ROOT / prefix / "fixtures-index.json")
    assert current["fixtures"][:len(original["fixtures"])] == original["fixtures"]
    attrs = git("check-attr", "text", "--", *names).decode()
    assert attrs.count("text: unset") == len(names), attrs
    write(evidence / "fixture-preservation.json", dict(parent=PARENT, files=hashes,
          unchanged_files=len(names), original_index_prefix=len(original["fixtures"]),
          current_index_entries=len(current["fixtures"]), attributes=attrs))


def gate(evidence):
    directory = evidence / "gate-final"
    before, after = read(directory / "source-before.json"), read(directory / "source-after.json")
    assert before == after
    names = {p.decode() for p in git("ls-files", "-co", "--exclude-standard", "-z").split(b"\0") if p}
    files = {name: sha(ROOT / name) for name in names if (ROOT / name).is_file() and not (ROOT / name).is_symlink()}
    assert set(files) == set(before["files"])
    changed = {name for name in files if files[name] != before["files"][name]}
    assert changed <= REPORTS, changed
    rows = dict(line.split("\t") for line in (directory / "gate.tsv").read_text().splitlines())
    assert rows and all(code == "0" for code in rows.values()), rows
    assert {"full-project", "full-agent", "full-app", "workspace-fast-verifier"} <= set(rows)
    selected = {p.stem: read(p) for p in directory.glob("*-counts.json")}
    required = {
        "host-integration": ["rescue_and_switch_share_one_owner_after_restart",
            "g05l_fast_restart_absent_vs_committed_lost_reply_suffix",
            "g05l_fast_restart_shared_floor_and_close_reply_loss",
            "g05a_fast_b5_switch_reply_loss_before_and_after_restore",
            "host_failure_preserves_originating_context_or_refuses_recovery",
            "w14_headless_and_agent_admission_precedes_mutation"],
        "ja_s2_rescue_record": ["w14_local_headless_admission_precedes_mutation",
            "review_fix2::n2_sealed_a_failed_b_resume_returns_committed_capture"],
        "local-agent-admission": ["w14_local_agent_admission_precedes_mutation"],
        "app": ["ja1_attachment::tests::n2_sealed_a_discloses_committed_b_and_offers_open",
            "ja1_attachment::tests::c8_retention_loss_fences_every_action_and_keeps_actual_source_until_close"],
    }
    for label, witnesses in required.items():
        output = content(directory / (label + ".log"))
        for witness in witnesses:
            assert f"test {witness} ... ok" in output, (label, witness)
    summaries = {}
    for label in ["full-project", "full-agent", "full-app", "workspace-fast"]:
        output = content(directory / (label + ".log"))
        lines = re.findall(r"^test result:.*$", output, re.M)
        assert lines and not re.findall(r"^test .+ \.\.\. FAILED$", output, re.M)
        counts = [tuple(map(int, re.search(r"(\d+) passed; (\d+) failed; (\d+) ignored", line).groups())) for line in lines]
        assert all(n[1] == 0 for n in counts)
        summaries[label] = dict(raw=lines, passed=sum(n[0] for n in counts), failed=0,
                                ignored=sum(n[2] for n in counts), failures=[])
    pairs = []
    for case in read(ROOT / "scripts/ja1-s2-c12-mutation-cases.json"):
        folder = evidence / "mutations-final" / case["name"]
        for name, original in read(folder / "originals.json").items():
            assert (ROOT / name).read_bytes() == original.encode(), (case["name"], name)
        executed = read(folder / "executed-source.json")
        assert executed
        deltas = [name for name, digest in executed.items() if before["files"][name] != digest]
        # App is not a linked dependency of Core, Project or Agent. Their exact
        # restored-green binary must still equal the final affected-gate binary.
        unlinked = [name for name in deltas if case["package"] != "kinewright-app" and name.startswith("crates/kinewright-app/")]
        assert set(deltas) == set(unlinked), (case["name"], deltas)
        assert all(name in APP_SIMPLIFICATION and (executed[name], before["files"][name]) == APP_SIMPLIFICATION[name] for name in unlinked), (case["name"], "unexpected App delta")
        red, green = read(folder / "red-result.json"), read(folder / "green-result.json")
        assert red["accepted"] and green["accepted"]
        if case["package"] == "kinewright-project":
            label = case["target"]
        elif case["package"] == "kinewright-core":
            label = "core-actor"
        elif case["package"] == "kinewright-app":
            label = "app"
        elif case["kind"] == "--test":
            label = "host-switch-matrix" if "slow-tests" in case["features"] else "host-integration"
        else:
            label = "agent-scripted" if case["features"] == "eval-harness" else "host-lib"
        assert green["binary_sha256"] == selected[label + "-counts"]["binary_sha256"], (case["name"], label, "green binary differs")
        pairs.append(dict(name=case["name"], witness=case["witness"], red=red, green=green, unlinked_source_deltas=unlinked))
    fixtures(evidence)
    subprocess.run(["git", "diff", "--check"], cwd=ROOT, check=True)
    write(evidence / "final-audit.json", dict(parent=PARENT, files=files, post_gate_changes=sorted(changed),
          selected=selected, summaries=summaries, mutations=pairs, command_rows=len(rows),
          expected=sum(r["expected"] for r in selected.values()),
          run=sum(r["run"] for r in selected.values()),
          ignored=sum(len(r["ignored_not_run"]) for r in selected.values())))


def commit(evidence):
    gate(evidence)
    head = git("rev-parse", "HEAD").decode().strip()
    assert git("rev-parse", "HEAD^").decode().strip() == PARENT
    assert git("rev-list", "--count", PARENT + "..HEAD").strip() == b"1"
    assert git("branch", "--show-current").strip() == b"ja1/s2"
    assert not git("status", "--porcelain=v1")
    message = git("log", "-1", "--format=%B").decode().strip()
    assert message.splitlines()[0].endswith("[slow-tier]")
    assert message.endswith("Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>")
    assert git("rev-parse", "refs/remotes/origin/ja1/s2").decode().strip() == PARENT
    audit = read(evidence / "final-audit.json")
    changed = git("diff-tree", "--no-commit-id", "--name-only", "-r", head).decode().splitlines()
    for name in changed:
        assert hashlib.sha256(git("show", head + ":" + name)).hexdigest() == audit["files"][name], name
    write(evidence / "commit-binding.json", dict(head=head, parent=PARENT, branch="ja1/s2",
          worktree=str(ROOT), clean=True, commits=1, committed_files=changed,
          source_manifest_sha256=sha(evidence / "final-audit.json"),
          remote_tracking_ref=git("rev-parse", "refs/remotes/origin/ja1/s2").decode().strip(),
          pushed=False))


if __name__ == "__main__":
    operation, folder = sys.argv[1], Path(sys.argv[2])
    {"fixtures": fixtures, "gate": gate, "commit": commit}[operation](folder)

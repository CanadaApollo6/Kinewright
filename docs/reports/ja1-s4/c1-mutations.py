"""Run only protected C1 contract witnesses, restoring every source mutation."""
from pathlib import Path
import hashlib
import json
import subprocess

ROOT = Path(__file__).resolve().parents[3]
REPORT = ROOT / "docs/reports/ja1-s4"
COMMIT = "crates/kinewright-project/src/orchestration_commit.rs"
RECORDS = "crates/kinewright-project/src/orchestration_records.rs"
MACHINE = "crates/kinewright-agent/src/orchestration/machine.rs"
OUTBOX = "crates/kinewright-agent/src/orchestration/outbox.rs"
MODULE = "crates/kinewright-agent/src/orchestration/mod.rs"

CASES = [
    ("reservation_scope", COMMIT, "if receipt.envelope != *envelope {", "if false {",
     "kinewright-project", "ja_s4_store", "s4_receipt_scope_collision_forced_reservation_loser_contract"),
    ("returned_cut", COMMIT, "if *receipt != self.receipt {", "if false {",
     "kinewright-project", "ja_s4_store", "s4_one_root_cut_and_replay_contract"),
    ("pending_effect_edge", COMMIT, "refs.extend(state.effects.iter().map(|effect| effect.payload.clone()));", "",
     "kinewright-project", "ja_s4_store", "s4_run_graph_pending_dependencies_contract"),
    ("schema_support", COMMIT, "if self.version != 1 {", "if false {",
     "kinewright-project", "ja_s4_store", "s4_support_and_original_envelope_contract"),
    ("original_digest", RECORDS, "self.id.sequence == 0 || self.digest != self.computed_digest()?", "self.id.sequence == 0",
     "kinewright-project", "ja_s4_store", "s4_support_and_original_envelope_contract"),
    ("unknown_freeze", MACHINE, "if self.uncertain || self.pending.is_some() {", "if false {",
     "kinewright-agent", "ja_s4_kernel", "s4_unknown_publication_freezes_original_envelope_contract"),
    ("no_reclaim", OUTBOX, "(first.state == EffectState::Pending).then_some(first)", "Some(first)",
     "kinewright-agent", "ja_s4_kernel", "s4_outbox_order_unknown_and_reconcile_contract"),
    ("reservation_identity", MODULE, "reservation.identity == *identity", "true",
     "kinewright-agent", "ja_s4_kernel", "s4_durable_before_commit_attempt_contract"),
    ("refused_attempt", MODULE, "returned.decision != CommandDecision::Applied", "false",
     "kinewright-agent", "ja_s4_kernel", "s4_durable_before_commit_attempt_contract"),
    ("predecessor_edge", COMMIT, "segment.first_sequence > 1 && segment.previous.is_none()", "false",
     "kinewright-project", "ja_s4_store", "s4_event_predecessor_retention_contract"),
    ("attempt_unique", COMMIT, "validate_attempt_ids(state)?;", "",
     "kinewright-project", "ja_s4_store", "s4_attempt_identity_unique_contract"),
    ("cleanup_facts", COMMIT, "validate_cleanup(state)?;", "",
     "kinewright-project", "ja_s4_store", "s4_cleanup_matches_attempt_facts_contract"),
]


def run(crate, target, witness, path):
    command = ["bash", "docs/reports/ja1-s4/c1-cargo.sh", "test", "-p", crate,
               "--test", target, witness, "--", "--exact"]
    with path.open("wb") as log:
        result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=False)
    output = path.read_text()
    summaries = [line for line in output.splitlines() if line.startswith("test result:")]
    return result.returncode, summaries, output


results = []
for name, relative, old, replacement, crate, target, witness in CASES:
    path = ROOT / relative
    original = path.read_bytes()
    source = original.decode()
    if source.count(old) != 1:
        raise RuntimeError(f"{name}: expected exactly one mutation site")
    try:
        path.write_text(source.replace(old, replacement, 1))
        code, red, output = run(crate, target, witness, REPORT / f"mutation-{name}-red.log")
        if code == 0 or not any("FAILED" in line for line in red):
            raise RuntimeError(f"{name}: no behavioral red, exit={code}, summaries={red}")
    finally:
        path.write_bytes(original)
    code, green, output = run(crate, target, witness, REPORT / f"mutation-{name}-green.log")
    if code != 0 or not any("1 passed; 0 failed" in line for line in green):
        raise RuntimeError(f"{name}: restored green failed, exit={code}, summaries={green}")
    entry = {"name": name, "source": relative, "source_sha256": hashlib.sha256(original).hexdigest(),
             "witness": witness, "red": red, "green": green}
    results.append(entry)
    (REPORT / "c1-mutations.json").write_text(json.dumps(results, indent=2) + "\n")
    print(f"{name}: runtime red / restored green", flush=True)
print(f"C1 protected contract mutation pairs: {len(results)}", flush=True)

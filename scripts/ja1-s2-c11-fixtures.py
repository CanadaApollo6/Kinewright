#!/usr/bin/env python3
"""Regenerate only C11 additive fixture bytes and their own index entries."""
import hashlib
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "crates/kinewright-agent/tests/fixtures/ja"
def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")
def reference(name, kind="input"):
    data = (FIXTURES / name).read_bytes()
    return dict(relative_file=name, sha256=hashlib.sha256(data).hexdigest(), byte_length=len(data), kind=kind)
def main():
    index_path = FIXTURES / "fixtures-index.json"
    original = json.loads(index_path.read_text())
    prefix = [entry for entry in original["fixtures"] if entry["fixture_id"] not in ("s2-switch-reply-drop", "s2-switch-return")]
    entries = []
    for floor in (False, True):
        label = "s2-switch-return" if floor else "s2-switch-reply-drop"
        control = label + "-control.json"
        write(FIXTURES / control, dict(batches=[], at=0, switch=not floor))
        actions = [dict(id=1, participant="main", action="app_fixture", args=dict(input="control")),
                   dict(id=2, participant="main", action="assert_observed", args=dict(id="query-ran", observation=1, pointer="/query", oracle="true", evidence_class="control", comparison="equal"))]
        for field in ("exact", "frozen", "first_undo", "stale", "saved", "generation"):
            actions.append(dict(id=len(actions)+1, participant="main", action="assert_observed", args=dict(id=field.replace("_", "-"), observation=1, pointer="/switch/"+field, oracle="true", evidence_class="revised_property", positive_control="query-ran", comparison="equal")))
        fixture = dict(protocol="kinewright.ja-harness", protocol_version=1, fixture_id=label, gate_ids=["JA1.G5-A"], lane="app-paint", requirements=[], inputs=dict(control=reference(control)), initial=dict(revision=0, participants=[]), actions=actions, barriers=[], oracles={"true": reference("oracle-true.json")}, limits=dict(input_bytes=1048576, output_bytes=16777216, action_timeout_ms=60000, setup_timeout_ms=30000))
        write(FIXTURES / (label + ".json"), fixture)
        entry = reference(label + ".json"); entry.pop("kind")
        entries.append(dict(fixture_id=label, **entry, expected_baseline_process_exit=0, inputs=fixture["inputs"], oracles=fixture["oracles"]))
    original["fixtures"] = prefix + entries
    write(index_path, original)
if __name__ == "__main__":
    main()

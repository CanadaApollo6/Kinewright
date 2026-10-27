"""Check the corrected historical evidence and original-source attribution."""
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[3]
REPORT = ROOT / "docs/reports/ja1-s4"
parent = subprocess.check_output(
    ["git", "rev-parse", "72c5518a^"], cwd=ROOT, text=True
).strip()
identities = json.loads((REPORT / "c1-identities.json").read_text())
original = json.loads(subprocess.check_output(
    ["git", "show", "061573e:docs/reports/ja1-s4/c1-identities.json"], cwd=ROOT, text=True
))
assert identities["base"] == parent == "4250d146eb767b64a058bcbafb5f9adbccbb5264"
assert identities["original_measured_base"] == original["base"]
for key in original.keys() - {"base"}:
    assert identities[key] == original[key], f"historical evidence changed: {key}"
assert "NOT RUN at 4250d14" in identities["rerun_at_reviewed_parent"]
for filename in ["c1.md", "c1-handoff.md"]:
    text = (REPORT / filename).read_text()
    assert parent in text and original["base"] in text
    assert "NOT RUN at base `4250d14`" in text
for filename in ["policy.rs", "server_tests.rs"]:
    text = (ROOT / "crates/kinewright-agent/src/orchestration" / filename).read_text()
    assert "Original Kinewright" in text and "no upstream source was translated" in text
text = (REPORT / "c4-pre-h3.md").read_text()
assert "included at the lead's request" in text
assert "included at Riel's explicit request" not in text
print("FU1 provenance, retained C1 identities, C3 headers and T1 attribution: PASS")

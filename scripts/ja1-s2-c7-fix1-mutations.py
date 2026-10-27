#!/usr/bin/env python3
"""Runtime controls for C7's path identity correction; run with nice -n 19 python3 -I."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = Path(sys.argv[1]).resolve()
OUT.mkdir(parents=True, exist_ok=True)
ENV = {k: v for k, v in os.environ.items() if not k.startswith(("JA_", "KINEWRIGHT_", "REV2_"))}
ENV.update(CARGO_TARGET_DIR=str(ROOT / "target/c7-fix1-build"),
           TMPDIR=str(ROOT / "target/c7-fix1-tmp"), CARGO_BUILD_JOBS="4", RUST_TEST_THREADS="4")
APP = "crates/kinewright-app/src/"
PROJECT = "crates/kinewright-project/src/"
IDENTITY = "app::in2b_tests::g04_session_spellings_match_file_identity_and_reuse_writer"
SPELLING = "app::in2b_tests::g04_open_and_save_preserve_the_persons_path"


def replace(source, before, after):
    assert source.count(before) == 1, (before, source.count(before))
    return source.replace(before, after, 1)


def path_guard(source):
    begin = source.index("    fn session_index_for_path(")
    end = source.index("    /// Whether a session other", begin)
    part = replace(source[begin:end], "same_project_file(open, path)",
                   "kinewright_project::canonical_project_identity(open).ok() == kinewright_project::canonical_project_identity(path).ok()")
    return source[:begin] + part + source[end:]


def caller_sidecar(source):
    begin = source.index("    pub fn flush_incidents(")
    end = source.index("    /// [`Self::flush_incidents`]", begin)
    part = source[begin:end]
    part = replace(part, '        let identity = self\n            .authority\n            .as_ref()\n            .map(|writer| writer.resolved_identity().to_path_buf());\n        let project_path = identity.as_deref().or(project_path);\n', '')
    part = replace(part, '&& occupied_path.exists()', '&& sidecar_path.exists()')
    return source[:begin] + part + source[end:]


def publish_unsaved_root(source):
    begin = source.index("    pub(crate) fn publish_project_path_to_agents(")
    end = source.index("    /// Build this session", begin)
    part = re.sub(r'\*slot = self\s*\.project_path\s*\.as_ref\(\)\s*\.map\(\|display\|', '*slot = Some(&PathBuf::new()).map(|display|', source[begin:end], count=1)
    assert part != source[begin:end]
    return source[:begin] + part + source[end:]


ROWS = [
    ("unsaved-studio-publishes-temporary-root", APP + "project.rs", publish_unsaved_root,
     "app::in2b_tests::g04_unsaved_studio_keeps_the_agent_project_path_absent",
     "an unsaved Studio session must not advertise its temporary root as a saved project"),
    ("save-as-alias-checks-caller-sidecar", PROJECT + "session.rs", caller_sidecar,
     "app::in2b_tests::g04_save_as_alias_preserves_the_actual_foreign_sidecar",
     "Save As alias preserves the actual foreign sidecar"),
    ("ordinary-save-ignores-bound-identity", APP + "app.rs",
     lambda s: replace(s, '            && self\n                .focused()\n                .recovery\n                .writer()\n                .is_some_and(|writer| !writer.names_project(path))', '            && false'),
     "app::in2b_tests::g04_retargeted_display_alias_cannot_transfer_an_ordinary_save",
     "ordinary Save cannot acquire the retargeted display alias"),
    ("session-uses-canonical-strings", APP + "app.rs", path_guard, IDENTITY, "a true alias is the same session"),
    ("writer-reuse-uses-canonical-strings", PROJECT + "lockfile.rs",
     lambda s: replace(s, "crate::same_project_file(path, &self.identity)",
                       "crate::canonical_project_identity(path).is_ok_and(|resolved| resolved == self.identity)"),
     IDENTITY, "alias reuses the held writer"),
    ("open-exposes-canonical-path", APP + "project.rs",
     lambda s: replace(s, "        let io_project_path = source_admission.source().or(project_path.as_deref());",
                       "        let project_path = source_admission.source().map(Path::to_path_buf).or(project_path);\n        let io_project_path = source_admission.source().or(project_path.as_deref());"),
     SPELLING, "open keeps the supplied spelling"),
    ("save-exposes-canonical-path", APP + "app.rs",
     lambda s: replace(s, "        let document = Arc::clone(&self.focused().document);\n        let previous_store = self.focused().lut_store.clone();",
                       "        let path = writer.resolved_identity();\n        let document = Arc::clone(&self.focused().document);\n        let previous_store = self.focused().lut_store.clone();"),
     SPELLING, "save keeps the supplied spelling"),
]


def run(directory, label, test, red, reason):
    assert int(re.search(r"MemAvailable:\s+(\d+)", Path("/proc/meminfo").read_text()).group(1)) >= 6291456
    command = ["bash", "-c", 'source scripts/setup-ffmpeg.sh >/dev/null; exec nice -n 10 cargo "$@"',
               "c7-fix1", "test", "-p", "kinewright-app", "--bin", "kinewright-app", "--", "--exact", test, "--test-threads=1"]
    (directory / (label + ".command.json")).write_text(json.dumps(command, indent=2) + "\n")
    with (directory / (label + ".stdout")).open("w") as stdout, (directory / (label + ".stderr")).open("w") as stderr:
        result = subprocess.run(command, cwd=ROOT, env=ENV, stdout=stdout, stderr=stderr, timeout=600)
    output = (directory / (label + ".stdout")).read_text() + (directory / (label + ".stderr")).read_text()
    summaries = re.findall(r"^test result:.*$", output, re.M)
    assert len(summaries) == 1 and "error[E" not in output, directory
    assert result.returncode == (101 if red else 0), directory
    assert f"test {test} ... {'FAILED' if red else 'ok'}" in output, directory
    assert ("0 passed; 1 failed" if red else "1 passed; 0 failed; 0 ignored") in summaries[0], directory
    if red:
        assert reason in output, directory
    binary = re.search(r"Running .* \(([^()]+)\)", output).group(1)
    return dict(exit=result.returncode, summary=summaries[0], binary=binary,
                binary_sha256=hashlib.sha256((ROOT / binary).read_bytes()).hexdigest())


results = []
for name, relative, mutate, test, reason in ROWS:
    directory = OUT / name
    directory.mkdir(exist_ok=True)
    path = ROOT / relative
    original = path.read_bytes()
    (directory / "source.original").write_bytes(original)
    try:
        changed = mutate(original.decode()).encode()
        assert changed != original
        path.write_bytes(changed)
        red = run(directory, "red", test, True, reason)
    finally:
        path.write_bytes(original)
    green = run(directory, "green", test, False, reason)
    assert path.read_bytes() == original
    results.append(dict(mutation=name, source=relative, source_sha256=hashlib.sha256(original).hexdigest(),
                        test=test, red=red, green=green))
    (OUT / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(name + ": runtime red/restored green", flush=True)

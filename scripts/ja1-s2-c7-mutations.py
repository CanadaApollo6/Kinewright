#!/usr/bin/env python3
"""C7 targeted runtime mutations. Run at nice -n 19 with python3 -I."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
OUT = Path(sys.argv[1]).resolve()
OUT.mkdir(parents=True, exist_ok=True)
ONLY = set(sys.argv[2:])
ENV = dict({key: value for key, value in os.environ.items() if not key.startswith(("JA_", "KINEWRIGHT_", "REV2_"))}, CARGO_TARGET_DIR=str(ROOT / "target/c7-build"), TMPDIR=str(ROOT / "target/c7-tmp"), CARGO_BUILD_JOBS="4", RUST_TEST_THREADS="4")
P = "crates/kinewright-project/src/"
U = "crates/kinewright-app/src/recovery.rs"
PROJECT = "kinewright-project"
APP = "kinewright-app"

def exact(package, kind, target, *names):
    args = ["test", "-p", package, kind]
    if target:
        args.append(target)
    if package == PROJECT:
        args += ["--features", "test-util"]
    if package == "kinewright-agent":
        args += ["--features", "eval-harness"]
    return args + ["--", "--exact", *names, "--test-threads=1"]

def project(name):
    return exact(PROJECT, "--test", "ja_s2_writer", name)

def app(*names):
    return exact(APP, "--bin", "kinewright-app", *["ja1_attachment::tests::" + n for n in names])

TWO = project("g04_two_process_lock_save_alias_and_discovery")
PIN = project("g04_parent_and_asset_directories_remain_pinned")
PROBE = project("g04_suffixed_journal_direct_probe_and_release_identity")
LOST = app("g04_gui_journal_lost_authority_refuses_create_append_and_retire")


def replace(source, old, new):
    if source.count(old) != 1:
        raise RuntimeError(f"expected one source match, got {source.count(old)}: {old[:100]}")
    return source.replace(old, new, 1)


def scoped(source, start, end, edit):
    begin = source.index(start)
    finish = source.index(end, begin)
    return source[:begin] + edit(source[begin:finish]) + source[finish:]


def old_legacy(source):
    previous = subprocess.check_output(["git", "show", "4952851:crates/kinewright-project/src/headless.rs"], cwd=ROOT).decode()
    begin = previous.index("pub fn save_headless(")
    finish = previous.index("/// Save an explicitly activated Studio bundle", begin)
    previous = previous[begin:finish].replace("    admission.authorize_legacy_write(path)?;", "    admission.authorize_legacy_write(path)?;\n    #[cfg(any(test, feature = \"test-util\"))]\n    crate::test_hook(\"legacy_save_verified\");", 1)
    # Retain the new sidecar capability so the successful control still lands;
    # this mutation specifically restores caller-path project IO after verification.
    previous = previous.replace("    let json = serialize_project_document(document)?;", "    sidecar.authority = lock.cloned();\n    let json = serialize_project_document(document)?;", 1)
    return scoped(source, "pub fn save_headless(", "/// Save an explicitly activated Studio bundle", lambda _: previous)


def omit_suffix(source):
    return scoped(source, "    for entry in fs::read_dir(recovery_dir)? {", "    // N1: `read_dir` can miss an entry", lambda _: "")


def omit_gui_create(source):
    source = scoped(source, "    fn create_owned(", "    fn append(", lambda part: replace(part, '        if let Some(writer) = &authority {\n            writer.verify().map_err(|e| e.to_string())?;\n        }\n', ""))
    return scoped(source, "    fn attach_owned(", "    /// Saving establishes", lambda part: re.sub(r'        if writer\s*\.as_ref\(\)\s*\.is_some_and\(\|writer\| writer.verify\(\).is_err\(\)\)\s*\{.*?            return;\n        }\n', "", part, count=1, flags=re.S))


def omit_gui_retire(source):
    return scoped(source, "    fn remove_journal(", "impl Drop for Recovery", lambda part: re.sub(r'        if self\s*\.authority\s*\.as_ref\(\)\s*\.is_some_and\(\|writer\| writer.verify\(\).is_err\(\)\)\s*\{\n            return;\n        }\n', "", part, count=1))


def raw_load_refusal(source):
    return scoped(source, "fn refuse_loaded_sidecar(", "/// Load a session", lambda _: '''fn refuse_loaded_sidecar(project: &Path, _authority: Option<&crate::LockfileHandle>, rename: Option<&RefuseRename>) -> std::io::Result<PathBuf> {
    let path = sidecar_path_for_project(Some(project)).ok_or_else(|| std::io::Error::other("sidecar filename"))?;
    match rename { Some(rename) => refuse_sidecar_with(&path, rename), None => refuse_sidecar(&path) }
}

''')


def drop_queued_owner(source):
    source = replace(source, "for (path, job) in winners {", "for (path, mut job) in winners {")
    return replace(source, "            write_one_job(&owned_path, job.seq, &job.bytes, rename_hook)", "            let held_directory = crate::legacy_io::PinnedDirectory::open(&owned_path.parent().ok_or_else(|| io::Error::other(\"sidecar parent\"))?.join(\".\"))?;\n            let owned_path = held_directory.io_path().join(owned_path.file_name().ok_or_else(|| io::Error::other(\"sidecar name\"))?);\n            drop(job.authority.take());\n            write_one_job(&owned_path, job.seq, &job.bytes, rename_hook)")

ROWS = [
    ("legacy-verify-then-caller-path", {P + "headless.rs": old_legacy}, [TWO, exact("kinewright-agent", "--test", "ja_baseline", "c7_writer_identity_fixture_uses_real_alias_save")]),
    ("legacy-parent-reopened", {P + "lockfile.rs": lambda s: scoped(s, "    pub fn io_path(", "    /// Confirm the handle", lambda _: "    pub fn io_path(&self) -> PathBuf { self.spelling.clone() }\n\n")}, [PIN]),
    ("asset-subdirectory-reopened", {P + "project.rs": lambda s: replace(s, '&destination.io_path().join(".")', '&next.luts_dir()')}, [PIN]),
    ("final-suffixed-probe-omitted", {P + "recovery.rs": omit_suffix}, [PROBE]),
    ("release-fd-verification-omitted", {P + "lockfile.rs": lambda s: replace(s, "&& self.verify().is_ok()", "")}, [PROBE]),
    ("sidecar-worker-verification-omitted", {P + "sidecar.rs": lambda s: replace(s, "            authority.verify().map_err(io::Error::other)?;\n", "")}, [project("g04_sidecar_refuses_lost_authority_and_preserves_pending_journal")]),
    ("queued-sidecar-owner-dropped", {P + "sidecar.rs": drop_queued_owner}, [exact(PROJECT, "--lib", "", "sidecar::writer_authority_tests::g04_queued_job_retains_writer_after_session_drop")]),
    ("sidecar-load-renames-without-authority", {P + "session.rs": raw_load_refusal}, [project("g04_sidecar_load_cannot_rename_under_another_writer")]),
    ("gui-lock-not-acquired", {P + "lockfile.rs": lambda s: replace(s, "if file.try_lock_exclusive().is_err() {", "if check_recovery && file.try_lock_exclusive().is_err() {")}, [app("g04_real_gui_journal_cannot_appear_after_owner_scan"), exact(APP, "--bin", "kinewright-app", "ja_harness::tests::gui_journal_additive_fixture_uses_real_recorder")]),
    ("gui-create-verification-omitted", {U: omit_gui_create}, [LOST]),
    ("gui-append-verification-omitted", {U: lambda s: scoped(s, "    fn append(", "#[derive(Debug, Clone)]\nstruct Damage", lambda part: replace(part, '        if let Some(writer) = &self.authority {\n            writer.verify().map_err(|e| e.to_string())?;\n        }\n', ""))}, [LOST]),
    ("gui-retire-verification-omitted", {U: omit_gui_retire}, [LOST]),
    ("gui-startup-retirement-owner-omitted", {U: lambda s: replace(s, "            let _owner = self.writer_for(project)?;", "            let _ = project;")}, [LOST]),
    ("gui-recovery-directory-reopened", {U: lambda s: replace(s, "if self.directory_owner.is_none() {", "if true {")}, [app("g04_gui_journal_directory_remains_pinned_across_checkpoint")]),
    ("startup-journal-directory-reopened", {U: lambda s: scoped(s, "    fn remove_pending(", "    /// A cross-version restore", lambda part: replace(part, '            let directory = entry\n                .directory\n                .as_ref()\n                .ok_or("journal directory identity is unavailable")?;', '            let directory = kinewright_project::legacy_io::PinnedDirectory::open(&fs::canonicalize(entry.journal_path.parent().ok_or("journal parent")?).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;'))}, [app("g04_startup_journal_retirement_uses_scanned_parent_and_bytes")]),
    ("startup-journal-changed-bytes-accepted", {U: lambda s: replace(s, "if entry.sha256.as_deref() != Some(&kinewright_media::sha256_bytes(&bytes)) {", "if false {")}, [app("g04_startup_journal_retirement_uses_scanned_parent_and_bytes")]),
    ("close-retains-journal-writer", {"crates/kinewright-app/src/project.rs": lambda s: replace(s, "        self.recovery.close();\n", "")}, [exact(APP, "--bin", "kinewright-app", "app::in2b_tests::in2b_a_proposal_after_the_last_save_survives_close_reopen")]),
    ("restore-uses-copy-instead-of-active-owner", {"crates/kinewright-app/src/app.rs": lambda s: replace(s, "let owner = retirement_owner.unwrap_or(self.focused_project);", "let _ = retirement_owner;\n                let owner = self.focused_project;")}, [exact(APP, "--bin", "kinewright-app", "app::in2b_tests::in2b_recovery_restores_without_the_digest_gate_and_refuses_cross_version")]),
]


def run(args, directory, label, red):
    while int(re.search(r"MemAvailable:\s+(\d+)", Path("/proc/meminfo").read_text()).group(1)) < 6291456:
        time.sleep(5)
    command = ["bash", "-c", 'source scripts/setup-ffmpeg.sh >/dev/null; exec nice -n 10 cargo "$@"', "c7-mutation", *args]
    (directory / (label + ".command.json")).write_text(json.dumps(command, indent=2) + "\n")
    with (directory / (label + ".stdout")).open("w") as stdout, (directory / (label + ".stderr")).open("w") as stderr:
        result = subprocess.run(command, cwd=ROOT, env=ENV, stdout=stdout, stderr=stderr, timeout=600, check=False)
    output = (directory / (label + ".stdout")).read_text() + (directory / (label + ".stderr")).read_text()
    summaries = re.findall(r"^test result:.*$", output, re.M)
    failed = re.findall(r"^test (.+?) \.\.\. FAILED$", output, re.M)
    passed = re.findall(r"^test (.+?) \.\.\. ok$", output, re.M)
    if not summaries or "error[E" in output or (red and (result.returncode != 101 or not failed)) or (not red and (result.returncode != 0 or failed or not passed)):
        raise RuntimeError(f"{label}: not a runtime {'red' if red else 'green'}: exit={result.returncode}, summaries={summaries}; inspect {directory}")
    fixture_reds = []
    if red and directory.name == "legacy-verify-then-caller-path":
        if label == "red-0" and "alias target is never written" not in output:
            raise RuntimeError("alias mutant did not reach the intended full-graph assertion")
        if label == "red-1":
            binary = ROOT / "target/c7-build/debug/kinewright-ja-baseline"
            digest = hashlib.sha256(binary.read_bytes()).hexdigest()
            for mode in ["baseline", "revised"]:
                destination = directory / ("fixture-red-" + mode)
                command = ["bash", "-c", 'source scripts/setup-ffmpeg.sh >/dev/null; exec nice -n 10 "$@"', "c7-alias-fixture", str(binary), "--mode", mode, "--fixture", str(ROOT / "crates/kinewright-agent/tests/fixtures/ja/s2-writer-identity.json"), "--output", str(destination)]
                observed = subprocess.run(command, cwd=ROOT, env=ENV, text=True, capture_output=True, timeout=120, check=False)
                (directory / ("fixture-red-" + mode + ".stdout")).write_text(observed.stdout)
                (directory / ("fixture-red-" + mode + ".stderr")).write_text(observed.stderr)
                summary = json.loads((destination / "summary.json").read_text())
                controls = summary["positive_controls"]
                properties = [row for row in summary["assertions"] if row["id"] == "alias_preserved"]
                if observed.returncode != 1 or not controls or not all(row["passed"] for row in controls) or len(properties) != 1 or properties[0]["passed"] or properties[0]["disposition"] != "baseline_red":
                    raise RuntimeError("alias fixture red did not preserve its successful positive control")
                fixture_reds.append({"mode": mode, "exit": observed.returncode, "binary_sha256": digest, "positive_controls": controls, "property": properties[0]})
    return {"exit": result.returncode, "summaries": summaries, "passed": passed, "failed": failed, "fixture_reds": fixture_reds}


results = []
for name, edits, commands in ROWS:
    if ONLY and name not in ONLY:
        continue
    directory = OUT / name
    directory.mkdir(exist_ok=True)
    originals = {path: (ROOT / path).read_bytes() for path in edits}
    entry = {"mutation": name, "original_sha256": {path: hashlib.sha256(data).hexdigest() for path, data in originals.items()}, "pairs": []}
    try:
        for path, edit in edits.items():
            changed = edit(originals[path].decode()).encode()
            if changed == originals[path]:
                raise RuntimeError(f"mutation did not change {path}: {name}")
            (directory / (Path(path).name + ".original")).write_bytes(originals[path])
            (ROOT / path).write_bytes(changed)
        reds = [run(command, directory, f"red-{index}", True) for index, command in enumerate(commands)]
    finally:
        for path, original in originals.items():
            (ROOT / path).write_bytes(original)
    for index, command in enumerate(commands):
        green = run(command, directory, f"green-{index}", False)
        entry["pairs"].append({"args": command, "red": reds[index], "green": green})
    for path, original in originals.items():
        if (ROOT / path).read_bytes() != original:
            raise RuntimeError(f"source was not restored: {path}")
    (directory / "result.json").write_text(json.dumps(entry, indent=2) + "\n")
    results.append(entry)
    print(name + ": runtime red/restored green", flush=True)
(OUT / "results.json").write_text(json.dumps(results, indent=2) + "\n")

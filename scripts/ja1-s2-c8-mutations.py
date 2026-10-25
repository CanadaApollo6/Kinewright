#!/usr/bin/env python3
"""Targeted C8 runtime mutations. Run with nice -n 19 python3 -I after FFmpeg setup."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
A = "crates/kinewright-agent/src/writer_host/"
P = "crates/kinewright-project/src/"
HOST_TEST = "host_failure_preserves_originating_context_or_refuses_recovery"


def replace(text, before, after):
    if text.count(before) != 1:
        raise RuntimeError(f"mutation anchor is not unique: {before!r}")
    return text.replace(before, after, 1)


def host_case(name, changes, witness, env=None, red_text=None):
    return dict(name=name, changes=changes, package="kinewright-agent", kind="--test",
                target="ja_s2_host", features="kinewright-project/test-util", production=True,
                witness=witness, environment=env or {}, red_text=red_text)


def source_release_changes():
    """Introduce the prohibited release message and retire the real GUI reservation.

    The original protocol has no release authority. This mutant adds that authority
    across both real processes, then uses StorageReservation's actual Drop accounting.
    It does not alter the witness or manufacture a counter observation.
    """
    remote_drop = r'''impl Drop for RemoteSource {
    fn drop(&mut self) {
        let (Some(source), Some(inputs)) = (&self.binding.source, &self.binding.inputs) else { return; };
        let request = SourceRequest::Read { source: source.clone(), object: inputs.clone(), offset: u64::MAX, length: 0, instance: self.instance.clone() };
        let body = serde_json::to_vec(&request).expect("mutated source release packet");
        let bootstrap = &self.binding.callback;
        let mut stream = std::net::TcpStream::connect_timeout(&bootstrap.endpoint.address, Duration::from_secs(2)).expect("mutated source release connection");
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        write!(stream, "POST /source HTTP/1.1\r\nHost: {}\r\nAuthorization: {}\r\nx-kinewright-project: {}\r\nx-kinewright-protocol: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bootstrap.endpoint.address, bootstrap.auth.bearer(), bootstrap.endpoint.project, body.len()).unwrap();
        stream.write_all(&body).unwrap();
        let mut reply = Vec::new();
        stream.take(1 << 20).read_to_end(&mut reply).expect("mutated source release reply");
    }
}
'''
    release = r'''    pub(crate) fn release_charge_for_mutation(&mut self) {
        let charged = StorageReservation {
            value: self.value.take(), usage: Arc::clone(&self.usage), bytes: self.bytes,
            decoded: self.decoded, history: self.history, copy: self.copy, active: self.active,
        };
        self.active = false;
        drop(charged);
    }
'''
    return [
        (A + "mod.rs", "impl RemoteSource {", remote_drop + "impl RemoteSource {"),
        (A + "mod.rs", "                    if *length > SOURCE_READ_CHUNK_BYTES {",
         "                    if *offset == u64::MAX {\n"
         "                        reader.as_mut().ok_or(BundleError::Unsettled)?.release_copy_for_mutation()?;\n"
         "                        return Ok(reply);\n                    }\n"
         "                    if *length > SOURCE_READ_CHUNK_BYTES {"),
        (P + "source_boundary.rs", "pub trait BorrowedSourceReader {",
         "pub trait BorrowedSourceReader {\n"
         "    fn release_copy_for_mutation(&mut self) -> Result<(), BundleError> {\n"
         "        Err(BundleError::Identity(\"source release unavailable\".into()))\n    }"),
        (P + "source_boundary.rs", "impl BorrowedSourceReader for BorrowedSourceService {",
         "impl BorrowedSourceReader for BorrowedSourceService {\n"
         "    fn release_copy_for_mutation(&mut self) -> Result<(), BundleError> {\n"
         "        let retained = self.retained.upgrade().ok_or(BundleError::Unsettled)?;\n"
         "        retained.overlap.lock().map_err(|_| BundleError::Unsettled)?.reservation.release_charge_for_mutation();\n"
         "        Ok(())\n    }"),
        (P + "storage_budget.rs", "impl<T> StorageReservation<T> {", "impl<T> StorageReservation<T> {\n" + release),
    ]


def cases():
    auth = A + "auth.rs"
    host = A + "mod.rs"
    yield host_case("missing-and-wrong-token-admitted", [(host,
        'if !bootstrap.auth.accepts(value("authorization")) {', 'if false {')],
        "auth_and_project_protocol_guard_every_read_and_command")
    yield host_case("cross-project-header-admitted", [(host,
        'value("x-kinewright-project") != Some(bootstrap.endpoint.project.as_str())', 'false')],
        "auth_and_project_protocol_guard_every_read_and_command")
    yield host_case("wrong-protocol-header-admitted", [(host,
        'value("x-kinewright-protocol") != Some("1")', 'false')],
        "auth_and_project_protocol_guard_every_read_and_command")
    yield host_case("forwarded-root-scope-ignored", [(auth,
        '!forwarded.permits(&observed, write)', 'false')],
        "roots_and_forwarded_roots_cover_all_file_surfaces")
    yield host_case("replacement-instance-trusted", [(host,
        '.is_some_and(|original| original != instance)', '.is_some_and(|_| false)')],
        HOST_TEST, {"JA_C8_HOST_FAILURE_CASE": "orderly"},
        "a new host instance or reacquired lock cannot prove old retained ownership")
    yield host_case("source-subtree-separation-skipped", [(P + "store.rs",
        'ProjectStore::inspect_borrowed_destination(path, &[&store])',
        'ProjectStore::inspect_borrowed_destination(path, &[])')],
        HOST_TEST, {"JA_C8_HOST_FAILURE_CASE": "orderly"},
        "copy into the actual GUI source subtree was accepted")
    yield host_case("stdio-proxy-emits-nonprotocol-stdout", [(host,
        '            let response = self.post("mcp", &bytes)?;',
        '            output.write_all(b"host debug output\\n")?;\n            let response = self.post("mcp", &bytes)?;')],
        "stdio_proxy_preserves_auth_roots_and_protocol_stdout")
    yield host_case("gui-detach-retires-host-owned-session", [(host,
        '            HostCommand::Attach | HostCommand::Status | HostCommand::Detach => {}',
        '            HostCommand::Attach | HostCommand::Status => {}\n            HostCommand::Detach => { self.sessions.clear(); }')],
        "host_owns_scripted_session_after_gui_detach")
    yield host_case("host-admission-mutates-before-raw-refusal", [(host,
        '            let mut owner = Ja1EditingSession::open(&pin.io)?;',
        '            let _premature = kinewright_project::bundle_copy::SessionStore::create(path.parent().unwrap(), StorageLimits::default())?;\n            let mut owner = Ja1EditingSession::open(&pin.io)?;')],
        "w14_headless_and_agent_admission_precedes_mutation")
    yield host_case("host-save-releases-live-writer-authority", [(host,
        '                self.message = format!("Saved {}.", report.path.display());',
        '                self.owner_mut()?.prepare_host_attachment()?;\n                self.message = format!("Saved {}.", report.path.display());')],
        "g04_two_process_lock_save_alias_and_discovery")
    for mode in ("orderly", "unwind"):
        yield host_case(f"host-drop-deletes-gui-root-{mode}", [(host,
            'impl Drop for WriterHost {\n    fn drop(&mut self) {',
            'impl Drop for WriterHost {\n    fn drop(&mut self) {\n'
            '        if let Some(root) = std::env::var_os("JA_C8_SOURCE_ROOT") {\n'
            '            std::fs::remove_dir_all(root).expect("mutated host source cleanup");\n'
            '        }')], HOST_TEST, {"JA_C8_HOST_FAILURE_CASE": mode},
            "host Drop deleted the GUI temporary root")
        yield host_case(f"host-drop-uncharges-gui-source-{mode}", source_release_changes(),
            HOST_TEST, {"JA_C8_HOST_FAILURE_CASE": mode},
            "host Drop uncharged a GUI source reservation")
    yield from json.loads((ROOT / "scripts/ja1-s2-c8-mutation-cases.json").read_text())


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def run_case(case, evidence):
    directory = evidence / case["name"]
    directory.mkdir()
    paths = {name: (ROOT / name).read_bytes() for name, _, _ in case["changes"]}
    original = {name: data.decode() for name, data in paths.items()}
    changed = dict(original)
    for name, before, after in case["changes"]:
        changed[name] = replace(changed[name], before, after)
    write_json(directory / "source.json", {
        name: hashlib.sha256(data).hexdigest() for name, data in paths.items()})
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(("JA_", "KINEWRIGHT_", "REV2_"))}
    environment.update(CARGO_BUILD_JOBS="4", RUST_TEST_THREADS="4")
    environment.setdefault("CARGO_TARGET_DIR", str(ROOT / "target/c8-build"))
    environment.setdefault("TMPDIR", str(ROOT / "target/c8-tmp"))
    Path(environment["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    environment.update(case["environment"])
    command = ["nice", "-n", "10", "cargo", "test", "-j", "4", "-p", case["package"]]
    if case["production"]:
        command += ["--no-default-features"]
    if case["features"]:
        command += ["--features", case["features"]]
    command += [case["kind"]]
    if case["kind"] != "--lib":
        command += [case["target"]]
    command += [case["witness"], "--", "--exact", "--test-threads=1"]
    results = []
    try:
        for phase in ("red", "green"):
            for name in paths:
                (ROOT / name).write_text(changed[name] if phase == "red" else original[name])
            write_json(directory / f"{phase}-command.json", {"argv": command, "environment": case["environment"]})
            available = int(next(line.split()[1] for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemAvailable:")))
            if available < 6 * 1024 * 1024:
                raise RuntimeError("required 6 GiB available memory is unavailable")
            started = time.monotonic()
            with (directory / f"{phase}.stdout").open("w") as stdout, (directory / f"{phase}.stderr").open("w") as stderr:
                process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=stdout,
                                           stderr=stderr, start_new_session=True)
                try:
                    code = process.wait(timeout=600)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                    raise RuntimeError("mutation timed out; its owned process group was terminated and reaped") from None
            text = (directory / f"{phase}.stdout").read_text()
            summaries = re.findall(r"^test result:.*$", text, re.M)
            expected = (code == 101 and len(summaries) == 1
                        and "0 passed; 1 failed; 0 ignored" in summaries[0]) if phase == "red" else (
                        code == 0 and len(summaries) == 1
                        and "1 passed; 0 failed; 0 ignored" in summaries[0])
            if phase == "red" and case["red_text"]:
                expected = expected and case["red_text"] in text
            binaries = re.findall(r"^\s*Running .* \(([^()]+)\)$", (directory / f"{phase}.stderr").read_text(), re.M)
            if len(binaries) != 1:
                expected = False
            binary = (ROOT / binaries[0]).resolve() if len(binaries) == 1 else None
            supporting = Path(environment["CARGO_TARGET_DIR"]) / "debug" / (
                "kinewright.exe" if os.name == "nt" else "kinewright")
            row = dict(phase=phase, exit=code, summaries=summaries,
                       binary=str(binary) if binary else None,
                       binary_sha256=hashlib.file_digest(binary.open("rb"), "sha256").hexdigest() if binary and binary.is_file() else None,
                       supporting_host_binary=str(supporting) if case["package"] != "kinewright-project" and supporting.is_file() else None,
                       supporting_host_sha256=hashlib.file_digest(supporting.open("rb"), "sha256").hexdigest() if case["package"] != "kinewright-project" and supporting.is_file() else None,
                       seconds=time.monotonic() - started, accepted=bool(expected))
            write_json(directory / f"{phase}-result.json", row)
            results.append(row)
            print(case["name"], phase, code, summaries, flush=True)
            if not expected:
                raise RuntimeError(f"{case['name']} {phase} is not the required runtime outcome; retained logs explain it")
    finally:
        for name, data in paths.items():
            (ROOT / name).write_bytes(data)
        for name, data in paths.items():
            if (ROOT / name).read_bytes() != data:
                raise RuntimeError(f"source restoration failed: {name}")
    return dict(name=case["name"], witness=case["witness"], results=results)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--case", action="append", default=[])
    parser.add_argument("--group", choices=("host", "project", "app"))
    parser.add_argument("--check", action="store_true", help="validate mutation anchors without changing source")
    arguments = parser.parse_args()
    packages = {"host": "kinewright-agent", "project": "kinewright-project", "app": "kinewright-app"}
    chosen = [case for case in cases()
              if (not arguments.case or case["name"] in arguments.case)
              and (not arguments.group or case["package"] == packages[arguments.group])]
    if not chosen or set(arguments.case) - {case["name"] for case in chosen}:
        raise SystemExit("empty or unknown mutation selection")
    if arguments.check:
        for case in chosen:
            changed = {name: (ROOT / name).read_text() for name, _, _ in case["changes"]}
            for name, before, after in case["changes"]:
                changed[name] = replace(changed[name], before, after)
            print(case["name"] + ": anchors valid")
        return
    arguments.evidence.mkdir(parents=True, exist_ok=False)
    write_json(arguments.evidence / "cases.json", chosen)
    results = []
    for case in chosen:
        results.append(run_case(case, arguments.evidence))
        write_json(arguments.evidence / "results.json", results)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Run MS2's kill-chain, probe-bound and escape red witnesses, restore source, rerun controls.

Invoke after `source scripts/setup-ffmpeg.sh`, without an outer flock.
Only this checkout's audited source files (listed below) and its target
directory change; every one is restored, even when a run fails.

Escape removals run `escape_native`, which tries every attack in primary and
in degraded mode. Each removal must fail that test with the intended result
in both modes, and the restored controls must pass.
Probe-bound removals run the silent-helper witness in both modes; each must
end at that witness's named outer deadline.

Interruption: SIGINT, SIGTERM and SIGHUP stop the running cargo/test process
group, restore every audited file with those signals blocked, verify the files
against git and exit non-zero. SIGKILL cannot be handled: after a killed run,
check `git status`. The script refuses to start while an audited file differs
from git, so such a leftover is caught at the next run.
`scripts/ms2_s1a_mutations_selftest.py` witnesses the SIGTERM/SIGHUP path.
"""
from __future__ import annotations

import hashlib
import json
import contextlib
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target/ms2-s1a/kill-mutations"
BOUNDARY = ROOT / "crates/kinewright-job-sys/src/media_boundary.rs"
ACTOR = ROOT / "crates/kinewright-media-client/src/linux/actor.rs"
FILTER = ROOT / "crates/kinewright-job-sys/src/media_boundary/filter.rs"
HOST = ROOT / "crates/kinewright-job-sys/src/media_host.rs"
BOOTSTRAP = ROOT / "crates/kinewright-media-helper/src/bootstrap.rs"
AUDITED = (BOUNDARY, ACTOR, FILTER, HOST, BOOTSTRAP)
ESCAPE_TEST = "full_stack_escapes_are_killed_and_canaries_unchanged"
LOSS = "watchdog_disabled_degraded_app_and_supervisor_loss"
RACE = "watchdog_disabled_spawner_abort_and_payload_prearm_race"
SIGNAL = "u64::from(libc::SIGKILL.cast_unsigned()),"
CASES = [
    ("shim-parent-death", BOUNDARY, SIGNAL,
     "if expected == 0 { u64::from(libc::SIGKILL.cast_unsigned()) } else { 0 },",
     LOSS, "shim"),
    ("payload-parent-death", BOUNDARY, SIGNAL,
     "if expected != 0 { u64::from(libc::SIGKILL.cast_unsigned()) } else { 0 },",
     LOSS, "payload"),
    ("payload-parent-pidfd", BOUNDARY, "if let Some(parent) = parent {",
     "if let Some(parent) = parent.filter(|_| expected != 0) {", RACE, "pidfd"),
    ("spawner-abort-guard", ACTOR, "std::process::abort();",
     "std::hint::black_box(());", RACE, "abort"),
]
# A silent helper must get its typed failure within the design's probe bound,
# in both resource modes. Each removal leaves the held-cleanup caller waiting
# until the witness's named outer deadline fires. Without the actor deadline
# the worker's own read bound still answers the unheld probe; only the held
# cleanup shows that no host cause, and so no early answer, exists.
SILENT = "a_silent_helper_fails_typed_within_the_probe_bound"
BOUNDS = [
    ("probe-reap-budget", "&& job.answer_at.is_some_and(|deadline| now >= deadline)",
     "&& false"),
    ("probe-deadline", "} else if now >= job.deadline {", "} else if false {"),
]
MODES = ("primary", "degraded")

# Escape removals, outermost first. Each edit is (file, unique anchor, text).
SECCOMP = (BOOTSTRAP, "    boundary::install_filter(mode)?;\n", "")
SECCOMP_CHECK = (HOST, '        ("Seccomp:", "2"),\n', "")
# Landlock's restrict_self is what sets no_new_privs once seccomp is gone.
NNP_CHECK = (HOST, '        ("NoNewPrivs:", "1"),\n', "")
PIVOT = (BOUNDARY, """    fs::create_dir(root.join("old"))?;
    rustix::process::chdir(root)?;
    rustix::process::pivot_root(".", "old")?;
    rustix::process::chdir("/")?;
    mount::unmount("/old", UnmountFlags::DETACH)?;
    fs::remove_dir("/old")?;
""", "")
LANDLOCK = (BOOTSTRAP, "    let landlock = boundary::landlock();\n", "    let landlock = true;\n")
SOCKET = (FILTER, "        libc::SYS_clone3,\n    ] {",
          "        libc::SYS_clone3,\n        libc::SYS_socket,\n    ] {")
SOCKETS = ("inet", "unix", "netlink", "vsock")
ESCAPES = [
    # The host's boundary observation refuses a helper without seccomp.
    ("escape-seccomp", [SECCOMP], "refused"),
    # Lower layers deny files, spawn and signals with errno; sockets open.
    ("escape-seccomp-and-host-check", [SECCOMP, SECCOMP_CHECK], "sockets"),
    # With the pivot and Landlock gone too, the canary opens and /bin/sh runs.
    ("escape-seccomp-host-check-pivot-landlock",
     [SECCOMP, SECCOMP_CHECK, NNP_CHECK, PIVOT, LANDLOCK], "files"),
    # Only the socket denial is removed: sockets must be reported, every other
    # attack is still killed.
    ("escape-socket-denial-only", [SOCKET], "socket-only"),
]
ESCAPE_LINE = re.compile(r"MS2_ESCAPE requested=(\w+) attack=(\w+)(?: target=(\S*))? "
                         r"result=(\w+) (.*)$")
KILLED = "Err(Sandbox(Crashed))"
ESCAPED = "Err(Sandbox(ProtocolViolation))"


# Test hook for the interruption self-test: every cargo step becomes a long
# shell step with a grandchild in the same process group.
SELFTEST_STEP = "KW_MS2_MUTATIONS_SELFTEST_STEP"
HANDLED = (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)


class Interrupted(BaseException):
    """SIGTERM or SIGHUP, raised so every `finally` restore runs."""

    def __init__(self, signum: int):
        super().__init__(signal.Signals(signum).name)
        self.signum = signum


def interrupt(signum: int, _frame) -> None:
    raise Interrupted(signum)


@contextlib.contextmanager
def uninterruptible():
    """Hold SIGINT/SIGTERM/SIGHUP pending; a second signal waits until the end."""
    previous = signal.pthread_sigmask(signal.SIG_BLOCK, HANDLED)
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous)


def dirty(paths) -> list[str]:
    result = subprocess.run(["git", "diff", "--name-only", "--", *map(str, paths)],
                            cwd=ROOT, capture_output=True, text=True, check=True)
    return result.stdout.split()


def restore(originals: dict[Path, bytes]) -> None:
    with uninterruptible():
        for path, data in originals.items():
            path.write_bytes(data)


def stop_group(process: subprocess.Popen, command: list[str]) -> None:
    """Stop and reap the step's own process group (its leader is `process`)."""
    group = process.pid
    if process.poll() is None:
        # Signal only the group this script created: the leader must still be
        # the process it started, running the command it launched.
        cmdline = (Path(f"/proc/{group}/cmdline").read_bytes().split(b"\0")[:-1])
        if os.getpgid(group) != group or cmdline != [part.encode() for part in command]:
            print(f"MS2_MUTATIONS_GROUP_UNVERIFIED pid={group}", flush=True)
            process.wait()
            return
    # After the leader is reaped, Linux never reuses its PID while the group it
    # leads still has members, so the group ID still names only this step.
    for sig, wait in ((signal.SIGTERM, 10), (signal.SIGKILL, 10)):
        with contextlib.suppress(ProcessLookupError):
            os.killpg(group, sig)
        with contextlib.suppress(subprocess.TimeoutExpired):
            process.wait(timeout=wait)
        if process.poll() is not None and not group_alive(group):
            break
    process.wait()
    end = time.monotonic() + 10
    while group_alive(group) and time.monotonic() < end:
        time.sleep(0.05)
    print(f"MS2_MUTATIONS_GROUP_STOPPED pgid={group} gone={not group_alive(group)}",
          flush=True)


def group_alive(group: int) -> bool:
    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        return False
    return True


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def cargo(label: str, args: list[str], extra: tuple[str, ...] = ()) -> tuple[int, Path]:
    log = OUT / (label + ".log")
    command = ["flock", "-o", "/tmp/kinewright-cargo-heavy.lock", "env", *extra,
               "CARGO_BUILD_JOBS=4", "RUST_TEST_THREADS=1", "nice", "-n", "10",
               "cargo", "+1.99.1", *args]
    if os.environ.get(SELFTEST_STEP):
        command = ["sh", "-c", "sleep 600 & wait"]
    with log.open("w") as output:
        # Its own process group, so an interruption stops cargo, rustc and the
        # test binaries together.
        process = subprocess.Popen(command, cwd=ROOT, stdout=output,
                                   stderr=subprocess.STDOUT, process_group=0)
        print(f"MS2_MUTATIONS_STEP label={label} pid={process.pid} pgid={process.pid}",
              flush=True)
        try:
            code = process.wait()
        except BaseException:
            with uninterruptible():
                stop_group(process, command)
            raise
    print(f"{label}: exit={code}, log={log.relative_to(ROOT)}", flush=True)
    return code, log


def build(label: str) -> None:
    code, _ = cargo(label, ["build", "-p", "kinewright-media-helper", "-p",
                            "kinewright-media-supervisor", "--bins", "--features",
                            "kinewright-media-helper/slow-tests"])
    if code:
        raise RuntimeError("native witness build failed")


def expected_failure(text: str, kind: str) -> bool:
    if "test result: FAILED." not in text or "could not compile" in text:
        return False
    if kind == "pidfd":
        return "payload failed its own pre-arm pidfd check while the app was stopped" in text
    if kind == "abort":
        return "fixture did not finish native cleanup" in text
    target = re.search(r"MS2_KILL_TARGET mode=degraded stage=before-ready loss=app "
                       r"app=(\d+) shim=(\d+) payload=(\d+)", text)
    if not target:
        return False
    pid = target.group(2 if kind == "shim" else 3)
    return f"live or unreaped owned process {pid}" in text


def mode_env(mode: str) -> tuple[str, ...]:
    # Degraded mode: a session bus address with nothing behind it.
    if mode == "degraded":
        return (f"DBUS_SESSION_BUS_ADDRESS=unix:path={OUT / 'no-bus'}",)
    return ()


def silent_test(label: str, mode: str) -> tuple[int, Path]:
    return cargo(label, ["test", "-p", "kinewright-media-client", "--features",
                         "slow-tests", "--test", "witness_native", SILENT, "--",
                         "--exact", "--nocapture"], mode_env(mode))


def bound_failure(text: str, mode: str) -> bool:
    if "could not compile" in text or "MS2_TEST_DEADLINE silent helper probe bound" not in text:
        return False
    if ("MS2_RESOURCE_MODE degraded" in text) != (mode == "degraded"):
        return False
    unheld = re.search(r"MS2_SILENT_PROBE held=false degraded=(true|false) .*"
                       r"outcome=Err\(Sandbox\(TimedOut\)\)", text)
    # The unheld probe still ends; only the held cleanup strands the caller.
    return bool(unheld) and "MS2_SILENT_PROBE held=true" not in text


def escape_results(text: str) -> dict[tuple[str, str, str], tuple[str, str]]:
    """(requested mode, attack, short target) -> (actual mode, result)."""
    results = {}
    for line in text.splitlines():
        found = ESCAPE_LINE.search(line)
        if not found:
            continue
        requested, attack, target, actual, result = found.groups()
        target = target or ""
        for suffix, short in (("/canary.txt", "canary"), ("/source.png", "source")):
            if target.endswith(suffix):
                target = short
        if target.startswith("/proc/") and target.endswith("/mem"):
            target = "app-mem"
        elif target.isdigit():
            target = "app"
        results[(requested, attack, target)] = (actual, result)
    return results


def escape_failure(text: str, kind: str) -> list[str]:
    """Why this red run is not the intended failure; empty when it is."""
    if "test result: FAILED." not in text or "could not compile" in text:
        return ["the escape test did not fail"]
    if "escape witnesses failed" not in text:
        return ["the escape test failed outside its collected assertion"]
    results = escape_results(text)
    problems = []
    for mode in ("primary", "degraded"):
        own = {key[1:]: value for key, value in results.items() if key[0] == mode}
        if len(own) != 13:
            problems.append(f"{mode}: {len(own)} of 13 results")
            continue
        if any(actual != mode for actual, _ in own.values()):
            problems.append(f"{mode}: ran in another mode")
        control = own[("control", "")][1]

        def result(attack: str, target: str) -> str:
            return own[(attack, target)][1]

        if kind == "refused":
            if control != "Err(Sandbox(SandboxUnavailable))":
                problems.append(f"{mode}: control {control}, expected host refusal")
            continue
        if control != "Ok(Image)":
            problems.append(f"{mode}: control {control}")
        if kind in ("sockets", "socket-only"):
            for family in SOCKETS:
                if result("socket", family) != ESCAPED:
                    problems.append(f"{mode}: socket {family} {result('socket', family)}")
        if kind == "socket-only":
            for (attack, target), (_, value) in own.items():
                if attack not in ("control", "socket") and value != KILLED:
                    problems.append(f"{mode}: {attack} {target} {value}, expected a kill")
        if kind == "files":
            for attack, target in (("read", "canary"), ("spawn", "/bin/sh")):
                if result(attack, target) != ESCAPED:
                    problems.append(f"{mode}: {attack} {target} {result(attack, target)}")
    return problems


def escape_test(label: str) -> tuple[int, Path]:
    return cargo(label, ["test", "-p", "kinewright-media-client", "--features", "slow-tests",
                         "--test", "escape_native", "--", "--exact", ESCAPE_TEST,
                         "--nocapture"])


def apply(edits: list[tuple[Path, str, str]], originals: dict[Path, bytes], name: str) -> None:
    current = {path: originals[path].decode() for path, _, _ in edits}
    for path, before, after in edits:
        if current[path].count(before) != 1:
            raise RuntimeError(f"{name}: source anchor is not unique in {path.name}")
        current[path] = current[path].replace(before, after)
    for path, text in current.items():
        path.write_text(text)


def main() -> None:
    if not os.environ.get("FFMPEG_DIR") and not os.environ.get(SELFTEST_STEP):
        raise SystemExit("source scripts/setup-ffmpeg.sh first; do not add an outer flock")
    for signum in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(signum, interrupt)
    left = dirty(AUDITED)
    if left:
        print(f"MS2_MUTATIONS_REFUSED dirty audited source: {' '.join(left)}", flush=True)
        raise SystemExit(2)
    OUT.mkdir(parents=True, exist_ok=True)
    originals = {path: path.read_bytes() for path in AUDITED}
    rows: list[dict] = []
    escapes: list[dict] = []
    controls = None
    escape_controls = None
    interrupted = False
    try:
        for name, path, before, after, test, kind in CASES:
            source = originals[path].decode()
            if source.count(before) != 1:
                raise RuntimeError(f"{name}: source anchor is not unique")
            path.write_text(source.replace(before, after))
            try:
                build(name + "-build")
                helper_hash = digest(ROOT / "target/debug/kinewright-media-helper-witness")
                code, log = cargo(name + "-red", ["test", "-p", "kinewright-media-client",
                    "--features", "slow-tests", "--test", "witness_native", test,
                    "--", "--exact", "--nocapture"])
                rejected = code != 0 and expected_failure(log.read_text(), kind)
                rows.append({"mutation": name, "test": test, "exit": code,
                             "expected_assertion_failed": rejected,
                             "helper_sha256": helper_hash,
                             "log": str(log.relative_to(ROOT)), "log_sha256": digest(log)})
                if not rejected:
                    raise RuntimeError(f"{name}: mutation did not fail at its intended assertion")
            finally:
                restore(originals)
        source = originals[ACTOR].decode()
        for name, before, after in BOUNDS:
            if source.count(before) != 1:
                raise RuntimeError(f"{name}: source anchor is not unique")
            ACTOR.write_text(source.replace(before, after))
            try:
                for mode in MODES:
                    code, log = silent_test(f"{name}-{mode}-red", mode)
                    rejected = code != 0 and bound_failure(log.read_text(), mode)
                    rows.append({"mutation": name, "mode": mode, "test": SILENT, "exit": code,
                                 "expected_assertion_failed": rejected,
                                 "log": str(log.relative_to(ROOT)), "log_sha256": digest(log)})
                    if not rejected:
                        raise RuntimeError(f"{name} {mode}: did not fail at its outer deadline")
            finally:
                restore(originals)
        for name, edits, kind in ESCAPES:
            try:
                apply(edits, originals, name)
                build(name + "-build")
                helper_hash = digest(ROOT / "target/debug/kinewright-media-helper-witness")
                code, log = escape_test(name + "-red")
                text = log.read_text()
                problems = escape_failure(text, kind) if code else ["the escape test passed"]
                results = escape_results(text)
                escapes.append({
                    "mutation": name, "exit": code, "expected_assertion_failed": not problems,
                    "problems": problems,
                    "results": {f"{m} {a} {t}".strip(): f"{actual} {value}"
                                for (m, a, t), (actual, value) in sorted(results.items())},
                    "helper_sha256": helper_hash,
                    "log": str(log.relative_to(ROOT)), "log_sha256": digest(log)})
                if problems:
                    raise RuntimeError(f"{name}: escape removal did not fail as intended: "
                                       f"{problems}")
            finally:
                restore(originals)
    except (Interrupted, KeyboardInterrupt):
        interrupted = True
        raise
    finally:
        restore(originals)
        left = dirty(AUDITED)
        if left:
            print(f"MS2_MUTATIONS_RESTORE_FAILED differs from git: {' '.join(left)}",
                  flush=True)
            raise SystemExit(3)
        print(f"MS2_MUTATIONS_RESTORED files={len(AUDITED)}", flush=True)
        if not interrupted:
            # Never leave a mutant executable as the checkout's helper, including
            # after an unexpected red result (an interruption skips the controls).
            build("restored-build")
            code, log = cargo("restored-controls", ["test", "-p", "kinewright-media-client",
                "--features", "slow-tests", "--test", "witness_native", "--", "--show-output"])
            controls = {"exit": code, "log": str(log.relative_to(ROOT)),
                        "log_sha256": digest(log)}
            bound_controls = []
            for mode in MODES:
                code, log = silent_test(f"restored-bound-{mode}", mode)
                text = log.read_text()
                bound_controls.append({
                    "mode": mode, "exit": code, "log": str(log.relative_to(ROOT)),
                    "log_sha256": digest(log),
                    "mode_matches": ("MS2_RESOURCE_MODE degraded" in text) == (mode == "degraded")})
            code, log = escape_test("restored-escape-controls")
            results = escape_results(log.read_text())
            escape_controls = {"exit": code, "log": str(log.relative_to(ROOT)),
                               "log_sha256": digest(log),
                               "modes": sorted({actual for actual, _ in results.values()}),
                               "results": len(results)}
            report = {"mutations": rows, "restored_controls": controls,
                      "restored_bound_controls": bound_controls,
                      "escape_mutations": escapes, "restored_escape_controls": escape_controls,
                      "source_restored": all(path.read_bytes() == data for path, data in originals.items())}
            (OUT / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    if controls["exit"] != 0:
        raise SystemExit("restored native controls failed")
    if any(row["exit"] != 0 or not row["mode_matches"] for row in bound_controls):
        raise SystemExit("restored probe-bound controls failed")
    if escape_controls["exit"] != 0 or escape_controls["modes"] != ["degraded", "primary"] \
            or escape_controls["results"] != 26:
        raise SystemExit(f"restored escape controls failed: {escape_controls}")


if __name__ == "__main__":
    try:
        main()
    except Interrupted as stop:
        # The source is restored and verified by now; the controls did not run,
        # so the next cargo build rebuilds the helper from the restored files.
        print(f"MS2_MUTATIONS_INTERRUPTED signal={stop} source_restored=yes", flush=True)
        sys.exit(128 + stop.signum)

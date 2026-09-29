#!/usr/bin/env python3
"""Slow-tier tooling for CI and local runs (standard library only).

    python3 scripts/slow_tests.py lint          manifest <-> source markers agree; allowlist
                                                format (a cheap early check only)
    python3 scripts/slow_tests.py fast          cargo test --workspace, then check the
                                                manifest was skipped and nothing else
                                                unexpected (ci/ignored-tests.txt) was
    python3 scripts/slow_tests.py slow          run exactly the manifest, check all
                                                of it ran and passed
    python3 scripts/slow_tests.py verify-fast FILE [--os linux|windows]
                                                the check `fast` applies, on a saved log
    python3 scripts/slow_tests.py verify-slow FILE   the check `slow` applies, on a saved log
    python3 scripts/slow_tests.py features      print the --features list for the slow tier

The manifest is ci/slow-tests.txt; ci/ignored-tests.txt allowlists the hardware,
audio-device, live-subscription, manual and on-demand tests, each with the OS it
applies to (any, linux or windows). The authoritative check is the compiled run's own
output: on each OS the fast tier must have skipped exactly the manifest plus the
allowlist entries for that OS, every one observed as ignored under its exact binary
and test path (doctests too), and nothing else. A stale, fabricated or conditionally
ignored entry is never observed ignored on every OS it claims, so it fails. A slow
test carries

    #[cfg_attr(not(feature = "slow-tests"), ignore = "slow tier: cargo test --features slow-tests")]

which makes a plain `cargo test` skip it. Enabling the crate's `slow-tests`
feature makes it an ordinary test again, so `cargo test -- --ignored` would
run only the hardware (NVIDIA) lanes; the slow tier therefore selects its
tests by exact name from the manifest instead.

Cargo is run as `cargo`; a caller that needs `nice`, `-j` or other cargo
arguments passes them after `--` (`slow_tests.py fast -- -j 4`), and libtest
arguments come from the environment (RUST_TEST_THREADS).
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "ci" / "slow-tests.txt"
IGNORED = ROOT / "ci" / "ignored-tests.txt"
REASON = "slow tier: cargo test --features slow-tests"

# The marker, tolerant of rustfmt wrapping it over several lines.
MARKER = re.compile(
    r'#\[cfg_attr\(\s*not\(feature = "slow-tests"\),\s*'
    r'ignore = "' + re.escape(REASON) + r'"\s*,?\s*\)\]'
)
ATTRIBUTE_OR_COMMENT = re.compile(r"\s*(?:#\[[^\]]*\]|//[^\n]*)")
FN_NAME = re.compile(r"\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)")
MACRO_ENTRY = re.compile(r"\s*(\w+)\s*=>")
# cargo prints `Running unittests src/lib.rs (target/debug/deps/x-<hash>)`; requiring the
# parenthesised executable keeps unrelated text such as "Running kernel ..." out.
RUNNING = re.compile(r"^\s*Running (?:unittests |tests[/\\])?(.*\([^)]*\))\s*$")
# A doctest name contains spaces (`path/lib.rs - item (line 3)`), and a should_panic
# test prints `name - should panic`; the outcome starts at the first " ... ".
TEST_LINE = re.compile(r"^test (.+?) \.\.\. (.*)$")
# FFmpeg's av_log writes `[name @ 0xADDR] ` to the (uncaptured) native stderr before its
# message, sometimes without a newline, so a libtest line can land after it on the same line
# (CI run 36536620605). Only this exact prefix shape is stripped; any other foreign text still
# breaks the per-section counts and fails the parse.
FFMPEG_LOG = r"\[[^\]\s]+ @ (?:0x)?[0-9a-fA-F]+\]"  # Windows prints the address without 0x
FFMPEG_LOG_PREFIX = re.compile(r"^(?:" + FFMPEG_LOG + r" )+")
# On a result line a fragment can also sit before the outcome, and a message can trail the
# outcome (Windows run 36536620605). The tail is cut only after a recognised outcome word, so a
# fragment can never swallow the outcome itself.
FFMPEG_LOG_SUFFIX = re.compile(FFMPEG_LOG + r" .*$")
OUTCOME = re.compile(r"^(?:ok|FAILED|ignored)")
DOC_TESTS = re.compile(r"^\s*Doc-tests (\S+)")


def strip_ffmpeg_log(line: str) -> str:
    """`line` without FFmpeg log fragments at its start, before a result's outcome, or after it."""
    line = FFMPEG_LOG_PREFIX.sub("", line)
    head, sep, rest = line.partition(" ... ")
    if not sep or not head.startswith("test "):
        return line
    rest = FFMPEG_LOG_PREFIX.sub("", rest)
    outcome = OUTCOME.match(rest)
    if outcome:
        rest = rest[: outcome.end()] + FFMPEG_LOG_SUFFIX.sub("", rest[outcome.end():])
    return head + sep + rest


OUTCOME_LINE = re.compile(r"^(?:ok|FAILED|ignored(?:, .*)?)$")


def result_lines(log: str):
    """(line number, line) of `log` with FFmpeg fragments stripped (strip_ffmpeg_log), and a
    result split by a whole FFmpeg message rejoined.

    A complete message, newline included, can land after `test name ... `, which pushes the
    outcome to a line of its own (pf1 R43 gate log: `test x ... [swscaler @ 0x..] No accelerated
    colorspace conversion ...` then `ok`). The outcome is taken from the next line that is not
    itself an FFmpeg message, and only if that line is nothing but an outcome; otherwise the
    result keeps an empty outcome and the section counts reject it.
    """
    pending = None  # (number, "test name ... ") awaiting its outcome
    for number, raw in enumerate(log.splitlines(), 1):
        line = strip_ffmpeg_log(raw)
        if pending is not None:
            if re.match(FFMPEG_LOG, raw) and not OUTCOME_LINE.match(line):
                continue
            if OUTCOME_LINE.match(line):
                yield pending[0], pending[1] + line
                pending = None
                continue
            yield pending
            pending = None
        head, sep, rest = FFMPEG_LOG_PREFIX.sub("", raw).partition(" ... ")
        if (
            sep
            and head.startswith("test ")
            and re.match(FFMPEG_LOG, rest)
            and not OUTCOME.match(FFMPEG_LOG_PREFIX.sub("", rest))
        ):
            pending = (number, head + sep)
            continue
        yield number, line
    if pending is not None:
        yield pending


def read_entries(path: Path) -> list[tuple[str, str, str]]:
    """`<binary> <test path> <source file>` lines; the path may not contain `#`."""
    entries = []
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        # A doctest path has spaces, so split off the binary and the file only.
        binary, _, rest = line.partition(" ")
        test_path, _, source_file = rest.rpartition(" ")
        if not binary or not test_path or not source_file:
            sys.exit(f"{path}:{number}: want '<binary> <test path> <source file>', got {raw!r}")
        entries.append((binary, test_path, source_file))
    seen = set()
    for entry in entries:
        if entry[:2] in seen:
            sys.exit(f"{path}: duplicate entry {entry[0]} {entry[1]}")
        seen.add(entry[:2])
    return entries


def read_manifest() -> list[tuple[str, str, str]]:
    return read_entries(MANIFEST)


CATEGORIES = ("hardware", "on-demand", "inherited-gap")
OSES = ("any", "linux", "windows")


def read_allowlist() -> list[dict]:
    """`<binary> <test path> <source file> <os> <category>  # reason` lines of ci/ignored-tests.txt.

    The test path may contain spaces (a doctest name), so the binary is the first
    token and the file, os and category are the last three.
    """
    entries = []
    for number, raw in enumerate(IGNORED.read_text(encoding="utf-8").splitlines(), 1):
        code, _, reason = raw.partition("#")
        code = code.strip()
        if not code:
            continue
        binary, _, rest = code.partition(" ")
        parts = rest.rsplit(None, 3)
        if not binary or len(parts) != 4:
            sys.exit(
                f"{IGNORED}:{number}: want '<binary> <test path> <source file> <os> <category>', got {raw!r}"
            )
        entries.append(
            {
                "binary": binary,
                "path": parts[0],
                "file": parts[1],
                "os": parts[2],
                "category": parts[3],
                "reason": reason.strip(),
                "line": number,
            }
        )
    seen = set()
    for entry in entries:
        key = (entry["binary"], entry["path"])
        if key in seen:
            sys.exit(f"{IGNORED}: duplicate entry {key[0]} {key[1]}")
        seen.add(key)
    return entries


def host_os() -> str:
    if sys.platform.startswith("win"):
        return "windows"
    if sys.platform.startswith("linux"):
        return "linux"
    sys.exit(f"unsupported platform {sys.platform!r}: the allowlist knows only linux and windows")


def allowlist_for(os_name: str) -> set[tuple[str, str]]:
    """(binary, test path) of every entry that must be observed ignored on `os_name`."""
    return {(e["binary"], e["path"]) for e in read_allowlist() if e["os"] in ("any", os_name)}


def read_ignored() -> list[tuple[str, str, str]]:
    return [(e["binary"], e["path"], e["file"]) for e in read_allowlist()]


def marked_tests() -> set[tuple[str, str]]:
    """(source file, function name) of every slow-tier marker under crates/."""
    found = set()
    total_reasons = 0
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        if "target" in path.relative_to(ROOT).parts:
            continue
        text = path.read_text(encoding="utf-8")
        total_reasons += text.count('"slow tier:')
        for match in MARKER.finditer(text):
            position = match.end()
            while True:
                skipped = ATTRIBUTE_OR_COMMENT.match(text, position)
                if not skipped:
                    break
                position = skipped.end()
            named = FN_NAME.match(text, position) or MACRO_ENTRY.match(text, position)
            rel = path.relative_to(ROOT).as_posix()
            if not named:
                sys.exit(f"{rel}: a slow-tier marker is not followed by a test function")
            found.add((rel, named.group(1)))
    marker_count = sum(
        len(MARKER.findall(p.read_text(encoding="utf-8")))
        for p in (ROOT / "crates").rglob("*.rs")
        if "target" not in p.relative_to(ROOT).parts
    )
    if marker_count != total_reasons:
        sys.exit(
            f"{total_reasons} 'slow tier:' reasons but {marker_count} well-formed markers: "
            f"a marker was retyped, so it would not be recognised"
        )
    return found


def crate_of(source_file: str) -> str:
    parts = Path(source_file).parts
    if len(parts) < 2 or parts[0] != "crates":
        sys.exit(f"{source_file} is not under crates/")
    return parts[1]


def feature_list(entries) -> str:
    return ",".join(sorted({f"{crate_of(f)}/slow-tests" for _, _, f in entries}))


def lint_ignored(problems: list[str], slow_keys: set[tuple[str, str]]) -> None:
    """Cheap early checks on the allowlist's format. Not the authority on its truth.

    Whether an entry is really an ignored test is established by `verify-fast`, on the
    compiled run's output, on every OS: an entry the run does not report as ignored
    (a stale name, a conditional ignore, a fabricated doctest) fails there.
    """
    for entry in read_allowlist():
        binary, path, file = entry["binary"], entry["path"], entry["file"]
        where = f"ci/ignored-tests.txt:{entry['line']}: {binary} {path}"
        if entry["os"] not in OSES:
            problems.append(f"{where}: os {entry['os']!r} is not one of {', '.join(OSES)}")
        if entry["category"] not in CATEGORIES:
            problems.append(
                f"{where}: category {entry['category']!r} is not one of {', '.join(CATEGORIES)}"
            )
        if entry["category"] == "inherited-gap" and not entry["reason"]:
            problems.append(f"{where}: an inherited-gap entry needs a one-line reason after '#'")
        if (binary, path) in slow_keys:
            problems.append(f"{where}: also in ci/slow-tests.txt")
        if not (ROOT / file).is_file():
            problems.append(f"{where}: {file} does not exist")


def lint() -> int:
    entries = read_manifest()
    listed = {(f, path.rsplit("::", 1)[-1]) for _, path, f in entries}
    marked = marked_tests()
    problems = []
    for file, name in sorted(listed - marked):
        problems.append(f"in the manifest but not marked slow: {name} ({file})")
    for file, name in sorted(marked - listed):
        problems.append(f"marked slow but not in {MANIFEST.relative_to(ROOT)}: {name} ({file})")
    for crate in sorted({crate_of(f) for _, _, f in entries} | {crate_of(f) for f, _ in marked}):
        manifest_toml = (ROOT / "crates" / crate / "Cargo.toml").read_text(encoding="utf-8")
        if not re.search(r"^slow-tests\s*=\s*\[\s*\]", manifest_toml, re.M):
            problems.append(f"crates/{crate}/Cargo.toml has no `slow-tests = []` feature")
    for file, name in sorted(marked):
        text = (ROOT / file).read_text(encoding="utf-8")
        if not re.search(rf"\bfn\s+{re.escape(name)}\b|\b{re.escape(name)}\s*=>", text):
            problems.append(f"{file} has no {name}")
    lint_ignored(problems, {(b, p) for b, p, _ in entries})
    if problems:
        print("slow-test manifest, ignored-test allowlist and markers disagree:")
        for problem in problems:
            print("  " + problem)
        return 1
    print(
        f"slow-test manifest and markers agree: {len(entries)} tests, "
        f"features {feature_list(entries)}; {len(read_ignored())} allowlist entries well-formed"
    )
    return 0


def binary_label(running_target: str) -> str:
    """`src/lib.rs (target/debug/deps/kinewright_media-0af...)` -> kinewright_media."""
    inner = re.search(r"\(([^)]*)\)", running_target)
    exe = (inner.group(1) if inner else running_target).replace("\\", "/").rsplit("/", 1)[-1]
    exe = re.sub(r"\.exe$", "", exe)
    return re.sub(r"-[0-9a-f]{16}$", "", exe)


ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
UNPARSABLE = "could not parse cargo test output"
RUNNING_COUNT = re.compile(r"^running (\d+) tests?$")
SUMMARY = re.compile(
    r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; "
    r"(\d+) measured; (\d+) filtered out\b"
)


class UnparsableLog(Exception):
    pass


def parse(log: str) -> list[tuple[str, str, str]]:
    """(binary, test path, outcome) for every `test x ... y` line, after stripping ANSI.

    The output is a series of sections, each `Running ...` or `Doc-tests ...`, then
    `running N tests`, the test lines, and libtest's `test result:` summary. The result
    is trusted only if every test line sits inside a section and every section's own
    counts (tests seen, passed, failed, ignored) equal its summary: a lost header, an
    orphan test line, a dropped line or a repeated
    `running N tests` then raises UnparsableLog instead of letting
    tests be attributed to the wrong binary or silently vanish.
    """
    rows: list[tuple[str, str, str]] = []
    section = None  # {"binary", "rows", "running"} while a section is open
    sections = 0

    def fail(number: int, message: str):
        raise UnparsableLog(f"line {number}: {message}")

    for number, line in result_lines(ANSI.sub("", log)):
        running = RUNNING.match(line)
        doc = DOC_TESTS.match(line)
        if running or doc:
            if section is not None:
                fail(number, f"{section['binary']} has no `test result:` summary before the next section")
            binary = binary_label(running.group(1)) if running else f"doc-tests:{doc.group(1)}"
            section = {"binary": binary, "rows": [], "running": None}
            sections += 1
            continue
        count = RUNNING_COUNT.match(line)
        if count:
            if section is None:
                fail(number, "`running N tests` outside any section")
            if section["running"] is not None or section["rows"]:
                fail(number, f"{section['binary']}: second `running N tests`, or one after its test lines")
            section["running"] = int(count.group(1))
            continue
        test = TEST_LINE.match(line)
        if test:
            if section is None:
                fail(number, f"test result outside any `Running`/`Doc-tests` section: {line.strip()}")
            section["rows"].append((section["binary"], test.group(1), test.group(2).strip()))
            continue
        summary = SUMMARY.match(line)
        if summary:
            if section is None:
                fail(number, "`test result:` summary outside any section")
            passed, failed, ignored, measured, _filtered = map(int, summary.groups())
            seen = section["rows"]
            seen_counts = (
                sum(1 for _, _, o in seen if o == "ok"),
                sum(1 for _, _, o in seen if o.startswith("FAILED")),
                sum(1 for _, _, o in seen if o.startswith("ignored")),
                sum(1 for _, _, o in seen if o.startswith("bench")),
            )
            if seen_counts != (passed, failed, ignored, measured):
                fail(
                    number,
                    f"{section['binary']}: saw {seen_counts[0]} passed, {seen_counts[1]} failed, "
                    f"{seen_counts[2]} ignored, {seen_counts[3]} measured, "
                    f"but its summary says {passed}, {failed}, {ignored}, {measured}",
                )
            if section["running"] is None:
                fail(number, f"{section['binary']}: no `running N tests` line before its summary")
            if section["running"] != len(seen):
                fail(number, f"{section['binary']}: `running {section['running']} tests` but {len(seen)} test lines")
            rows.extend(seen)
            section = None
    if section is not None:
        raise UnparsableLog(f"{section['binary']} has no `test result:` summary (truncated output?)")
    if not sections:
        raise UnparsableLog("no `Running` or `Doc-tests` line found")
    if not rows:
        raise UnparsableLog(f"{sections} sections but no `test ... ok|ignored` line found")
    return rows


def verify_fast(log: str, os_name: str | None = None) -> int:
    """The authoritative check of the skipped set, on the compiled run's own output.

    On `os_name` (default: this machine) the run must have skipped exactly the manifest
    (as slow) plus the allowlist entries that apply to that OS (as plain ignores), each
    under its exact binary and test path, doctests included. An allowlist entry the run
    does not report as ignored (stale, misnamed, fabricated, or only conditionally
    ignored on another OS) fails, as does any skip the two files do not name.
    """
    os_name = os_name or host_os()
    try:
        rows = parse(log)
    except UnparsableLog as error:
        return unparsable(error)
    expected = {(b, p) for b, p, _ in read_manifest()}
    allowed = allowlist_for(os_name)
    ignored = {(b, p, outcome) for b, p, outcome in rows if outcome.startswith("ignored")}
    skipped = {(b, p) for b, p, outcome in ignored if REASON in outcome}
    others = {(b, p): outcome for b, p, outcome in ignored if REASON not in outcome}
    problems = [f"not skipped by the fast tier: {b} {p}" for b, p in sorted(expected - skipped)]
    problems += [f"skipped as slow but not in the manifest: {b} {p}" for b, p in sorted(skipped - expected)]
    problems += [
        f"allowlisted but not observed as ignored on {os_name}: {b} {p}"
        for b, p in sorted(allowed - set(others))
    ]
    problems += [
        f"unexpected skip on {os_name}, not in ci/slow-tests.txt and not in ci/ignored-tests.txt "
        f"for {os_name}: {b} {p} ({outcome})"
        for (b, p), outcome in sorted(others.items())
        if (b, p) not in allowed
    ]
    if problems:
        print("fast tier and manifest disagree:")
        print("\n".join("  " + p for p in problems))
        return 1
    print(
        f"fast tier on {os_name} skipped exactly the {len(expected)} manifest tests and "
        f"{len(others)} allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else"
    )
    return 0


def unparsable(error: UnparsableLog) -> int:
    print(
        f"{UNPARSABLE}: {error}. The test output format may have changed "
        "(ANSI colour, --format, -q, --nocapture); this is not a report about missing skips."
    )
    return 1


def verify_slow(log: str) -> int:
    try:
        rows = parse(log)
    except UnparsableLog as error:
        return unparsable(error)
    expected = {(b, p) for b, p, _ in read_manifest()}
    outcomes = {}
    for b, p, outcome in rows:
        outcomes.setdefault((b, p), []).append(outcome)
    ran = {key for key in outcomes if any(o == "ok" for o in outcomes[key])}
    problems = [f"did not run (or did not pass): {b} {p} {outcomes.get((b, p), '')}" for b, p in sorted(expected - ran)]
    problems += [f"ran but is not in the manifest: {b} {p}" for b, p in sorted(set(outcomes) - expected)]
    problems += [
        f"more than one binary result for {b} {p}"
        for (b, p), seen in sorted(outcomes.items())
        if len(seen) != 1
    ]
    if problems:
        print("slow tier and manifest disagree:")
        print("\n".join("  " + p for p in problems))
        return 1
    print(f"slow tier ran and passed exactly the {len(expected)} manifest tests")
    return 0


def run_cargo(args: list[str]) -> tuple[int, str]:
    print("+ " + " ".join(args), flush=True)
    # dtolnay/rust-toolchain exports CARGO_TERM_COLOR=always; the output is parsed, so ask
    # for plain text. (parse() also strips ANSI, for saved logs and other callers.)
    env = {**os.environ, "CARGO_TERM_COLOR": "never"}
    process = subprocess.Popen(
        args,
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    captured = []
    assert process.stdout is not None
    for line in process.stdout:
        sys.stdout.write(line)
        captured.append(line)
    return process.wait(), "".join(captured)


def split_extra(argv: list[str]) -> list[str]:
    return argv[argv.index("--") + 1 :] if "--" in argv else []


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2
    command = argv[1]
    extra = split_extra(argv)
    if command == "lint":
        return lint()
    if command == "features":
        print(feature_list(read_manifest()))
        return 0
    if command in ("verify-fast", "verify-slow"):
        log = Path(argv[2]).read_text(encoding="utf-8", errors="replace")
        if command == "verify-slow":
            return verify_slow(log)
        os_name = argv[argv.index("--os") + 1] if "--os" in argv else None
        if os_name not in (None, "linux", "windows"):
            sys.exit("--os must be linux or windows")
        return verify_fast(log, os_name)
    if command == "fast":
        code, log = run_cargo(["cargo", "test", "--workspace", *extra])
        if code != 0:
            print(f"cargo test --workspace exited {code}")
            return code
        return verify_fast(log)
    if command == "slow":
        entries = read_manifest()
        names = [path for _, path, _ in entries]
        cargo = [
            "cargo", "test", "--workspace",
            "--features", feature_list(entries),
            "--lib", "--bins", "--tests",
            *extra,
            "--", "--exact", *names,
        ]
        code, log = run_cargo(cargo)
        if code != 0:
            print(f"the slow tier's cargo test exited {code}")
            return code
        return verify_slow(log)
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))

#!/usr/bin/env python3
"""Slow-tier tooling for CI and local runs (standard library only).

    python3 scripts/slow_tests.py lint          manifest <-> source markers agree
    python3 scripts/slow_tests.py fast          cargo test --workspace, then check the
                                                manifest was skipped and nothing else
                                                unexpected (ci/ignored-tests.txt) was
    python3 scripts/slow_tests.py slow          run exactly the manifest, check all
                                                of it ran and passed
    python3 scripts/slow_tests.py verify-fast FILE   the check `fast` applies, on a saved log
    python3 scripts/slow_tests.py verify-slow FILE   the check `slow` applies, on a saved log
    python3 scripts/slow_tests.py features      print the --features list for the slow tier

The manifest is ci/slow-tests.txt; ci/ignored-tests.txt allowlists the hardware,
audio-device, live-subscription, manual and on-demand tests that carry a plain
`#[ignore]`. The fast tier fails on any other skipped test. A slow test carries

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
RUNNING = re.compile(r"^\s*Running (?:unittests |tests[/\\])?(.*)$")
# A doctest name contains spaces (`path/lib.rs - item (line 3)`), and a should_panic
# test prints `name - should panic`; the outcome starts at the first " ... ".
TEST_LINE = re.compile(r"^test (.+?) \.\.\. (.*)$")
DOC_TESTS = re.compile(r"^\s*Doc-tests (\S+)")


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


def read_allowlist() -> list[dict]:
    """`<binary> <test path> <source file> <category>  # reason` lines of ci/ignored-tests.txt.

    The test path may contain spaces (a doctest name), so the binary is the first
    token and the file and category are the last two.
    """
    entries = []
    for number, raw in enumerate(IGNORED.read_text(encoding="utf-8").splitlines(), 1):
        code, _, reason = raw.partition("#")
        code = code.strip()
        if not code:
            continue
        binary, _, rest = code.partition(" ")
        parts = rest.rsplit(None, 2)
        if not binary or len(parts) != 3:
            sys.exit(
                f"{IGNORED}:{number}: want '<binary> <test path> <source file> <category>', got {raw!r}"
            )
        entries.append(
            {
                "binary": binary,
                "path": parts[0],
                "file": parts[1],
                "category": parts[2],
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


def mask_source(text: str) -> str:
    """`text` with comments blanked and string/char literal contents replaced by `x`.

    Line structure and offsets are kept, so a search on the result finds real code
    only: an `#[ignore]` inside a comment, a doc string or a string literal is gone.
    Handles nested block comments, escapes, raw strings and lifetimes.
    """
    out = list(text)
    n = len(text)
    i = 0

    def blank(start: int, end: int, keep_quotes: bool = False) -> None:
        for k in range(start, min(end, n)):
            if out[k] != "\n":
                out[k] = "x" if keep_quotes else " "

    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            end = text.find("\n", i)
            end = n if end < 0 else end
            blank(i, end)
            i = end
        elif c == "/" and nxt == "*":
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == "r" and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_") or text[i - 1] == "b"):
            hashes = 0
            while i + 1 + hashes < n and text[i + 1 + hashes] == "#":
                hashes += 1
            if i + 1 + hashes < n and text[i + 1 + hashes] == '"':
                close = '"' + "#" * hashes
                end = text.find(close, i + 2 + hashes)
                end = n if end < 0 else end
                blank(i + 2 + hashes, end, keep_quotes=True)
                i = end + len(close)
            else:
                i += 1
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            blank(i + 1, j, keep_quotes=True)
            i = j + 1
        elif c == "'":
            if nxt == "\\":
                end = text.find("'", i + 3)
                end = n if end < 0 else end
                blank(i + 1, end, keep_quotes=True)
                i = end + 1
            elif i + 2 < n and text[i + 2] == "'":
                blank(i + 1, i + 2, keep_quotes=True)
                i += 3
            else:
                i += 1  # a lifetime or label
        else:
            i += 1
    return "".join(out)


QUALIFIERS = re.compile(r'(?:pub(?:\s*\([^)]*\))?|async|unsafe|const|extern(?:\s*"[^"]*")?)\s*$')
PLAIN_IGNORE = re.compile(r'#\s*\[\s*ignore\s*(?:=\s*"[^"]*"\s*)?\]')


def attributes_of_fn(masked: str, name: str) -> list[list[str]]:
    """For every `fn <name>` in comment-free source: the attributes attached to it."""
    found = []
    for match in re.finditer(rf"\bfn\s+{re.escape(name)}\b", masked):
        head = masked[: match.start()].rstrip()
        while (qualifier := QUALIFIERS.search(head)) is not None:
            head = head[: qualifier.start()].rstrip()
        attributes = []
        while head.endswith("]"):
            depth, k = 0, len(head) - 1
            while k >= 0:
                depth += {"]": 1, "[": -1}.get(head[k], 0)
                if depth == 0:
                    break
                k -= 1
            if k < 1 or head[:k].rstrip()[-1:] != "#":
                break
            hash_at = head[:k].rstrip().rfind("#")
            attributes.append(head[hash_at:])
            head = head[:hash_at].rstrip()
        found.append(attributes)
    return found


DOC_NAME = re.compile(r"^(?P<file>.+?) - (?P<item>.*?)\s*\(line (?P<line>\d+)\)$")
FENCE = re.compile(r"^\s*(?:///|//!|\*)?\s*(`{3,}|~{3,})\s*(?P<info>[^`]*)$")


def check_doctest(entry: dict) -> str | None:
    """A problem with a doctest entry, or None if it names a real ignored doctest."""
    named = DOC_NAME.match(entry["path"])
    if not named:
        return "doctest name is not '<file> - <item> (line <n>)'"
    if Path(named["file"]).as_posix() != Path(entry["file"]).as_posix():
        return f"doctest names {named['file']}, not {entry['file']}"
    lines = (ROOT / entry["file"]).read_text(encoding="utf-8").splitlines()
    number = int(named["line"])
    if not 1 <= number <= len(lines):
        return f"line {number} is outside {entry['file']}"
    line = lines[number - 1]
    if not re.match(r"^\s*(///|//!)", line):
        return f"line {number} of {entry['file']} is not a doc comment"
    fence = FENCE.match(line)
    if not fence:
        return f"line {number} of {entry['file']} does not open a code fence"
    if "ignore" not in re.split(r"[,\s]+", fence["info"].strip()):
        return f"the fence at line {number} of {entry['file']} is not ignored"
    if line.lstrip().startswith("///"):
        # An outer doc comment documents the next item: it must be the named one.
        following = number
        while following < len(lines) and re.match(r"^\s*(///|#\[|//)", lines[following]):
            following += 1
        segment = re.sub(r"<.*", "", named["item"].rsplit("::", 1)[-1]).strip()
        if following >= len(lines) or not re.search(rf"\b{re.escape(segment)}\b", lines[following]):
            return f"the doc comment at line {number} of {entry['file']} does not document {named['item']}"
    return None


def lint_ignored(problems: list[str], slow_keys: set[tuple[str, str]]) -> None:
    for entry in read_allowlist():
        binary, path, file = entry["binary"], entry["path"], entry["file"]
        where = f"ci/ignored-tests.txt:{entry['line']}: {binary} {path}"
        if entry["category"] not in CATEGORIES:
            problems.append(
                f"{where}: category {entry['category']!r} is not one of {', '.join(CATEGORIES)}"
            )
        if entry["category"] == "inherited-gap" and not entry["reason"]:
            problems.append(f"{where}: an inherited-gap entry needs a one-line reason after '#'")
        if (binary, path) in slow_keys:
            problems.append(f"{where}: also in ci/slow-tests.txt")
        source = ROOT / file
        if not source.is_file():
            problems.append(f"{where}: {file} does not exist")
            continue
        if binary.startswith("doc-tests:"):
            problem = check_doctest(entry)
            if problem:
                problems.append(f"{where}: {problem}")
            continue
        name = path.rsplit("::", 1)[-1]
        candidates = attributes_of_fn(mask_source(source.read_text(encoding="utf-8")), name)
        if not candidates:
            problems.append(f"{where}: {file} has no fn {name}")
        elif not all(any(PLAIN_IGNORE.fullmatch(a) for a in attrs) for attrs in candidates):
            problems.append(
                f"{where}: fn {name} in {file} has no unconditional #[ignore] / #[ignore = \"...\"] attribute"
            )


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
        f"features {feature_list(entries)}; {len(read_ignored())} allowlisted ignores checked"
    )
    return 0


def binary_label(running_target: str) -> str:
    """`src/lib.rs (target/debug/deps/kinewright_media-0af...)` -> kinewright_media."""
    inner = re.search(r"\(([^)]*)\)", running_target)
    exe = (inner.group(1) if inner else running_target).replace("\\", "/").rsplit("/", 1)[-1]
    exe = re.sub(r"\.exe$", "", exe)
    return re.sub(r"-[0-9a-f]{16}$", "", exe)


def parse(log: str):
    """Yield (binary, test path, outcome) for every `test x ... y` line."""
    binary = None
    for line in log.splitlines():
        running = RUNNING.match(line)
        if running:
            binary = binary_label(running.group(1))
            continue
        doc = DOC_TESTS.match(line)
        if doc:
            binary = f"doc-tests:{doc.group(1)}"
            continue
        test = TEST_LINE.match(line)
        if test and binary:
            yield binary, test.group(1), test.group(2).strip()


def verify_fast(log: str) -> int:
    expected = {(b, p) for b, p, _ in read_manifest()}
    allowed = {(b, p) for b, p, _ in read_ignored()}
    ignored = {(b, p, outcome) for b, p, outcome in parse(log) if outcome.startswith("ignored")}
    skipped = {(b, p) for b, p, outcome in ignored if REASON in outcome}
    others = {(b, p): outcome for b, p, outcome in ignored if REASON not in outcome}
    problems = [f"not skipped by the fast tier: {b} {p}" for b, p in sorted(expected - skipped)]
    problems += [f"skipped as slow but not in the manifest: {b} {p}" for b, p in sorted(skipped - expected)]
    problems += [
        f"unexpected skip, in neither ci/slow-tests.txt nor ci/ignored-tests.txt: {b} {p} ({outcome})"
        for (b, p), outcome in sorted(others.items())
        if (b, p) not in allowed
    ]
    if problems:
        print("fast tier and manifest disagree:")
        print("\n".join("  " + p for p in problems))
        return 1
    print(
        f"fast tier skipped exactly the {len(expected)} manifest tests and "
        f"{len(others)} allowlisted ignores (ci/ignored-tests.txt), nothing else"
    )
    return 0


def verify_slow(log: str) -> int:
    expected = {(b, p) for b, p, _ in read_manifest()}
    outcomes = {}
    for b, p, outcome in parse(log):
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
    process = subprocess.Popen(
        args,
        cwd=ROOT,
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
        return verify_fast(log) if command == "verify-fast" else verify_slow(log)
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

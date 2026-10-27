"""Run only with the parent's exclusive source/Cargo lane, from the worktree root."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess


def mutate(source, case):
    """Resolve all exact edits against the original bytes before applying any edit."""
    spans = []
    for edit in case["edits"]:
        old, new = edit["old"].encode(), edit["new"].encode()
        if not old or old == new or source.count(old) != 1:
            raise SystemExit(f"mutation selector drift: {case['id']}")
        start = source.index(old)
        spans.append((start, start + len(old), new))
    spans.sort()
    if not spans or any(left[1] > right[0] for left, right in zip(spans, spans[1:])):
        raise SystemExit(f"empty or overlapping edits: {case['id']}")
    for start, end, new in reversed(spans):
        source = source[:start] + new + source[end:]
    return source


def panic_sites(source, case):
    """Bind the intended marker to assertion or explicit-panic lines in its witness."""
    text = source.decode()
    function = re.search(r"\bfn " + re.escape(case["selector"]) + r"\s*\(", text)
    if function is None:
        raise SystemExit(f"missing witness: {case['id']}")
    following = re.search(r"\n\s*(?:pub\s+)?fn\s+\w+\s*\(", text[function.end():])
    end = function.end() + following.start() if following else len(text)
    marker = case["expected_panic"]
    if not re.fullmatch(r"C0_[A-Z0-9_]+", marker):
        raise SystemExit(f"invalid intended panic marker: {case['id']}")
    if "panic_anchor" in case:
        anchor = case["panic_anchor"]
        body = text[function.start():end]
        if (body.count(anchor) != 1 or f'"{marker}"' not in body
                or not re.match(r"\s*(?:assert(?:_eq|_ne)?|panic)!\s*\(", anchor)
                or '"{marker}"' not in anchor):
            raise SystemExit(f"intended assertion anchor drift: {case['id']}")
        offset = function.start() + body.index(anchor) + len(anchor) - len(anchor.lstrip())
        return {text.count("\n", 0, offset) + 1}
    sites = set()
    for match in re.finditer(r'"' + re.escape(marker) + r'(?=[" :])', text[function.start():end]):
        offset = function.start() + match.start()
        macros = list(re.finditer(r"\b(?:assert(?:_eq|_ne)?|panic)!\s*\(", text[function.start():offset]))
        if not macros:
            raise SystemExit(f"marker is not an assertion or panic: {case['id']}")
        assertion = function.start() + macros[-1].start()
        sites.add(text.count("\n", 0, assertion) + 1)
    if not sites:
        raise SystemExit(f"missing intended panic marker: {case['id']} {marker}")
    return sites


def behavioral_red(result, case, sites):
    if (result.returncode != 101
            or "test result: FAILED. 0 passed; 1 failed; 0 ignored;" not in result.stdout
            or re.search(r"could not compile|error\[E\d+\]|called `(?:Result|Option)::unwrap", result.stdout)):
        return False
    panics = list(re.finditer(
        r"^thread '([^']+)'(?: \(\d+\))? panicked at ([^\n]+):(\d+):(\d+):\n(.*?)(?=\n(?:note:|stack backtrace:|thread '|failures:|test result:)|\Z)",
        result.stdout, re.MULTILINE | re.DOTALL))
    if len(panics) != 1:
        return False
    panic = panics[0]
    location = Path(panic[2])
    if not location.is_absolute():
        location = Path.cwd() / location
    return (panic[1].split("::")[-1] == case["selector"]
            and location == Path.cwd() / case["witness_file"]
            and int(panic[3]) in sites
            and re.search(r"\b" + re.escape(case["expected_panic"]) + r"\b", panic[5]) is not None)


def require_memory():
    available = int(next(line.split()[1] for line in Path("/proc/meminfo").read_text().splitlines()
                         if line.startswith("MemAvailable:"))) * 1024
    if available < 6 * 1024 * 1024 * 1024:
        raise RuntimeError("less than 6 GiB available")


def run(shell, root, evidence, case, phase):
    require_memory()
    process = subprocess.Popen(["bash", "-c", shell], cwd=root, env=os.environ.copy(), text=True,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        output = process.communicate(timeout=600)[0]
    except BaseException:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        output = process.communicate()[0]
        with gzip.open(evidence / f"{case['id']}-{phase}.log.gz", "wt") as log:
            log.write(output)
        raise
    with gzip.open(evidence / f"{case['id']}-{phase}.log.gz", "wt") as log:
        log.write(output)
    return subprocess.CompletedProcess(["bash", "-c", shell], process.returncode, output)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exclusive-lane", action="store_true")
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--case", action="append")
    args = parser.parse_args()
    root = Path.cwd()
    if root != Path("/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5"):
        raise SystemExit("wrong worktree")
    if subprocess.check_output(["git", "branch", "--show-current"], text=True).strip() != "ja1/s3":
        raise SystemExit("wrong branch")
    if subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip() != "4250d146eb767b64a058bcbafb5f9adbccbb5264":
        raise SystemExit("wrong base commit")
    cases = json.loads((root / "crates/kinewright-project/tests/c0-mutations.json").read_bytes())
    ids, mutations, snapshots, intended_sites = set(), set(), {}, {}
    for case in cases:
        if case["id"] in ids:
            raise SystemExit(f"duplicate case id: {case['id']}")
        ids.add(case["id"])
        for key in ("file", "witness_file"):
            path = Path(case[key])
            if path.is_absolute() or ".." in path.parts or path.parts[:2] != ("crates", "kinewright-project"):
                raise SystemExit(f"file outside project scope: {case['id']}")
            if path not in snapshots:
                snapshots[path] = (root / path).read_bytes()
        mutant = mutate(snapshots[Path(case["file"])], case)
        identity = (case["file"], hashlib.sha256(mutant).digest())
        if identity in mutations:
            raise SystemExit(f"duplicate mutation: {case['id']}")
        mutations.add(identity)
        witness = mutant if case["file"] == case["witness_file"] else snapshots[Path(case["witness_file"])]
        intended_sites[case["id"]] = panic_sites(witness, case)
    if args.case and set(args.case) - ids:
        parser.error("unknown case ids: " + ", ".join(sorted(set(args.case) - ids)))
    selected = [case for case in cases if not args.case or case["id"] in args.case]
    if args.list:
        for case in selected:
            record = {key: case[key] for key in ("id", "file", "selector", "target", "witness_file", "expected_panic")}
            record["panic_lines"] = sorted(intended_sites[case["id"]])
            print(json.dumps(record))
        return
    if not args.exclusive_lane or args.evidence is None:
        parser.error("--exclusive-lane and --evidence are required for execution")
    args.evidence.mkdir(parents=True, exist_ok=True)
    for case in selected:
        for relative, original in snapshots.items():
            if (root / relative).read_bytes() != original:
                raise SystemExit(f"source changed since preflight: {relative}")
        path = root / case["file"]
        original = snapshots[Path(case["file"])]
        mutant = mutate(original, case)
        command = ["cargo", "test", "-p", "kinewright-project", "--features", "test-util"]
        command += ["--lib"] if case["target"] == "lib" else ["--test", case["target"]]
        command += [case["selector"], "--", "--test-threads=1"]
        shell = "source scripts/setup-ffmpeg.sh && flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 " + shlex.join(command)
        try:
            path.write_bytes(mutant)
            red = run(shell, root, args.evidence, case, "red")
        finally:
            if path.read_bytes() != mutant:
                raise SystemExit("concurrent source edit detected; do not overwrite it")
            path.write_bytes(original)
        if not behavioral_red(red, case, intended_sites[case["id"]]):
            raise SystemExit(f"not the intended assertion red: {case['id']} {case['expected_panic']}")
        for relative, expected in snapshots.items():
            if (root / relative).read_bytes() != expected:
                raise SystemExit(f"source changed during red: {relative}")
        green = run(shell, root, args.evidence, case, "green")
        if green.returncode != 0 or "test result: ok. 1 passed; 0 failed; 0 ignored;" not in green.stdout:
            raise SystemExit(f"restored green failed: {case['id']}")
        for relative, expected in snapshots.items():
            if (root / relative).read_bytes() != expected:
                raise SystemExit(f"restored source changed during green: {relative}")
        record = {"case": case["id"], "selector": case["selector"], "expected_panic": case["expected_panic"],
                  "source_sha256": hashlib.sha256(original).hexdigest(),
                  "mutant_sha256": hashlib.sha256(mutant).hexdigest(),
                  "witness_sha256": hashlib.sha256(snapshots[Path(case["witness_file"])]).hexdigest(),
                  "red_exit": red.returncode, "green_exit": green.returncode}
        with (args.evidence / "pairs.jsonl").open("a") as output:
            output.write(json.dumps(record) + "\n")
        print(json.dumps(record), flush=True)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    main()

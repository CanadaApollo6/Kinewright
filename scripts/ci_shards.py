#!/usr/bin/env python3
"""Build a deterministic libtest partition from nextest's compiled inventory.

Nextest archives avoid compilation on test runners. Run libtest batches directly
because mcp_server has crashed with per-test filtering and the app has order-sensitive
fixtures. Media and agent library tests get separate processes, at most four per runner.
All result logs retain the existing slow_tests.py ignored-reason proof.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import slow_tests as st


def load(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def save(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def inventory(listing, tier, os_name=None):
    expected = {(b, p) for b, p, _, scope in st.read_manifest()
                if not scope.endswith("-only") or scope == (os_name or st.host_os()) + "-only"}
    suites = []
    total = sum(len(s["testcases"]) for s in listing["rust-suites"].values())
    if "test-count" in listing and listing["test-count"] != total:
        raise ValueError("compiled listing count differs from its suites")
    for suite in listing["rust-suites"].values():
        if suite.get("status", "listed") != "listed":
            raise ValueError("compiled listing contains an unlisted test binary")
        label = st.binary_label(suite["binary-path"])
        tests = sorted(suite["testcases"])
        if tier == "slow":
            tests = [p for p in tests if (label, p) in expected]
        if tests:
            suites.append({"label": label, "path": suite["binary-path"], "cwd": suite["cwd"],
                           "tests": tests, "ignored": [p for p in tests if suite["testcases"][p]["ignored"]]})
    keys = [(s["label"], p) for s in suites for p in s["tests"]]
    if len(keys) != len(set(keys)) or not keys:
        raise ValueError("empty or ambiguous compiled inventory")
    if tier == "slow" and set(keys) != expected:
        raise ValueError(f"compiled slow inventory differs from manifest: {set(keys) ^ expected}")
    if tier == "slow" and any(s["ignored"] for s in suites):
        raise ValueError("slow feature did not enable every manifest test")
    return suites


def weights():
    result = {}
    for line in st.MANIFEST.read_text(encoding="utf-8").splitlines():
        match = re.search(r"#\s*(\d+)/(\d+)", line)
        if match:
            b, p, *_ = line.split()
            result[b, p] = int(match.group(1 if st.host_os() == "linux" else 2))
    measured = load(st.ROOT / "ci" / "test-timings.json")
    os_name = st.host_os()
    for binary, tests in measured["unit_seconds"][os_name].items():
        result.update({(binary, name): seconds for name, seconds in tests.items()})
    return result, measured["slow_batch_seconds"][os_name]


def predicted_seconds(units):
    """Match the runner's exclusive batches followed by four isolated processes."""
    exclusive = sum(u["estimate"] for u in units if not u["isolated"])
    lanes = [0.0] * 4
    for unit in units:
        if unit["isolated"]:
            lane = min(range(4), key=lambda i: lanes[i])
            lanes[lane] += unit["estimate"]
    return exclusive + max(lanes)


def plan(listing, tier, count):
    suites = inventory(listing, tier)
    estimates, slow_batches = weights()
    units = []
    for suite_index, suite in enumerate(suites):
        split = suite["label"] in ("kinewright_media", "kinewright_agent")
        groups = [[p] for p in suite["tests"] if p not in suite["ignored"]] if split else [suite["tests"]]
        if split and suite["ignored"]:
            groups.append(suite["ignored"])
        for names in groups:
            cost = sum(estimates.get((suite["label"], p), 60 if p.startswith("pf1_s2c_witness::") else 2) for p in names if p not in suite["ignored"])
            if not split:
                cost = {"kinewright_app": 342 if tier == "fast" else 28, "mcp_server": 182,
                        "ja_s1_storage": 188, "kinewright_project": 86}.get(suite["label"], max(1, len(names) * 0.02))
                if len(names) == 1:
                    cost = estimates.get((suite["label"], names[0]), cost)
                if tier == "slow":
                    cost = slow_batches.get(suite["label"], cost)
            units.append({"suite": suite_index, "tests": names, "full": not split and tier == "fast", "isolated": split, "estimate": cost})
    shards = [[] for _ in range(count)]
    ordered = sorted(units, key=lambda u: (-u["estimate"], suites[u["suite"]]["label"], u["tests"]))
    # Long tests get a runner alone, without waiting behind other test processes.
    long_tests = [u for u in ordered if u["isolated"] and u["estimate"] > 240][:max(0, count - 2)]
    for slot, unit in enumerate(long_tests, 1):
        shards[slot].append(unit)
    available = list(range(1 + len(long_tests), count)) if count > 1 else [0]
    for unit in ordered:
        if unit in long_tests:
            continue
        if suites[unit["suite"]]["label"] == "kinewright_app":
            slot = 0
        else:
            slot = min(available, key=lambda i: (predicted_seconds([*shards[i], unit]),
                                                 sum(u["estimate"] for u in shards[i]), i))
        shards[slot].append(unit)
    loads = [round(predicted_seconds(shard), 3) for shard in shards]
    return {"os": st.host_os(), "tier": tier, "suites": suites, "shards": shards, "estimated_seconds": loads}


def execute_unit(data, unit, number, index, directory):
    suite = data["suites"][unit["suite"]]
    binary = Path(suite["path"])
    # Embedded Cargo paths require the same checkout/target paths on each OS.
    if not binary.is_file():
        raise ValueError(f"missing archived executable {binary}")
    args = [str(binary), "--color", "never"]
    if not unit["full"]:
        args += ["--exact", *unit["tests"]]
    if data["tier"] == "slow":
        args += ["--show-output"]
    threads = "1" if suite["label"] == "kinewright_media" else "4"
    args += ["--test-threads", threads]
    log_name = f"shard-{index}-{number}.log"
    log_path = directory / log_name
    started = time.monotonic()
    env = {**os.environ, "CARGO_MANIFEST_DIR": suite["cwd"],
           "CARGO_TERM_COLOR": "never", "RUST_TEST_THREADS": threads}
    with log_path.open("w", encoding="utf-8") as log:
        log.write(f"Running tests/{suite['label']}.rs ({binary})\n")
        log.flush()
        try:
            process = subprocess.run(args, cwd=suite["cwd"], env=env, stdout=log,
                                     stderr=subprocess.STDOUT, timeout=600)
            code = process.returncode
        except subprocess.TimeoutExpired:
            code = 124
    elapsed = time.monotonic() - started
    print(f"{suite['label']} {len(unit['tests'])} tests: {elapsed:.2f}s, exit {code}", flush=True)
    text = log_path.read_text(encoding="utf-8", errors="replace")
    if not code:
        st.verify_exact_rows([(suite["label"], p) for p in unit["tests"]], st.parse(text))
    else:
        print(text)
    return {"log": log_name, "code": code, "seconds": elapsed,
            "binary": suite["label"], "tests": unit["tests"]}


def run_shard(plan_path, index, output):
    data = load(plan_path)
    directory = Path(output)
    directory.mkdir(parents=True, exist_ok=True)
    units = data["shards"][index]
    result = {"plan_sha256": hashlib.sha256(Path(plan_path).read_bytes()).hexdigest(),
              "index": index, "completed": False, "runs": [None] * len(units)}
    result_path = directory / f"shard-{index}.json"
    save(result_path, result)
    errors = []

    def record(number, run):
        result["runs"][number] = run
        save(result_path, result)
        if run["code"]:
            errors.append(f"test process {number} failed or timed out ({run['code']})")

    # Full binaries retain libtest's scheduling and never share their runner's GPU.
    for number, unit in enumerate(units):
        if not unit["isolated"]:
            record(number, execute_unit(data, unit, number, index, directory))
            if errors:
                raise ValueError("; ".join(errors))
    with ThreadPoolExecutor(max_workers=4) as executor:
        pending = {executor.submit(execute_unit, data, u, n, index, directory): n
                   for n, u in enumerate(units) if u["isolated"]}
        for future in as_completed(pending):
            number = pending[future]
            try:
                record(number, future.result())
            except Exception as error:
                errors.append(f"test process {number}: {error}")
    if errors:
        raise ValueError("; ".join(errors))
    result["completed"] = True
    save(result_path, result)


def verify(plan_path, output, doc_log=None, doc_list=None):
    data = load(plan_path)
    directory = Path(output)
    compiled = inventory(load(Path(plan_path).with_name("compiled-list.json")), data["tier"], data["os"])
    if compiled != data["suites"]:
        raise ValueError("plan differs from the independent compiled listing")
    expected = [(s["label"], p) for s in compiled for p in s["tests"]]
    planned = [(compiled[u["suite"]]["label"], p) for shard in data["shards"] for u in shard for p in u["tests"]]
    if Counter(planned) != Counter(expected):
        raise ValueError("partition dropped or duplicated a compiled test")
    digest = hashlib.sha256(Path(plan_path).read_bytes()).hexdigest()
    logs = []
    timings = []
    for index, units in enumerate(data["shards"]):
        result = load(directory / f"shard-{index}.json")
        if result["plan_sha256"] != digest or result["index"] != index or not result["completed"]:
            raise ValueError(f"shard {index} missing, incomplete or from another plan")
        if len(result["runs"]) != len(units):
            raise ValueError(f"shard {index} missing a test process")
        for number, (run, unit) in enumerate(zip(result["runs"], units)):
            if run["code"] != 0 or run["log"] != f"shard-{index}-{number}.log":
                raise ValueError(f"shard {index} failed or reused a log")
            log = (directory / run["log"]).read_text(encoding="utf-8", errors="replace")
            rows = st.parse(log)
            st.verify_exact_rows([(compiled[unit["suite"]]["label"], p) for p in unit["tests"]], rows)
            logs.append(log)
            timings.append(run)
    rows = st.parse("\n".join(logs))
    st.verify_exact_rows(expected, rows)
    if data["tier"] == "fast":
        if doc_log is None or doc_list is None:
            raise ValueError("fast coverage requires the doctest inventory and results")
        doc_text = Path(doc_log).read_text(encoding="utf-8")
        st.verify_exact_rows(load(doc_list), st.parse(doc_text))
        logs.append(doc_text)
        rows.extend(st.parse(doc_text))
        code = st.verify_fast("\n".join(logs), data["os"])
    else:
        code = st.verify_slow("\n".join(logs), data["os"])
    if code:
        raise ValueError("tier accounting failed")
    passed = sum(o == "ok" for _, _, o in rows)
    ignored = sum(o.startswith("ignored") for _, _, o in rows)
    print(f"{data['os']} {data['tier']}: {len(data['shards'])} shards, {passed} passed, {ignored} ignored; every compiled test exactly once")
    print("20 slowest timed invocations (individual where one test, batches otherwise):")
    for run in sorted(timings, key=lambda r: -r["seconds"])[:20]:
        label = run["tests"][0] if len(run["tests"]) == 1 else f"[batch of {len(run['tests'])} tests]"
        print(f"{run['seconds']:.3f}s {run['binary']} {label}")


def doc_inventory(log):
    binary = None
    rows = []
    for line in st.ANSI.sub("", log).splitlines():
        match = st.DOC_TESTS.match(line)
        if match:
            binary = "doc-tests:" + match.group(1)
        elif line.endswith(": test"):
            if binary is None:
                raise ValueError("doctest listed outside a section")
            rows.append((binary, line.removesuffix(": test")))
    if not rows or len(rows) != len(set(rows)):
        raise ValueError("empty or duplicate doctest inventory")
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["plan", "run", "verify", "doc-list"])
    parser.add_argument("--plan", default="ci-plan.json")
    parser.add_argument("--listing")
    parser.add_argument("--tier", choices=["fast", "slow"])
    parser.add_argument("--count", type=int)
    parser.add_argument("--index", type=int)
    parser.add_argument("--output", default="ci-results")
    parser.add_argument("--doc-log")
    parser.add_argument("--doc-list")
    args = parser.parse_args()
    if args.command == "plan":
        if not args.count or args.count < 1:
            raise ValueError("positive shard count required")
        data = plan(load(args.listing), args.tier, args.count)
        save(args.plan, data)
        print(f"{sum(len(s['tests']) for s in data['suites'])} compiled tests; estimated shard seconds {data['estimated_seconds']}")
    elif args.command == "run":
        run_shard(args.plan, args.index, args.output)
    elif args.command == "doc-list":
        save(args.doc_list, doc_inventory(Path(args.doc_log).read_text(encoding="utf-8")))
    else:
        verify(args.plan, args.output, args.doc_log, args.doc_list)


if __name__ == "__main__":
    main()

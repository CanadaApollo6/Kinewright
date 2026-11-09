#!/usr/bin/env python3
"""Coverage, process-failure and empty-partition witnesses for archived CI tests."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci_shards as cs
import slow_tests as st


def listing():
    suites = {}
    for binary, names in {"kinewright_media": ["a", "b", "skip"],
                          "kinewright_app": ["app1", "app2"], "mcp_server": ["mcp1", "mcp2"]}.items():
        suites[binary] = {"binary-path": f"/target/debug/deps/{binary}-0123456789abcdef",
                          "cwd": "/repo", "testcases": {p: {"ignored": p == "skip"} for p in names}}
    return {"rust-suites": suites}


def log(binary, names):
    passed = sum(p != "skip" for p in names)
    ignored = len(names) - passed
    return (f"Running tests/{binary}.rs (target/debug/deps/{binary}-0123456789abcdef)\n"
            f"running {len(names)} tests\n" +
            "".join(f"test {p} ... {'ignored, hardware' if p == 'skip' else 'ok'}\n" for p in names) +
            f"test result: ok. {passed} passed; 0 failed; {ignored} ignored; 0 measured; 0 filtered out; finished in 0.01s\n")


class Sharding(unittest.TestCase):
    def test_partition_preserves_all_tests_and_sensitive_batches(self):
        for count in (1, 3, 12):
            data = cs.plan(listing(), "fast", count)
            units = [u for shard in data["shards"] for u in shard]
            expected = [(s["label"], p) for s in data["suites"] for p in s["tests"]]
            observed = [(data["suites"][u["suite"]]["label"], p, "ok") for u in units for p in u["tests"]]
            st.verify_exact_rows(expected, observed)
            for name in ("kinewright_app", "mcp_server"):
                batch = [u for u in units if data["suites"][u["suite"]]["label"] == name]
                self.assertEqual(len(batch), 1)
                self.assertTrue(batch[0]["full"])
                self.assertEqual(len(batch[0]["tests"]), 2)
            self.assertEqual(data, cs.plan(listing(), "fast", count))

    def test_inventory_rejects_a_dropped_suite_or_an_unlisted_binary(self):
        data = listing()
        data["test-count"] = 8
        with self.assertRaisesRegex(ValueError, "listing count differs"):
            cs.plan(data, "fast", 2)
        del data["test-count"]
        data["rust-suites"]["mcp_server"]["status"] = "skipped"
        with self.assertRaisesRegex(ValueError, "unlisted test binary"):
            cs.plan(data, "fast", 2)

    def test_measured_long_tests_get_dedicated_shards(self):
        data = listing()
        data["rust-suites"]["kinewright_media"]["testcases"]["c"] = {"ignored": False}
        measured = {("kinewright_media", "a"): 434, ("kinewright_media", "b"): 346,
                    ("kinewright_media", "c"): 200}
        with mock.patch.object(cs, "weights", return_value=(measured, {})):
            result = cs.plan(data, "fast", 5)
        self.assertEqual([u["tests"] for u in result["shards"][1]], [["a"]])
        self.assertEqual([u["tests"] for u in result["shards"][2]], [["b"]])
        self.assertEqual(result["estimated_seconds"][1:3], [434, 346])
        for count in (1, 2, 3):
            with mock.patch.object(cs, "weights", return_value=(measured, {})):
                small = cs.plan(data, "fast", count)
            expected = [(s["label"], p) for s in small["suites"] for p in s["tests"]]
            rows = [(small["suites"][u["suite"]]["label"], p, "ok")
                    for shard in small["shards"] for u in shard for p in u["tests"]]
            st.verify_exact_rows(expected, rows)

    def test_prediction_keeps_exclusive_batches_and_parallel_tail(self):
        units = [{"isolated": False, "estimate": 50},
                 *[{"isolated": True, "estimate": x} for x in (120, 100, 90, 80, 70)]]
        self.assertEqual(cs.predicted_seconds(units), 200)

    def test_slow_inventory_requires_every_manifest_test_enabled(self):
        entries = [("mcp_server", "mcp1", "source", "any")]
        with mock.patch.object(st, "read_manifest", return_value=entries):
            data = cs.plan(listing(), "slow", 2)
            self.assertEqual([s["tests"] for s in data["suites"]], [["mcp1"]])
            data_listing = listing()
            data_listing["rust-suites"]["mcp_server"]["testcases"]["mcp1"]["ignored"] = True
            with self.assertRaisesRegex(ValueError, "did not enable"):
                cs.plan(data_listing, "slow", 2)
            entries.append(("absent", "test", "source", "any"))
            with self.assertRaisesRegex(ValueError, "differs"):
                cs.plan(listing(), "slow", 2)

    def test_slow_os_scope_comes_from_the_plan(self):
        entries = [("mcp_server", "mcp1", "source", "windows-only")]
        with mock.patch.object(st, "read_manifest", return_value=entries):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(st.verify_slow(log("mcp_server", ["mcp1"]), "windows"), 0)
                self.assertEqual(st.verify_slow(log("mcp_server", ["mcp1"]), "linux"), 1)

    def test_doctest_inventory_requires_names_in_sections(self):
        self.assertEqual(cs.doc_inventory(" Doc-tests foo\npath.rs - item (line 3): test\n"),
                         [("doc-tests:foo", "path.rs - item (line 3)")])
        for text in ("", "x: test\n", " Doc-tests foo\nx: test\nx: test\n"):
            with self.assertRaises(ValueError):
                cs.doc_inventory(text)

    def prepare(self, root):
        data = cs.plan(listing(), "fast", 12)
        plan_path = root / "plan.json"
        cs.save(plan_path, data)
        cs.save(root / "compiled-list.json", listing())
        digest = hashlib.sha256(plan_path.read_bytes()).hexdigest()
        for index, units in enumerate(data["shards"]):
            runs = []
            for number, unit in enumerate(units):
                name = f"shard-{index}-{number}.log"
                (root / name).write_text(log(data["suites"][unit["suite"]]["label"], unit["tests"]))
                runs.append({"log": name, "code": 0, "seconds": 0.01,
                             "binary": data["suites"][unit["suite"]]["label"], "tests": unit["tests"]})
            cs.save(root / f"shard-{index}.json", {"plan_sha256": digest, "index": index,
                                                   "completed": True, "runs": runs})
        docs = root / "docs.log"
        docs.write_text("Doc-tests foo\nrunning 1 test\ntest item ... ok\n"
                        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n")
        doc_list = root / "docs.json"
        cs.save(doc_list, [("doc-tests:foo", "item")])
        return plan_path, docs, doc_list

    def check(self, root, paths):
        with mock.patch.object(st, "read_manifest", return_value=[]), \
             mock.patch.object(st, "allowlist_for", return_value={("kinewright_media", "skip")}), \
             contextlib.redirect_stdout(io.StringIO()):
            cs.verify(paths[0], root, paths[1], paths[2])

    def test_aggregate_accepts_explicitly_empty_shards(self):
        with tempfile.TemporaryDirectory(prefix="kinewright-ci-selftest-") as directory:
            root = Path(directory)
            paths = self.prepare(root)
            self.check(root, paths)

    def test_aggregate_rejects_each_dropped_or_corrupted_artifact(self):
        for fault in ("missing-shard", "missing-log", "incomplete", "wrong-plan", "wrong-index",
                      "failed", "timeout", "missing-process", "zero-tests", "duplicate-result",
                      "wrong-test", "wrong-binary", "missing-doc", "missing-doc-result", "missing-unit"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory(prefix="kinewright-ci-selftest-") as directory:
                root = Path(directory)
                paths = self.prepare(root)
                data = cs.load(paths[0])
                index = next(i for i, units in enumerate(data["shards"]) if units)
                result_path = root / f"shard-{index}.json"
                result = cs.load(result_path)
                log_path = root / result["runs"][0]["log"]
                if fault == "missing-shard":
                    result_path.unlink()
                elif fault == "missing-log":
                    log_path.unlink()
                elif fault == "missing-doc":
                    paths[1].unlink()
                elif fault == "missing-doc-result":
                    paths[1].write_text(log("foo", ["other"]).replace("Running tests/foo.rs (target/debug/deps/foo-0123456789abcdef)", "Doc-tests foo"))
                elif fault == "zero-tests":
                    log_path.write_text(log(result["runs"][0]["binary"], []))
                elif fault == "duplicate-result":
                    log_path.write_text(log_path.read_text() * 2)
                elif fault == "wrong-test":
                    log_path.write_text(log(result["runs"][0]["binary"], ["other"]))
                elif fault == "wrong-binary":
                    log_path.write_text(log("other", result["runs"][0]["tests"]))
                elif fault == "missing-unit":
                    data["shards"][index].pop()
                    cs.save(paths[0], data)
                else:
                    if fault == "incomplete": result["completed"] = False
                    if fault == "wrong-plan": result["plan_sha256"] = "other"
                    if fault == "wrong-index": result["index"] = 99
                    if fault == "failed": result["runs"][0]["code"] = -11
                    if fault == "timeout": result["runs"][0]["code"] = 124
                    if fault == "missing-process": result["runs"].pop()
                    cs.save(result_path, result)
                with self.assertRaises((ValueError, st.UnparsableLog, FileNotFoundError)):
                    self.check(root, paths)

    def test_parallel_failure_drains_and_records_other_started_tests(self):
        for raises in (False, True):
            with self.subTest(raises=raises), tempfile.TemporaryDirectory(prefix="kinewright-ci-selftest-") as directory:
                root = Path(directory)
                path = root / "plan.json"
                cs.save(path, {"shards": [[{"isolated": True}, {"isolated": True}]]})
                started = threading.Event()
                finished = threading.Event()

                def execute(data, unit, number, index, output):
                    if number == 0:
                        self.assertTrue(started.wait(2))
                        if raises:
                            raise ValueError("parser failure")
                        return {"code": -11}
                    started.set()
                    finished.set()
                    return {"code": 0}

                with mock.patch.object(cs, "execute_unit", side_effect=execute):
                    with self.assertRaises(ValueError):
                        cs.run_shard(path, 0, root)
                self.assertTrue(finished.is_set())
                result = cs.load(root / "shard-0.json")
                self.assertFalse(result["completed"])
                self.assertEqual(result["runs"][1]["code"], 0)

    def test_runner_records_timeout_and_never_marks_it_complete(self):
        with tempfile.TemporaryDirectory(prefix="kinewright-ci-selftest-") as directory:
            root = Path(directory)
            binary = root / "witness-0123456789abcdef"
            binary.touch()
            suite = {"path": str(binary), "cwd": str(root), "label": "witness"}
            data = {"tier": "fast", "suites": [suite], "shards": [[{"suite": 0, "tests": ["x"], "full": False, "isolated": False}]]}
            path = root / "plan.json"
            cs.save(path, data)
            with mock.patch.object(subprocess, "run", side_effect=subprocess.TimeoutExpired("test", 600)), \
                 contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(ValueError, "failed or timed out"):
                    cs.run_shard(path, 0, root)
            result = cs.load(root / "shard-0.json")
            self.assertFalse(result["completed"])
            self.assertEqual(result["runs"][0]["code"], 124)


if __name__ == "__main__":
    unittest.main()

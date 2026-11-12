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
import ci_archive as ca
import ci_display_inputs as di
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
    def test_linux_only_host_hooks_match_windows_slow_and_fast_accounting(self):
        # The parent module is absent on Windows, independently of cfg_attr's
        # choice to ignore a compiled test in the fast tier.
        hooks = st.ROOT / "crates/kinewright-agent/src/writer_host/hooks_tests.rs"
        self.assertIn('#![cfg(target_os = "linux")]', hooks.read_text(encoding="utf-8"))
        entries = [e for e in st.read_manifest()
                   if e[0] == "kinewright_agent" and e[1].startswith("writer_host::hooks_tests::")]
        self.assertTrue(entries)
        with mock.patch.object(st, "read_manifest",
                               return_value=[("mcp_server", "mcp1", "source", "any"), *entries]):
            observed = cs.inventory(listing(), "slow", "windows")
            self.assertEqual([(s["label"], s["tests"]) for s in observed],
                             [("mcp_server", ["mcp1"])])
        with mock.patch.object(st, "read_manifest", return_value=entries), \
             mock.patch.object(st, "read_allowlist", return_value=[]), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(st.verify_fast(log("mcp_server", ["mcp1"]), "windows"), 0)
        self.assertTrue(all(e[3] == "linux-only" for e in entries))

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

    def test_control_family_and_compiler_probe_are_exclusive_without_dropping_names(self):
        data = listing()
        names = [cs.CONTROL_PREFIX + "cold_certification_switch", cs.CONTROL_PREFIX + "decode_stop", cs.COMPILER_PROBE]
        data["rust-suites"]["kinewright_agent"] = {"binary-path": "/target/debug/deps/kinewright_agent-0123456789abcdef",
                                                   "cwd": "/repo", "testcases": {n: {"ignored": False} for n in names}}
        display_names = [cs.DISPLAY_PARITY_PREFIX + name + "_matches_ten_frames_exactly"
                         for name in ("typical_1080p", "blend_heavy_1080p", "explainer_16x9", "reel_9x16", "feed_4x5", "talk_recut")]
        data["rust-suites"]["kinewright_media"]["testcases"].update({name: {"ignored": False} for name in display_names})
        neighbor = "display::tests::i5_exact_self_check_requires_gpu"
        data["rust-suites"]["kinewright_media"]["testcases"][neighbor] = {"ignored": False}
        result = cs.plan(data, "fast", 14)
        flat = [u for shard in result["shards"] for u in shard]
        controls = [u for u in flat if names[0] in u["tests"]]
        self.assertEqual(len(controls), 1)
        self.assertEqual(set(controls[0]["tests"]), set(names[:2]))
        self.assertFalse(controls[0]["isolated"])
        self.assertEqual(controls[0]["threads"], 1)
        for name in (names[0], cs.COMPILER_PROBE):
            shard = next(shard for shard in result["shards"] if any(name in u["tests"] for u in shard))
            self.assertEqual(len(shard), 1)
        display_shards = [i for i, shard in enumerate(result["shards"])
                          for u in shard if any(name in u["tests"] for name in display_names)]
        self.assertEqual(len(set(display_shards)), 6)
        self.assertTrue(all(not u["isolated"] for u in flat if u.get("display", False)))
        display = next(u for u in flat if neighbor in u["tests"])
        self.assertEqual(display["timeout_seconds"], 600)
        self.assertEqual(display["tests"], [neighbor])
        self.assertFalse(display["isolated"])
        self.assertEqual(display["threads"], 1)
        self.assertTrue(next(u for u in flat if cs.COMPILER_PROBE in u["tests"])["compiler_inputs"])
        expected = [(s["label"], p) for s in result["suites"] for p in s["tests"]]
        rows = [(result["suites"][u["suite"]]["label"], p, "ok") for u in flat for p in u["tests"]]
        st.verify_exact_rows(expected, rows)

    def test_stale_large_hints_cannot_displace_display_or_preserved_runners(self):
        data = listing()
        display_names = [cs.DISPLAY_PARITY_PREFIX + name + "_matches_ten_frames_exactly"
                         for name in ("typical_1080p", "blend_heavy_1080p", "explainer_16x9", "reel_9x16", "feed_4x5", "talk_recut")]
        stale = [f"stale_test_{i}" for i in range(30)]
        data["rust-suites"]["kinewright_media"]["testcases"].update({name: {"ignored": False} for name in [*display_names, *stale]})
        controls = cs.CONTROL_PREFIX + "cold_certification_switch"
        data["rust-suites"]["kinewright_agent"] = {"binary-path": "/target/debug/deps/kinewright_agent-0123456789abcdef",
                                                   "cwd": "/repo", "testcases": {name: {"ignored": False} for name in (controls, cs.COMPILER_PROBE)}}
        with mock.patch.object(cs, "weights", return_value=({("kinewright_media", name): 9999 for name in stale}, {})):
            result = cs.plan(data, "fast", 14)
        positions = [i for i, shard in enumerate(result["shards"]) for u in shard if u.get("display", False)]
        self.assertEqual(len(set(positions)), 6)
        for name in (controls, cs.COMPILER_PROBE):
            self.assertEqual(len(next(shard for shard in result["shards"] if any(name in u["tests"] for u in shard))), 1)
        expected = [(s["label"], p) for s in result["suites"] for p in s["tests"]]
        rows = [(result["suites"][u["suite"]]["label"], p, "ok") for shard in result["shards"] for u in shard for p in u["tests"]]
        st.verify_exact_rows(expected, rows)

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

    def test_workflow_matrix_and_conditional_compiler_download(self):
        with tempfile.TemporaryDirectory(prefix="kinewright-ci-matrix-") as directory:
            path = Path(directory) / "plan.json"
            cs.save(path, {"shards": [[{"timeout_seconds": 1800}], [{"compiler_inputs": True}], []]})
            output = io.StringIO()
            with mock.patch.object(sys, "argv", ["ci_shards.py", "matrix", "--plan", str(path)]), contextlib.redirect_stdout(output):
                cs.main()
            matrix = json.loads(output.getvalue().removeprefix("matrix="))
            self.assertEqual(matrix, {"include": [{"shard": 0, "timeout": 35}, {"shard": 1, "timeout": 15}, {"shard": 2, "timeout": 15}]})
            for index, wanted in ((0, "false"), (1, "true"), (2, "false")):
                output = io.StringIO()
                with mock.patch.object(sys, "argv", ["ci_shards.py", "requirements", "--plan", str(path), "--index", str(index)]), contextlib.redirect_stdout(output):
                    cs.main()
                self.assertEqual(output.getvalue().strip(), "compiler_inputs=" + wanted + "\ndisplay_workload=")

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

    def prepare(self, root, compiled=None):
        compiled = listing() if compiled is None else compiled
        data = cs.plan(compiled, "fast", 12)
        plan_path = root / "plan.json"
        cs.save(plan_path, data)
        cs.save(root / "compiled-list.json", compiled)
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

    def test_new_te1_test_without_a_timing_is_scheduled_and_required_by_coverage(self):
        name = "writer_host::hooks_tests::tests::te1_new_regression_without_timing"
        for hints in ({}, {"unit_seconds": {"linux": {"fast": {"removed_binary": {"removed_test": 999}}}}},
                      FileNotFoundError("timing seed absent")):
            with self.subTest(hints=hints), tempfile.TemporaryDirectory(prefix="kinewright-ci-new-test-") as directory:
                root = Path(directory)
                compiled = listing()
                compiled["rust-suites"]["kinewright_agent"] = {
                    "binary-path": "/target/debug/deps/kinewright_agent-0123456789abcdef",
                    "cwd": "/repo", "testcases": {name: {"ignored": False}}}
                original_load = cs.load

                def optional_hints(path):
                    if Path(path).name == "test-timings.json":
                        if isinstance(hints, Exception):
                            raise hints
                        return hints
                    return original_load(path)

                with mock.patch.object(cs, "load", side_effect=optional_hints):
                    paths = self.prepare(root, compiled)
                    data = cs.load(paths[0])
                    matches = [(i, n, u) for i, shard in enumerate(data["shards"])
                               for n, u in enumerate(shard) if name in u["tests"]]
                    self.assertEqual(len(matches), 1)
                    self.assertEqual(matches[0][2]["estimate"], 2)
                    self.check(root, paths)
                    index, number, _ = matches[0]
                    (root / f"shard-{index}-{number}.log").write_text(log("kinewright_agent", []))
                    with self.assertRaises((ValueError, st.UnparsableLog)):
                        self.check(root, paths)

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

    def test_alsa_noise_uses_shared_parser_without_hiding_a_crash(self):
        for status in (0, -11):
            with tempfile.TemporaryDirectory(prefix="kinewright-ci-selftest-") as directory:
                root = Path(directory)
                binary = root / "witness-0123456789abcdef"
                binary.touch()
                data = {"tier": "fast", "suites": [{"path": str(binary), "cwd": str(root), "label": "witness"}]}
                unit = {"suite": 0, "tests": ["x"], "full": False}
                def execute(*args, **kwargs):
                    kwargs["stdout"].write("running 1 test\ntest x ... ALSA lib pcm.c:1:(open_pcm) Unknown PCM default\nok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n")
                    return subprocess.CompletedProcess(args[0], status)
                with mock.patch.object(subprocess, "run", side_effect=execute), contextlib.redirect_stdout(io.StringIO()):
                    result = cs.execute_unit(data, unit, 0, 0, root)
                self.assertEqual(result["code"], status)

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


class ArchiveEnvironment(unittest.TestCase):
    def test_compiler_inputs_survive_restore_and_target_tmp_exists(self):
        with tempfile.TemporaryDirectory(prefix="kinewright-ci-archive-") as directory:
            root = Path(directory)
            debug = root / "target" / "debug"
            fingerprint = debug / ".fingerprint" / "agent-123" / "test-lib-agent.json"
            fingerprint.parent.mkdir(parents=True)
            fingerprint.write_bytes(b'{"deps":[[1,"project",false,2]]}')
            library = debug / "deps" / "libproject-123.rlib"
            library.parent.mkdir()
            library.write_bytes(b"compiled-library")
            (library.parent / "test-binary").write_bytes(b"do not duplicate nextest binaries")
            artifact = root / "artifacts"
            artifact.mkdir()
            with mock.patch.object(ca, "ROOT", root):
                ca.pack_compiler_inputs(artifact)
                import shutil
                shutil.rmtree(root / "target")
                ca.restore_test_environment(artifact, [{"compiler_inputs": True}])
            self.assertEqual(fingerprint.read_bytes(), b'{"deps":[[1,"project",false,2]]}')
            self.assertEqual(library.read_bytes(), b"compiled-library")
            self.assertFalse((library.parent / "test-binary").exists())
            self.assertTrue((root / "target" / "tmp").is_dir())
            checksum = artifact / "compiler-inputs.tar.sha256"
            checksum.write_text("wrong")
            with mock.patch.object(ca, "ROOT", root), self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                ca.restore_test_environment(artifact, [{"compiler_inputs": True}])

    def test_locked_fetch_uses_network_only_when_dependency_cache_is_incomplete(self):
        for code in (0, 101):
            with self.subTest(code=code), \
                 mock.patch.object(subprocess, "check_output", return_value="host: x86_64-pc-windows-msvc\n"), \
                 mock.patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], code)) as check, \
                 mock.patch.object(ca, "run") as online, contextlib.redirect_stdout(io.StringIO()):
                ca.fetch_locked([])
            self.assertEqual(check.call_args.args[0], ["cargo", "metadata", *ca.CARGO_CONFIG, "--locked", "--filter-platform", "x86_64-pc-windows-msvc", "--format-version", "1", "--offline"])
            if code:
                online.assert_called_once_with(["cargo", "metadata", *ca.CARGO_CONFIG, "--locked", "--filter-platform", "x86_64-pc-windows-msvc", "--format-version", "1"], ca.os.devnull)
            else:
                online.assert_not_called()

    def test_normal_shard_needs_no_compiler_artifact(self):
        with tempfile.TemporaryDirectory(prefix="kinewright-ci-archive-") as directory:
            root = Path(directory)
            with mock.patch.object(ca, "ROOT", root):
                ca.restore_test_environment(root / "absent-artifact", [])
            self.assertTrue((root / "target" / "tmp").is_dir())


class DisplayInputs(unittest.TestCase):
    def witness(self, root):
        unit = {"display": True, "suite": 0,
                "tests": [cs.DISPLAY_PARITY_PREFIX + "talk_recut_matches_ten_frames_exactly"]}
        data = {"suites": [{"label": "kinewright_media", "path": "/target/debug/deps/kinewright_media-0123456789abcdef", "cwd": str(root)}]}
        return unit, data

    def write_canonical_input(self, root):
        directory, manifest, _ = di.paths("talk_recut")
        directory.mkdir()
        (directory / "0.mp4").write_bytes(b"original canonical video bytes")
        (directory / "document.json").write_text('{"original":"document"}')

        def reference(name):
            path = directory / name
            return {"path": str(path.relative_to(root)), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

        (directory / "media.json").write_text(json.dumps({"files": [reference("0.mp4")]}))
        manifest.write_text(json.dumps({"key": "talk_recut", "document": reference("document.json"), "media": reference("media.json")}))

    def prepare(self, root, unit, data):
        def generate(args, **kwargs):
            self.assertIn("--ignored", args)
            self.assertIn(di.PREPARE_TEST, args)
            self.assertEqual(kwargs["env"]["PF1_ONLY"], "talk_recut")
            self.assertNotIn("PF1_FROZEN_INPUT", kwargs["env"])
            self.write_canonical_input(root)
            kwargs["stdout"].write(log("kinewright_media", [di.PREPARE_TEST]).split("\n", 1)[1])

        with mock.patch.object(subprocess, "run", side_effect=generate), contextlib.redirect_stdout(io.StringIO()):
            di.prepare(data, [unit], root / "results")

    def test_canonical_preparation_is_reused_and_inputs_are_hash_checked(self):
        with tempfile.TemporaryDirectory(prefix="kinewright-display-input-") as directory:
            root = Path(directory)
            unit, data = self.witness(root)
            with mock.patch.object(di, "ROOT", root):
                self.prepare(root, unit, data)
                with mock.patch.object(subprocess, "run", side_effect=AssertionError("must reuse prepared bytes")), contextlib.redirect_stdout(io.StringIO()):
                    di.prepare(data, [unit], root / "results")
                env = di.environment(unit)
                self.assertEqual(env["PF1_ROOT"], str(root))
                self.assertEqual(Path(env["PF1_FROZEN_INPUT"]).name, "workload.json")
                self.assertNotIn("PF1_ONLY", env)
                (di.paths("talk_recut")[0] / "0.mp4").write_bytes(b"changed video bytes")
                with self.assertRaisesRegex(ValueError, "artifact SHA-256"):
                    di.environment(unit)

    def test_changed_manifest_missing_file_and_partial_preparation_fail(self):
        for fault in ("manifest", "missing", "partial", "identity", "escape"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory(prefix="kinewright-display-input-") as directory:
                root = Path(directory)
                unit, data = self.witness(root)
                with mock.patch.object(di, "ROOT", root):
                    self.prepare(root, unit, data)
                    folder, manifest, ready = di.paths("talk_recut")
                    if fault == "manifest":
                        manifest.write_bytes(manifest.read_bytes() + b"\n")
                    elif fault == "missing":
                        (folder / "0.mp4").unlink()
                    elif fault == "partial":
                        ready.unlink()
                    else:
                        value = json.loads(manifest.read_text())
                        if fault == "identity": value["key"] = "feed_4x5"
                        else: value["document"]["path"] = "../outside-input"
                        manifest.write_text(json.dumps(value))
                        ready.write_text(json.dumps({"manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest()}))
                    with contextlib.redirect_stdout(io.StringIO()), self.assertRaises((ValueError, FileNotFoundError)):
                        di.prepare(data, [unit], root / "results")

    def test_failed_or_empty_preparation_never_marks_the_cache_ready(self):
        for error in (subprocess.TimeoutExpired("prepare", 1800), subprocess.CalledProcessError(101, "prepare"), None):
            with self.subTest(error=error), tempfile.TemporaryDirectory(prefix="kinewright-display-input-") as directory:
                root = Path(directory)
                unit, data = self.witness(root)
                with mock.patch.object(di, "ROOT", root), \
                     mock.patch.object(subprocess, "run", side_effect=error), contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaises((subprocess.SubprocessError, ValueError, st.UnparsableLog)):
                        di.prepare(data, [unit], root / "results")
                    self.assertFalse(di.paths("talk_recut")[2].exists())


if __name__ == "__main__":
    unittest.main()

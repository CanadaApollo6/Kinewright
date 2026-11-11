#!/usr/bin/env python3
"""CI apt witnesses. Only fake package tools and loopback HTTP are executed."""
import contextlib
import http.server
import io
import json
import os
from pathlib import Path
import subprocess
import socket
import textwrap
import sys
import tempfile
import threading
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci_apt_mirrors as mirrors

ROOT = Path(__file__).resolve().parent.parent
PACKAGES = subprocess.check_output(
    ["bash", str(ROOT / "scripts/install-linux-deps.sh"), "--print-apt-packages"], text=True).splitlines()
FAKE = r'''#!/usr/bin/env python3
import json,os,sys,time
from pathlib import Path
root=Path(os.environ['TEST_ROOT']);command=Path(sys.argv[0]).name;args=sys.argv[1:]
with (root/'commands.jsonl').open('a') as out:out.write(json.dumps([command,*args])+'\n')
state=json.loads((root/'installed.json').read_text())
if command=='sudo':
 while args and args[0]=='-n':args.pop(0)
 if args[0]=='cp':
  assert Path(args[-1]).resolve().is_relative_to(root)
 if '=' in args[0]:args.insert(0,'env')
 os.execvp(args[0],args)
if command=='dpkg-query':
 pkg=args[-1]
 if pkg not in state:sys.exit(1)
 print(state[pkg] if 'Version' in args[1] else 'installed ok',end='');sys.exit(0)
if command=='dpkg':
 if args==['--print-architecture']:print('amd64');sys.exit(0)
 left,op,right=args[1:];sys.exit(0 if (int(left)>int(right) if op=='gt' else int(left)>=int(right)) else 1)
if command=='dpkg-deb':
 data=json.loads(Path(args[-1]).read_text());print(data['name'],data['version'],data.get('arch','amd64'));sys.exit(0)
if command=='curl':sys.exit(7 if 'unreachable.invalid' in args[-1] else 0)
if command=='apt-get':
 if 'update' in args and os.environ.get('TEST_STALL')=='1':
  archives=Path(next(x.split('=',1)[1] for x in args if x.startswith('Dir::Cache::archives=')))
  (archives/'downloaded-before-timeout.deb').write_bytes(b'completed download')
  (archives/'partial').mkdir(exist_ok=True);(archives/'partial'/'unfinished.deb').write_bytes(b'incomplete')
  (archives/'lock').touch();time.sleep(20)
 if '--no-download' in args:
  assert 'Dir::Etc::sourcelist=/dev/null' in args and 'Dir::Etc::sourceparts=-' in args
  if os.environ.get('TEST_CLOSURE')=='partial':sys.exit(100)
  assert any(Path(a).name.startswith('transitive_') for a in args if a.endswith('.deb'))
 if 'install' in args:
  for pkg in json.loads((root/'packages.json').read_text()):state[pkg]='2'
  (root/'installed.json').write_text(json.dumps(state))
 sys.exit(0)
raise AssertionError(command)
'''


def collection_script():
    action = (ROOT / ".github/actions/ci-native/action.yml").read_text()
    block = action.split("id: apt-collect", 1)[1].split("run: |\n", 1)[1].split("\n    #", 1)[0]
    return textwrap.dedent(block)


def collect(home):
    output = home / "output"
    output.write_text("")
    subprocess.run(["bash", "-c", collection_script()], env={**os.environ, "HOME": str(home),
                   "RUNNER_TEMP": str(home), "GITHUB_OUTPUT": str(output)},
                   capture_output=True, text=True, check=True)
    return output.read_text()


class AptPath(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="kinewright-ci-apt-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for name in ["apt-get", "sudo", "dpkg", "dpkg-query", "dpkg-deb", "curl"]:
            p = self.bin / name
            p.write_text(FAKE)
            p.chmod(0o755)
        self.cache = self.root / "apt-archives"
        self.cache.mkdir()
        self.mirror = self.root / "mirrors.txt"
        self.mirror.write_text("http://unreachable.invalid/ubuntu/\tpriority:1\nhttps://archive.ubuntu.com/ubuntu/\tpriority:2\nhttps://security.ubuntu.com/ubuntu/\tpriority:3\n")
        self.source = self.root / "ubuntu.sources"
        self.source.write_text(f"URIs: mirror+file:{self.mirror}\nSuites: noble noble-updates\n")
        self.installed({})
        (self.root / "packages.json").write_text(json.dumps(PACKAGES))
        self.env = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
                    "TEST_ROOT": str(self.root), "GITHUB_ACTIONS": "true", "RUNNER_OS": "Linux",
                    "RUNNER_TEMP": str(self.root), "KINEWRIGHT_APT_CACHE": str(self.cache),
                    "KINEWRIGHT_APT_SUITE": "noble", "KINEWRIGHT_APT_MIRRORS": str(self.mirror), "KINEWRIGHT_APT_SOURCES": str(self.source)}

    def installed(self, state):
        (self.root / "installed.json").write_text(json.dumps(state))

    def deb(self, name, version=2, arch="amd64"):
        p = self.cache / f"{name}_{version}_{arch}.deb"
        p.write_text(json.dumps({"name": name, "version": str(version), "arch": arch}))
        return p

    def full_cache(self):
        for p in [*PACKAGES, "transitive"]:
            self.deb(p)

    def run_installer(self, **env):
        # The exact command and cwd are owned here; GNU timeout only signals its
        # fake package-tool child, never a pre-existing process.
        return subprocess.run(["bash", str(ROOT / "scripts/ci-linux-deps.sh")], cwd=ROOT,
                              env={**self.env, **env}, capture_output=True, text=True, timeout=8)

    def commands(self):
        p = self.root / "commands.jsonl"
        return [json.loads(x) for x in p.read_text().splitlines()] if p.exists() else []

    def test_installed_needs_neither_indexes_nor_mirror_file(self):
        self.installed(dict.fromkeys(PACKAGES, "2"))
        self.mirror.unlink()
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(x[0] in ["apt-get", "curl"] for x in self.commands()))

    def test_full_cache_uses_local_debs_and_transitive_closure_without_network(self):
        self.full_cache()
        original = self.mirror.read_bytes()
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [x for x in self.commands() if x[0] == "apt-get"]
        self.assertEqual(len(calls), 1)
        self.assertIn("--no-download", calls[0])
        self.assertIn("Dir::Etc::sourceparts=-", calls[0])
        self.assertFalse(any(x[0] == "curl" for x in self.commands()))
        self.assertEqual(self.mirror.read_bytes(), original)

    def test_partial_top_level_cache_goes_online_with_bounded_network(self):
        self.deb(PACKAGES[0])
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [x for x in self.commands() if x[0] == "apt-get"]
        self.assertEqual(len(calls), 2)
        self.assertIn("update", calls[0])
        for call in calls:
            for option in ["Acquire::http::Timeout=5", "Acquire::https::Timeout=5", "Acquire::Retries=0"]:
                self.assertIn(option, call)
        self.assertNotIn("unreachable.invalid", self.mirror.read_text())
        self.assertIn("security.ubuntu.com", self.mirror.read_text())

    def test_cached_top_level_with_missing_transitive_dependency_is_partial(self):
        self.full_cache()
        result = self.run_installer(TEST_CLOSURE="partial")
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [x for x in self.commands() if x[0] == "apt-get"]
        self.assertEqual(len(calls), 3)
        self.assertIn("--no-download", calls[0])
        self.assertIn("update", calls[1])

    def test_newest_native_deb_and_no_installed_downgrade(self):
        self.full_cache()
        self.deb(PACKAGES[0], 1)
        self.deb(PACKAGES[0], 9, "arm64")
        self.installed({"transitive": "3"})
        # This case checks selection; the fake solver needs no transitive file
        # when its installed version already satisfies the closure.
        fake = (self.bin / "apt-get").read_text().replace("  assert any(Path(a).name.startswith('transitive_') for a in args if a.endswith('.deb'))", "  assert 'transitive' in state")
        (self.bin / "apt-get").write_text(fake)
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        call = next(x for x in self.commands() if x[0] == "apt-get")
        self.assertIn(str(self.cache / f"{PACKAGES[0]}_2_amd64.deb"), call)
        self.assertFalse(any(x.endswith("_1_amd64.deb") or x.endswith("_arm64.deb") or "transitive_" in x for x in call))

    def test_deadline_names_mirror_and_keeps_downloaded_archive(self):
        archive = self.deb(PACKAGES[0])
        started = time.monotonic()
        result = self.run_installer(TEST_STALL="1", KINEWRIGHT_APT_DEADLINE="2")
        self.assertEqual(result.returncode, 124, result.stderr)
        self.assertLess(time.monotonic() - started, 5)
        self.assertIn("timed out after 2s", result.stderr)
        self.assertIn("archive.ubuntu.com", result.stderr)
        self.assertTrue(archive.is_file())
        self.assertIn("digest=", collect(self.root))
        self.assertTrue((self.cache / "downloaded-before-timeout.deb").is_file())
        self.assertFalse((self.cache / "partial").exists())
        self.assertFalse((self.cache / "lock").exists())
        self.assertFalse((self.cache / "unfinished.deb").exists())
        self.assertTrue(any("update" in x for x in self.commands() if x[0] == "apt-get"))

    def test_developer_default_still_uses_the_original_update_and_install_path(self):
        original = self.mirror.read_bytes()
        result = subprocess.run(["bash", str(ROOT / "scripts/install-linux-deps.sh")], cwd=ROOT,
                                env={**self.env, "GITHUB_ACTIONS": "false"}, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [x for x in self.commands() if x[0] == "apt-get"]
        self.assertEqual(calls[0], ["apt-get", "update"])
        self.assertEqual(calls[1], ["apt-get", "install", "-y", "--no-install-recommends", *PACKAGES])
        self.assertEqual(self.mirror.read_bytes(), original)

    def test_ci_path_is_explicit(self):
        result = self.run_installer(GITHUB_ACTIONS="false")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(self.commands(), [])


class Mirrors(unittest.TestCase):
    def test_stalled_first_mirror_is_dropped_with_a_real_curl_deadline(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if self.server.stalled:
                    time.sleep(0.4)
                    return
                self.send_response(200)
                self.end_headers()
            def log_message(self, *_):
                pass
        servers = []
        for stalled in [True, False]:
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            server.stalled = stalled
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            servers.append(server)
            self.addCleanup(server.server_close)
            self.addCleanup(server.shutdown)
        with tempfile.TemporaryDirectory(prefix="kinewright-apt-mirrors-") as directory:
            source, destination = Path(directory) / "input", Path(directory) / "output"
            dead = f"http://127.0.0.1:{servers[0].server_port}/ubuntu/"
            live = f"http://127.0.0.1:{servers[1].server_port}/ubuntu/"
            original = f"{dead}\tpriority:1\n{live}\tpriority:2\nhttps://security.ubuntu.com/ubuntu/\tpriority:3\n"
            source.write_text(original)
            started = time.monotonic()
            with contextlib.redirect_stdout(io.StringIO()):
                mirrors.select(source, destination, "noble", 0.15)
            self.assertLess(time.monotonic() - started, 2)
            self.assertEqual(source.read_text(), original)
            self.assertEqual(destination.read_text(), original.split("\n", 1)[1])

    def test_all_mirrors_unreachable_fails_without_writing_a_selection(self):
        with socket.socket() as closed_listener, tempfile.TemporaryDirectory(prefix="kinewright-apt-dead-") as directory:
            closed_listener.bind(("127.0.0.1", 0))
            url = f"http://127.0.0.1:{closed_listener.getsockname()[1]}/ubuntu/"
            source, destination = Path(directory) / "input", Path(directory) / "output"
            source.write_text(url + "\tpriority:1\n")
            original = source.read_bytes()
            with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(RuntimeError, "No apt mirror"):
                mirrors.select(source, destination, "noble", 0.15)
            self.assertFalse(destination.exists())
            self.assertEqual(source.read_bytes(), original)


class Workflow(unittest.TestCase):
    def test_all_ci_native_calls_are_bounded_and_witness_is_scheduled(self):
        for name in ["ci.yml", "ci-tests.yml", "release.yml"]:
            text = (ROOT / ".github/workflows" / name).read_text()
            calls = text.split("uses: ./.github/actions/ci-native")[1:]
            self.assertTrue(calls, name)
            self.assertTrue(all(x.startswith("\n        timeout-minutes: 8") for x in calls), name)
        self.assertIn("python scripts/ci_linux_deps_selftest.py", (ROOT / ".github/workflows/ci.yml").read_text())

    def test_partial_download_growth_changes_the_saved_content_key(self):
        with tempfile.TemporaryDirectory(prefix="kinewright-cache-content-") as directory:
            home = Path(directory)
            active = home / "kinewright-apt/archives"
            active.mkdir(parents=True)
            self.assertEqual(collect(home), "")
            (active / "first.deb").write_bytes(b"first complete archive")
            partial = collect(home)
            self.assertEqual(collect(home), partial)
            (active / "second.deb").write_bytes(b"second complete archive")
            self.assertNotEqual(collect(home), partial)

    def test_failed_installs_save_partial_content_and_prefer_complete_cache(self):
        text = (ROOT / ".github/actions/ci-native/action.yml").read_text()
        self.assertEqual(text.count("always() && runner.os == 'Linux'"), 2)
        self.assertIn("steps.apt-collect.outputs.digest", text)
        self.assertIn("'complete' || 'partial'", text)
        self.assertLess(text.index("-complete-"), text.index("-partial-"))
        self.assertNotIn("autoclean", text)
        self.assertNotIn("cache-hit != 'true'", text)


if __name__ == "__main__":
    unittest.main()

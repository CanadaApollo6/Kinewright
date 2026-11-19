#!/usr/bin/env python3
"""Native dependency and CI workflow witnesses."""
import contextlib
import http.server
import io
import json
import os
import re
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
if command=='curl':
 sys.exit(7 if os.environ.get('TEST_MIRRORS_UNREACHABLE')=='1' or 'unreachable.invalid' in args[-1] else 0)
if command=='sleep':sys.exit(0)
if command=='apt-get':
 lists=Path(next((x.split('=',1)[1] for x in args if x.startswith('Dir::State::lists=')), root/'image-lists'))
 assert lists.resolve().is_relative_to(root)
 if 'update' in args:
  if os.environ.get('TEST_STALL')=='1':
   archives=Path(next(x.split('=',1)[1] for x in args if x.startswith('Dir::Cache::archives=')))
   (archives/'downloaded-before-timeout.deb').write_bytes(b'completed download')
   (archives/'partial').mkdir(exist_ok=True);(archives/'partial'/'unfinished.deb').write_bytes(b'incomplete')
   (archives/'lock').touch();time.sleep(20)
  attempts=root/'update-attempts'
  attempt=int(attempts.read_text())+1 if attempts.exists() else 1
  attempts.write_text(str(attempt))
  lists.mkdir(exist_ok=True)
  (lists/'noble').touch()
  if attempt <= int(os.environ.get('TEST_SECURITY_FAILURES','0')):
   print('Failed to fetch noble-security InRelease',file=sys.stderr)
   sys.exit(100 if '--error-on=any' in args else 0)
  (lists/'noble-security').touch()
 if '--no-download' in args:
  assert 'Dir::Etc::sourcelist=/dev/null' in args and 'Dir::Etc::sourceparts=-' in args
  if os.environ.get('TEST_CLOSURE')=='partial':sys.exit(100)
  assert any(Path(a).name.startswith('transitive_') for a in args if a.endswith('.deb'))
 if 'install' in args:
  if '--no-download' not in args:
   assert (lists/'noble').is_file()
   if os.environ.get('TEST_NEEDS_SECURITY')=='1' and not (lists/'noble-security').is_file():
    print('Unable to locate package '+args[-1],file=sys.stderr)
    sys.exit(100)
  for pkg in json.loads((root/'packages.json').read_text()):state[pkg]='2'
  (root/'installed.json').write_text(json.dumps(state))
 sys.exit(0)
raise AssertionError(command)
'''


def collection_script():
    action = (ROOT / ".github/actions/ci-apt/action.yml").read_text()
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
        for name in ["apt-get", "sudo", "dpkg", "dpkg-query", "dpkg-deb", "curl", "sleep"]:
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
        self.assertIn("Acquire::Retries=0", calls[0])
        self.assertNotIn("Acquire::Retries=3", calls[0])
        self.assertFalse(any(x[0] == "curl" for x in self.commands()))
        self.assertEqual(self.mirror.read_bytes(), original)

    def test_partial_top_level_cache_goes_online_with_bounded_network(self):
        self.deb(PACKAGES[0])
        self.installed({PACKAGES[1]: "2"})
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        missing_names = " ".join(PACKAGES[2:])
        self.assertIn(f"Missing cached debs: {missing_names}", result.stdout.splitlines())
        self.assertNotIn("Cache-only apt install failed", result.stderr)
        calls = [x for x in self.commands() if x[0] == "apt-get"]
        self.assertEqual(len(calls), 2)
        self.assertIn("update", calls[0])
        for call in calls:
            for option in ["Acquire::http::Timeout=5", "Acquire::https::Timeout=5"]:
                self.assertIn(option, call)
        self.assertNotIn("unreachable.invalid", self.mirror.read_text())
        self.assertIn("security.ubuntu.com", self.mirror.read_text())

    def test_package_fetch_retries_apply_only_to_online_install(self):
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        update, install = [x for x in self.commands() if x[0] == "apt-get"]
        self.assertIn("Acquire::Retries=0", update)
        self.assertNotIn("Acquire::Retries=3", update)
        self.assertIn("Acquire::Retries=3", install)
        self.assertNotIn("--allow-unauthenticated", install)
        self.assertNotIn("--fix-missing", install)

    def test_all_mirror_probes_can_fail_before_index_refresh_recovers(self):
        original = self.mirror.read_bytes()
        result = self.run_installer(TEST_MIRRORS_UNREACHABLE="1", TEST_SECURITY_FAILURES="1",
                                    TEST_NEEDS_SECURITY="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Apt mirror probe failed", result.stderr)
        self.assertEqual(len([x for x in self.commands() if x[0] == "curl"]), 3)
        self.assertEqual(self.mirror.read_bytes(), original)
        self.assertEqual((self.root / "update-attempts").read_text(), "2")
        self.assertEqual(json.loads((self.root / "installed.json").read_text()), dict.fromkeys(PACKAGES, "2"))

    def test_direct_ubuntu_sources_install_without_probing_or_rewriting_mirrors(self):
        ubuntu_source = (
            "Types: deb\n"
            "URIs: http://us-east-2.ec2.archive.ubuntu.com/ubuntu/\n"
            "Suites: noble noble-updates\n"
            "Components: main universe\n"
            "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n\n"
            "Types: deb\n"
            "URIs: http://security.ubuntu.com/ubuntu/\n"
            "Suites: noble-security\n"
            "Components: main universe\n"
            "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n"
        )
        self.source.write_text(ubuntu_source)
        original_mirrors = self.mirror.read_bytes()
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Skipping the mirror-list rewrite.", result.stdout)
        self.assertFalse(any(x[0] == "curl" for x in self.commands()))
        self.assertEqual(self.mirror.read_bytes(), original_mirrors)
        for line in ubuntu_source.splitlines():
            if line.startswith(("URIs:", "Suites:")):
                self.assertIn(line, result.stdout)
            elif line:
                self.assertNotIn(line, result.stdout + result.stderr)
        for call in (x for x in self.commands() if x[0] == "apt-get"):
            self.assertIn("Dir::Etc::sourcelist=/dev/null", call)
            source_dir = Path(next(x.split("=", 1)[1] for x in call if x.startswith("Dir::Etc::sourceparts=")))
            self.assertEqual(list(source_dir.iterdir()), [source_dir / "ubuntu.sources"])
            self.assertEqual((source_dir / "ubuntu.sources").read_text(), ubuntu_source)
        self.assertEqual(json.loads((self.root / "installed.json").read_text()), dict.fromkeys(PACKAGES, "2"))

    def test_security_index_succeeds_on_fifth_attempt_after_backoff(self):
        result = self.run_installer(TEST_SECURITY_FAILURES="4", TEST_NEEDS_SECURITY="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        steps = [x[1] if x[0] == "sleep" else ("update" if "update" in x else "install")
                 for x in self.commands() if x[0] in ("apt-get", "sleep")]
        self.assertEqual(steps, ["update", "2", "update", "4", "update", "8", "update", "16", "update", "install"])
        self.assertEqual(json.loads((self.root / "installed.json").read_text()), dict.fromkeys(PACKAGES, "2"))

    def test_persistent_security_failure_installs_from_refreshed_indexes(self):
        result = self.run_installer(TEST_SECURITY_FAILURES="99")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "update-attempts").read_text(), "5")
        self.assertEqual(json.loads((self.root / "installed.json").read_text()), dict.fromkeys(PACKAGES, "2"))

    def test_persistent_security_failure_cannot_resolve_from_stale_image_index(self):
        stale_lists = self.root / "image-lists"
        stale_lists.mkdir()
        (stale_lists / "noble-security").touch()
        result = self.run_installer(TEST_SECURITY_FAILURES="99", TEST_NEEDS_SECURITY="1")
        self.assertEqual(result.returncode, 100, result.stderr)
        self.assertIn("Unable to locate package " + PACKAGES[-1], result.stderr)
        self.assertIn("::error::Native dependency installation failed with exit 100; apt mirrors:", result.stderr)
        self.assertEqual((self.root / "update-attempts").read_text(), "5")
        self.assertEqual(json.loads((self.root / "installed.json").read_text()), {})

    def test_cached_top_level_with_missing_transitive_dependency_is_partial(self):
        self.full_cache()
        result = self.run_installer(TEST_CLOSURE="partial")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Cache-only apt install failed", result.stderr)
        self.assertNotIn("Missing cached debs:", result.stdout)
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


def check_download_attempts(text):
    blocks = [block for block in re.split(r'(?m)^      - ', text)[1:]
              if 'uses: actions/download-artifact@' in block]
    assert len(blocks) == 10, 'five downloads, each with one retry'
    for first, retry in zip(blocks[::2], blocks[1::2]):
        step = re.search(r'(?m)^        id: (\S+)$', first)
        assert step, 'initial attempt needs an outcome id'
        assert '        continue-on-error: true\n' in first
        assert 'continue-on-error:' not in retry
        for block in (first, retry):
            assert re.search(r'(?m)^        timeout-minutes: 3$', block), step[1]
        assert f"steps.{step[1]}.outcome == 'failure'" in retry
        assert '!cancelled()' in retry
        payload = lambda block: re.search(r'        with:\n((?:          [^\n]*\n)+)', block)[1]
        assert payload(first) == payload(retry), 'retry must request the same artifact'
        assert re.search(r'uses: (\S+)', first)[1] == re.search(r'uses: (\S+)', retry)[1]
        if 'compiler-inputs-' in first:
            assert "if: steps.requirements.outputs.compiler_inputs == 'true'" in first


class Workflow(unittest.TestCase):
    def test_linux_shipping_builds_are_isolated_and_precede_clippy(self):
        text = (ROOT / '.github/workflows/ci.yml').read_text()
        block = text.split('      - name: Linux shipping builds\n', 1)[1].split('      - ', 1)[0]
        self.assertIn("if: runner.os == 'Linux'", block)
        self.assertIn('source scripts/setup-ffmpeg.sh || exit 1', block)
        self.assertIn('set -euo pipefail', block)
        self.assertEqual(re.findall(r'^          (cargo build .+)$', block, re.M), [
            'cargo build -p kinewright-agent --no-default-features --lib',
            'cargo build -p kinewright-app --no-default-features --bin kinewright-app',
            'cargo build -p kinewright-agent --no-default-features --bin kinewright',
            'cargo build -p kinewright-media-helper -p kinewright-media-supervisor --bins',
        ])
        self.assertLess(text.index('name: Linux shipping builds'), text.index('cargo clippy --workspace'))

    def test_artifact_downloads_have_bounded_attempts_and_one_retry(self):
        text = (ROOT / '.github/workflows/ci-tests.yml').read_text()
        check_download_attempts(text)
        for old, new in [('timeout-minutes: 3', 'timeout-minutes: 4'),
                         ('continue-on-error: true', 'continue-on-error: false'),
                         ('.outcome', '.conclusion')]:
            with self.subTest(wrong_change=old), self.assertRaises(AssertionError):
                check_download_attempts(text.replace(old, new, 1))


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
        text = (ROOT / ".github/actions/ci-apt/action.yml").read_text()
        self.assertEqual(text.count("always() && runner.os == 'Linux'"), 2)
        self.assertIn("steps.apt-collect.outputs.digest", text)
        self.assertIn("'complete' || 'partial'", text)
        self.assertLess(text.index("-complete-"), text.index("-partial-"))
        self.assertNotIn("autoclean", text)
        self.assertNotIn("cache-hit != 'true'", text)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Source admission must fail before extraction, configure, or install replacement."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('ffmpeg_builder', Path(__file__).with_name('build-ffmpeg.py'))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class SourceAdmission(unittest.TestCase):
    def test_bad_source_never_executes_or_replaces_an_install(self):
        scratch = builder.ROOT / 'target'
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            install = root / 'third_party/ffmpeg'
            install.mkdir(parents=True)
            sentinel = install / 'keep'
            sentinel.write_bytes(b'previous verified install')
            def download(_url, target):
                Path(target).write_bytes(b'unverified input')
            with patch.object(builder, 'ROOT', root), \
                 patch.object(builder, 'ensure_nasm'), \
                 patch.object(builder, 'fingerprint', return_value=('test-key', {})), \
                 patch.object(builder.urllib.request, 'urlretrieve', download), \
                 patch.object(builder, 'run') as run, \
                 patch.object(builder.tarfile, 'open') as extract:
                with self.assertRaisesRegex(SystemExit, 'checksum mismatch'):
                    builder.build('libraries')
                run.assert_not_called()
                extract.assert_not_called()
            self.assertEqual(sentinel.read_bytes(), b'previous verified install')
            self.assertEqual(list((root / 'target/ffmpeg-build').iterdir()), [])

    def test_recorder_has_no_file_demuxers_or_test_generators(self):
        args = builder.configure('recorder')
        self.assertFalse(any(a.startswith('--enable-demuxer') for a in args))
        self.assertFalse(any('lavfi' in a for a in args))
        self.assertFalse(any(a.startswith('--enable-parser') for a in args))
        self.assertIn('--disable-network', args)
        self.assertIn('--disable-autodetect', args)
        self.assertIn('--disable-ffprobe', args)
        self.assertIn('--enable-protocol=pipe', builder.configure('testgen'))
        self.assertNotIn('--enable-protocol=pipe', args)
        with patch.object(builder.platform, 'system', return_value='Windows'):
            windows = builder.configure('recorder')
        enabled = next(a for a in windows if a.startswith('--enable-decoder='))
        self.assertIn('bmp', enabled.split('=', 1)[1].split(','))


class BuildIdentity(unittest.TestCase):
    def test_linker_query_nonce_is_not_part_of_the_tool_identity(self):
        from types import SimpleNamespace
        with patch.object(builder.shutil, 'which', return_value='/tool/gcc'), \
             patch.object(builder, 'file_digest', return_value='compiler-digest'), \
             patch.object(builder.subprocess, 'run') as query:
            query.return_value = SimpleNamespace(stdout='gcc 15.2 -plugin-opt=-fresolution=/tmp/ccAAA.res GNU ld 2.45')
            first = builder.tool_identity(['gcc'], ['-Wl,--version'])
            query.return_value = SimpleNamespace(stdout='gcc 15.2 -plugin-opt=-fresolution=/tmp/ccBBB.res GNU ld 2.45')
            self.assertEqual(builder.tool_identity(['gcc'], ['-Wl,--version']), first)
            query.return_value = SimpleNamespace(stdout='gcc 15.2 -plugin-opt=-fresolution=/tmp/ccBBB.res GNU ld 2.46')
            self.assertNotEqual(builder.tool_identity(['gcc'], ['-Wl,--version']), first)

    def test_ci_and_local_use_the_same_fingerprint_and_source_only_ignores_it(self):
        with tempfile.TemporaryDirectory(dir=builder.ROOT/'target') as directory:
            root = Path(directory)
            prefix = root/'third_party/ffmpeg'
            prefix.mkdir(parents=True)
            (prefix/'.build-key').write_text('shared-fingerprint')
            with patch.object(builder, 'ROOT', root), \
                 patch.object(builder, 'fingerprint', return_value=('shared-fingerprint', {})), \
                 patch.object(builder.urllib.request, 'urlretrieve', side_effect=RuntimeError('fresh verified-source download')) as download:
                builder.build('libraries')
                download.assert_not_called()
                with self.assertRaisesRegex(RuntimeError, 'fresh verified-source download'):
                    builder.build('libraries', no_binary_cache=True)
                download.assert_called_once()

    def test_fingerprint_changes_with_tool_dependency_flag_or_setup_changes(self):
        from types import SimpleNamespace
        import os
        def query(command, **_kwargs):
            return SimpleNamespace(returncode=0, stdout='1.0')
        with patch.object(builder, 'ensure_nasm'), \
             patch.object(builder, 'tool_identity', return_value={'version': 'tool-1'}) as tool, \
             patch.object(builder.subprocess, 'check_output', return_value='ld'), \
             patch.object(builder.subprocess, 'run', side_effect=query) as dependency:
            baseline, _ = builder.fingerprint()
            tool.return_value = {'version': 'tool-2'}
            self.assertNotEqual(builder.fingerprint()[0], baseline)
            tool.return_value = {'version': 'tool-1'}
            dependency.side_effect = lambda command, **kwargs: SimpleNamespace(returncode=0, stdout='2.0' if '--modversion' in command and command[-1]=='x264' else '1.0')
            self.assertNotEqual(builder.fingerprint()[0], baseline)
            dependency.side_effect = query
            with patch.dict(os.environ, {'CFLAGS': '-DMS1_FINGERPRINT_WITNESS'}):
                self.assertNotEqual(builder.fingerprint()[0], baseline)
            actual_digest = builder.file_digest
            with patch.object(builder, 'file_digest', side_effect=lambda path: 'changed' if str(path).endswith('setup-ffmpeg.sh') else actual_digest(path)):
                self.assertNotEqual(builder.fingerprint()[0], baseline)

    def test_windows_pins_msys_make_and_shell_even_when_host_make_is_first(self):
        import os
        # The default root is the subject here; the CI root has its own witness below.
        with patch.object(builder.platform, 'system', return_value='Windows'), \
             patch.dict(os.environ), \
             patch.object(builder.subprocess, 'run') as run:
            os.environ.pop('KINEWRIGHT_MSYS_ROOT', None)
            builder.run(['make', '-j4', 'install'], builder.ROOT)
        command = run.call_args.args[0]
        self.assertEqual(command[0].replace('\\', '/'), 'C:/msys64/usr/bin/make.exe')
        self.assertEqual(command[1:], ['SHELL=/usr/bin/sh', '-j4', 'install'])
        self.assertNotIn('shell', run.call_args.kwargs)

    def test_ci_uses_the_action_installation_instead_of_the_runner_preinstall(self):
        import os
        with patch.object(builder.platform, 'system', return_value='Windows'), \
             patch.dict(os.environ, {'KINEWRIGHT_MSYS_ROOT': 'D:/a/_temp/msys64'}):
            commands = builder.tool_commands()
            self.assertEqual(commands['make'][0].replace('\\', '/'), 'D:/a/_temp/msys64/usr/bin/make.exe')
            self.assertEqual(commands['shell'][0].replace('\\', '/'), 'D:/a/_temp/msys64/usr/bin/sh.exe')
        workflow = (builder.ROOT/'.github/actions/ffmpeg/action.yml').read_text()
        self.assertIn('MSYS_ROOT: ${{ steps.msys.outputs.msys2-location }}', workflow)
        self.assertIn('cache: false', workflow)

    def test_depfile_backslash_round_trip(self):
        builder.depfile_shell_witness()

    def test_release_workflows_and_packagers_require_source_only_flag(self):
        release = (builder.ROOT/'.github/workflows/release.yml').read_text()
        self.assertNotIn('actions/cache', release)
        self.assertEqual(release.count("no-binary-cache: 'true'"), 2)
        self.assertIn('-NoBinaryCache', release)
        self.assertIn('--no-binary-cache', release)
        package = builder.ROOT/'scripts/package-linux.sh'
        result = builder.subprocess.run(['bash', package.as_posix(), '--version', '0.1.0', '--stage-only'],
                                        capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('requires --no-binary-cache', result.stderr)
        windows = (builder.ROOT/'scripts/package-windows.ps1').read_text()
        self.assertIn('if (-not $NoBinaryCache)', windows)
        self.assertIn('-NoBinaryCache -SkipBuildCachePrune', windows)

    def test_native_cache_is_push_write_only_and_forks_do_not_restore(self):
        workflow = (builder.ROOT/'.github/actions/ffmpeg/action.yml').read_text()
        restore = workflow[workflow.index('    - name: Restore trusted'):workflow.index('    - name: Consumer Windows')]
        save = workflow[workflow.index('    - name: Save binaries'):]
        self.assertIn("github.event.pull_request.head.repo.full_name == github.repository", restore)
        self.assertIn("github.repository == 'CanadaApollo6/Kinewright'", restore)
        self.assertIn("github.event_name == 'push'", save)
        self.assertIn("inputs.no-binary-cache != 'true'", save)
        self.assertIn('ms1-${{ steps.fingerprint.outputs.key }}', save)
        self.assertNotIn('restore-keys', workflow)


if __name__ == '__main__':
    unittest.main()

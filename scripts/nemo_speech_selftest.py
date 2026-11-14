"""SDK cache witnesses without network, builds, models or listening sockets."""
import contextlib
import importlib.util
import io
import os
import stat
import subprocess
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('nemo_speech', SCRIPTS / 'nemo_speech.py')
nemo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nemo)


class InstalledSdk(unittest.TestCase):
    def setUp(self):
        (SCRIPTS.parent / 'target').mkdir(exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix='kinewright-nemo-', dir=SCRIPTS.parent / 'target')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for path in ('lib/libnemo_speech_asr_c.so', 'lib/libnemo_speech_asr.so',
                     'lib/nemo_speech_asr_c.lib', 'bin/nemo_speech_asr_c.dll', 'bin/nemo_speech_asr.dll',
                     'include/nemo_speech/asr.h', 'share/licenses/nemo-speech/LICENSE',
                     'share/licenses/nemo-speech/NOTICE', 'share/licenses/nemo-speech/THIRD_PARTY_NOTICES.md',
                     'share/licenses/nemo-speech/third_party/sentencepiece/LICENSE'):
            file = self.root / path
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(b'sdk')
        (self.root / '.build-key').write_text('key\n')

    def test_installed_sdk_survives_removing_all_build_inputs(self):
        for directory in nemo.BUILD_TREES:
            path = self.root / directory
            path.mkdir()
            (path / 'scratch').write_bytes(b'build-only')
        self.assertTrue(nemo.sdk_complete(self.root, 'key'))
        nemo.provision(self.root, 'key')
        self.assertTrue(nemo.sdk_complete(self.root, 'key'))
        self.assertFalse(any((self.root / directory).exists() for directory in nemo.BUILD_TREES))
        self.assertEqual({path.name for path in self.root.iterdir()}, {'.build-key', 'lib', 'bin', 'include', 'share'})
        nemo.provision(self.root, 'key')  # Reuse needs neither a fetch nor CMake.

    def test_readonly_git_pack_is_removed_after_success(self):
        pack = self.root / 'src/.git/objects/pack/source.idx'
        pack.parent.mkdir(parents=True)
        pack.write_bytes(b'pinned pack')
        pack.chmod(stat.S_IREAD)
        # Windows rejects unlink before chmod; POSIX also runs this same tree witness.
        nemo.provision(self.root, 'key')
        self.assertFalse((self.root / 'src').exists())
        self.assertTrue(nemo.sdk_complete(self.root, 'key'))

    def test_permission_retry_makes_the_file_writable(self):
        file = self.root / 'readonly'
        file.write_bytes(b'pack')
        file.chmod(stat.S_IREAD)
        calls = []
        def retry(path):
            self.assertTrue(os.stat(path).st_mode & stat.S_IWRITE)
            calls.append(path)
            os.unlink(path)
        nemo.retry_readonly(retry, file, (PermissionError, PermissionError('read only'), None))
        self.assertEqual(calls, [file])
        self.assertFalse(file.exists())

    def test_leftover_cleanup_warns_without_invalidating_complete_sdk(self):
        (self.root / 'src').mkdir()
        output = io.StringIO()
        with patch.object(nemo, 'remove_tree', side_effect=PermissionError('busy file')), contextlib.redirect_stderr(output):
            nemo.provision(self.root, 'key')
        self.assertIn('Warning: could not remove NeMo build tree', output.getvalue())
        self.assertTrue(nemo.sdk_complete(self.root, 'key'))

    def test_fetch_enables_long_paths_before_checkout_and_propagates_failure(self):
        calls = []
        def git(args):
            calls.append(args)
            if 'checkout' in args:
                raise subprocess.CalledProcessError(1, args)
        with patch.object(nemo, 'run', side_effect=git), self.assertRaises(subprocess.CalledProcessError):
            nemo.fetch(self.root / 'checkout', {'url': 'official', 'commit': 'pin'})
        config = next(i for i, args in enumerate(calls) if 'core.longpaths' in args)
        checkout = next(i for i, args in enumerate(calls) if 'checkout' in args)
        self.assertLess(config, checkout)
        self.assertEqual(calls[config][-2:], ['core.longpaths', 'true'])

    def test_missing_runtime_dependency_or_notice_refuses_completeness(self):
        for file in ('lib/libnemo_speech_asr.so', 'bin/nemo_speech_asr.dll',
                     'share/licenses/nemo-speech/third_party/sentencepiece/LICENSE'):
            with self.subTest(file=file):
                path = self.root / file
                path.unlink()
                if not (file.startswith('bin') and not nemo.WINDOWS) and not (file.startswith('lib/lib') and nemo.WINDOWS):
                    self.assertFalse(nemo.sdk_complete(self.root, 'key'))
                path.write_bytes(b'sdk')
        self.assertFalse(nemo.sdk_complete(self.root, 'other-key'))

    def test_windows_sdk_requires_the_cpp_dll_and_c_import_library(self):
        with patch.object(nemo, 'WINDOWS', True):
            self.assertTrue(nemo.sdk_complete(self.root, 'key'))
            for name in ('bin/nemo_speech_asr.dll', 'lib/nemo_speech_asr_c.lib'):
                path = self.root / name
                path.unlink()
                self.assertFalse(nemo.sdk_complete(self.root, 'key'))
                path.write_bytes(b'sdk')


if __name__ == '__main__':
    unittest.main()

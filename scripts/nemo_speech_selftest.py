"""SDK cache witnesses without network, builds, models or listening sockets."""
import importlib.util
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

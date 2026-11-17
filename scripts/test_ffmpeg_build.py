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


if __name__ == '__main__':
    unittest.main()

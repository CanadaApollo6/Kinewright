import hashlib
import io
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import patch
import ffmpeg_archive as archive


class Download(unittest.TestCase):
    def test_transient_retry_and_verified_cache(self):
        payload=b'verified payload';digest=hashlib.sha256(payload).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'archive'
            responses=[urllib.error.HTTPError('url',504,'gateway',None,None),TimeoutError(),io.BytesIO(payload)]
            with patch.object(archive.urllib.request,'urlopen',side_effect=responses) as opened, patch.object(archive.time,'sleep') as slept:
                archive.fetch('https://invalid.test',path,digest)
                self.assertEqual(opened.call_count,3)
                self.assertEqual([c.args[0] for c in slept.call_args_list],[2,4])
                archive.fetch('https://invalid.test',path,digest)
                self.assertEqual(opened.call_count,3)
            path.write_bytes(b'corrupt')
            with patch.object(archive.urllib.request,'urlopen') as opened:
                with self.assertRaises(ValueError):archive.fetch('https://invalid.test',path,digest)
                opened.assert_not_called()

    def test_four_tries_only(self):
        with tempfile.TemporaryDirectory() as folder, patch.object(archive.urllib.request,'urlopen',side_effect=TimeoutError()) as opened, patch.object(archive.time,'sleep') as slept:
            with self.assertRaises(TimeoutError):archive.fetch('https://invalid.test',Path(folder)/'archive','0'*64)
            self.assertEqual(opened.call_count,4)
            self.assertEqual([c.args[0] for c in slept.call_args_list],[2,4,8])

    def test_permanent_errors_do_not_retry(self):
        for error in [urllib.error.HTTPError('url',404,'missing',None,None),urllib.error.URLError('DNS failure')]:
            with tempfile.TemporaryDirectory() as folder, patch.object(archive.urllib.request,'urlopen',side_effect=error) as opened, patch.object(archive.time,'sleep') as slept:
                with self.assertRaises(urllib.error.URLError):archive.fetch('https://invalid.test',Path(folder)/'archive','0'*64)
                self.assertEqual(opened.call_count,1);slept.assert_not_called()

    def test_bad_download_is_not_installed(self):
        with tempfile.TemporaryDirectory() as folder, patch.object(archive.urllib.request,'urlopen',return_value=io.BytesIO(b'wrong')), patch.object(archive.time,'sleep') as slept:
            path=Path(folder)/'archive'
            with self.assertRaises(ValueError):archive.fetch('https://invalid.test',path,'0'*64)
            self.assertEqual(list(Path(folder).iterdir()),[]);slept.assert_not_called()


if __name__=='__main__':unittest.main()

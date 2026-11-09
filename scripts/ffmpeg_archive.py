#!/usr/bin/env python3
"""Fetch or reuse a pinned archive. Verify cached bytes before every use."""
import argparse
import hashlib
import re
import socket
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path


def verify(path, expected):
    with path.open('rb') as source:
        actual = hashlib.file_digest(source, 'sha256').hexdigest()
    if actual != expected:
        raise ValueError(f'FFmpeg archive SHA-256 mismatch: expected {expected}, got {actual}')


def fetch(url, archive, expected):
    archive = Path(archive)
    if archive.exists():
        verify(archive, expected)
        return
    archive.parent.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + 150
    for attempt in range(4):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError('FFmpeg download deadline exceeded')
        temporary = None
        try:
            request = urllib.request.Request(url, headers={'User-Agent': 'Kinewright-FFmpeg'})
            with urllib.request.urlopen(request, timeout=min(30, remaining)) as source:
                with tempfile.NamedTemporaryFile(dir=archive.parent, suffix='.part', delete=False) as target:
                    temporary = Path(target.name)
                    while True:
                        if time.monotonic() >= deadline:
                            raise TimeoutError('FFmpeg download deadline exceeded')
                        chunk = source.read1(1024 * 1024)
                        if not chunk:
                            break
                        target.write(chunk)
            verify(temporary, expected)
            temporary.replace(archive)
            return
        except (urllib.error.URLError, TimeoutError) as error:
            retryable = (isinstance(error, urllib.error.HTTPError) and 500 <= error.code < 600) or isinstance(
                error.reason if isinstance(error, urllib.error.URLError) else error,
                (TimeoutError, socket.timeout),
            )
            if not retryable or attempt == 3 or time.monotonic() >= deadline:
                raise
            delay = min(2 ** (attempt + 1), max(0, deadline - time.monotonic()))
            print(f'FFmpeg transient download failure; retry {attempt + 2}/4 in {delay:g}s', file=sys.stderr)
            time.sleep(delay)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache-key', choices=['Windows', 'Linux'])
    parser.add_argument('download', nargs='*', metavar='URL ARCHIVE SHA256')
    args = parser.parse_args()
    if args.cache_key:
        root = Path(__file__).resolve().parent
        script = root / ('setup-ffmpeg.ps1' if args.cache_key == 'Windows' else 'setup-ffmpeg.sh')
        pattern = r"\$FfmpegSha256 = '([0-9a-f]{64})'" if args.cache_key == 'Windows' else r"FFMPEG_SHA256='([0-9a-f]{64})'"
        match = re.search(pattern, script.read_text())
        if match is None:
            parser.error('missing pinned FFmpeg SHA-256')
        print('sha=' + match[1])
    elif len(args.download) == 3:
        fetch(*args.download)
    else:
        parser.error('provide URL ARCHIVE SHA256 or --cache-key')


if __name__ == '__main__':
    main()

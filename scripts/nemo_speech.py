#!/usr/bin/env python3
"""Provision the pinned NeMo shared library. Cargo only links the finished SDK."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time

SCRIPTS = Path(__file__).resolve().parent
WINDOWS = sys.platform == 'win32'


def run(args):
    subprocess.run([str(arg) for arg in args], check=True, stdout=sys.stderr)


def build(args):
    if not WINDOWS:
        args = ['flock', '-o', '/tmp/kinewright-cargo-heavy.lock', 'env',
                'CARGO_BUILD_JOBS=4', 'nice', '-n', '10'] + args
        if os.environ.get('KINEWRIGHT_NEMO_OFFLINE_BUILD') == '1':
            args = args[:8] + ['unshare', '-rn'] + args[8:]
    started = time.monotonic()
    run(args)
    print(f'NeMo build step: {time.monotonic() - started:.2f}s (includes lock wait)', file=sys.stderr)


def build_key():
    compiler = os.environ.get('CXX', 'cl' if WINDOWS else 'c++')
    result = subprocess.run([compiler] + ([] if WINDOWS else ['--version']),
                            capture_output=True, check=False)
    digest = hashlib.sha256()
    digest.update(platform.system().encode())
    digest.update(platform.machine().encode())
    digest.update(result.stdout + result.stderr)
    for name in ('CC', 'CXX', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS'):
        digest.update(name.encode() + b'=' + os.environ.get(name, '').encode())
    for name in ('cmake', 'ninja'):
        digest.update(Path(shutil.which(name)).read_bytes())
    for path in sorted([SCRIPTS / 'nemo-speech-pins.json', SCRIPTS / 'nemo-speech.cmake',
                        SCRIPTS / 'nemo_speech.py', SCRIPTS / 'setup-nemo-speech.sh',
                        SCRIPTS / 'setup-nemo-speech.ps1', *SCRIPTS.glob('nemo-patches/*.patch')]):
        digest.update(path.name.encode() + b'\0' + path.read_bytes())
    # Settings supplied on the command line are fixed below and hashed with this file.
    return digest.hexdigest()


@contextlib.contextmanager
def cache_lock(path):
    with path.open('a+b') as handle:
        if WINDOWS:
            import msvcrt
            if handle.tell() == 0:
                handle.write(b'\0')
                handle.flush()
            handle.seek(0)
            # LK_LOCK has a short timeout; use the blocking Windows file lock API instead.
            import ctypes
            import ctypes.wintypes
            class Overlapped(ctypes.Structure):
                _fields_ = [('Internal', ctypes.c_size_t), ('InternalHigh', ctypes.c_size_t),
                            ('Offset', ctypes.wintypes.DWORD), ('OffsetHigh', ctypes.wintypes.DWORD),
                            ('hEvent', ctypes.wintypes.HANDLE)]
            overlap = Overlapped()
            lock = ctypes.windll.kernel32.LockFileEx
            lock.argtypes = [ctypes.wintypes.HANDLE, ctypes.wintypes.DWORD,
                             ctypes.wintypes.DWORD, ctypes.wintypes.DWORD,
                             ctypes.wintypes.DWORD, ctypes.POINTER(Overlapped)]
            if not lock(msvcrt.get_osfhandle(handle.fileno()), 2, 0, 1, 0, ctypes.byref(overlap)):
                raise OSError('Could not lock NeMo cache')
        else:
            import fcntl
            fcntl.flock(handle, fcntl.LOCK_EX)
        yield


def fetch(path, pin):
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    run(['git', 'init', '--quiet', path])
    run(['git', '-C', path, 'remote', 'add', 'origin', pin['url']])
    run(['git', '-C', path, 'fetch', '--depth=1', 'origin', pin['commit']])
    run(['git', '-C', path, 'checkout', '--quiet', '--detach', 'FETCH_HEAD'])
    actual = subprocess.check_output(['git', '-C', str(path), 'rev-parse', 'HEAD'], text=True).strip()
    if actual != pin['commit']:
        raise RuntimeError(f'Source pin mismatch: {path}: {actual}')


def provision(root, key):
    complete = root / '.build-key'
    lib = root / 'lib' / ('nemo_speech_asr_c.lib' if WINDOWS else 'libnemo_speech_asr_c.so')
    runtime = root / 'bin' / 'nemo_speech_asr_c.dll' if WINDOWS else lib
    if complete.exists() and complete.read_text().strip() == key and lib.is_file() and runtime.is_file():
        return
    complete.unlink(missing_ok=True)
    pins = json.loads((SCRIPTS / 'nemo-speech-pins.json').read_text())
    fetch(root / 'src', pins['nemo'])
    fetch(root / 'src' / 'llama.cpp', pins['llama'])
    fetch(root / 'sentencepiece-src', pins['sentencepiece'])
    for patch in sorted(SCRIPTS.glob('nemo-patches/*.patch')):
        run(['git', '-C', root / 'src', 'apply', '--check', patch])
        run(['git', '-C', root / 'src', 'apply', patch])
    # No fetches below this line. SentencePiece uses its bundled protobuf/abseil.
    for directory in ['sentencepiece-build', 'build', 'lib', 'bin', 'deps']:
        shutil.rmtree(root / directory, ignore_errors=True)
    sp = root / 'deps' / 'sentencepiece'
    common = ['-G', 'Ninja', '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_INSTALL_LIBDIR=lib',
              '-DCMAKE_POSITION_INDEPENDENT_CODE=ON', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL']
    build(['cmake', '-S', root / 'sentencepiece-src', '-B', root / 'sentencepiece-build', *common,
           '-DSPM_ENABLE_SHARED=OFF', '-DSPM_ENABLE_TCMALLOC=OFF', '-DSPM_BUILD_TEST=OFF',
           '-DSPM_ENABLE_MSVC_MT_BUILD=OFF', '-DSPM_ABSL_PROVIDER=internal',
           '-DSPM_PROTOBUF_PROVIDER=internal', f'-DCMAKE_INSTALL_PREFIX={sp}'])
    build(['cmake', '--build', root / 'sentencepiece-build', '--parallel', '4'])
    build(['cmake', '--install', root / 'sentencepiece-build'])
    build(['cmake', '-S', root / 'src', '-B', root / 'build', '-G', 'Ninja',
           '-C', SCRIPTS / 'nemo-speech.cmake', f'-DCMAKE_INSTALL_PREFIX={root}',
           f'-DNEMO_SPEECH_DEPENDENCY_PREFIX={root / "deps"}',
           f'-DCMAKE_PREFIX_PATH={sp}', f'-DCMAKE_INSTALL_RPATH={root / "lib"}'])
    build(['cmake', '--build', root / 'build', '--parallel', '4'])
    build(['cmake', '--install', root / 'build'])
    if not lib.is_file() or not runtime.is_file():
        raise RuntimeError(f'NeMo shared library installation incomplete: {root}')
    partial = root / '.build-key.part'
    partial.write_text(key + '\n')
    partial.replace(complete)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--print-key', action='store_true')
    args = parser.parse_args()
    if platform.machine().lower() not in ('x86_64', 'amd64'):
        raise RuntimeError('NeMo requires an x86-64 AVX2 machine')
    key = build_key()
    if args.print_key:
        print(key)
        return
    cache = os.environ.get('KINEWRIGHT_NEMO_CACHE')
    if not cache:
        base = Path(os.environ.get('LOCALAPPDATA', Path.home() / 'AppData/Local')) if WINDOWS else Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache'))
        cache = base / 'kinewright' / 'nemo-speech'
    cache = Path(cache).resolve()
    cache.mkdir(parents=True, exist_ok=True)
    root = cache / key
    with cache_lock(cache / (key + '.lock')):
        root.mkdir(exist_ok=True)
        provision(root, key)
    print(root)


if __name__ == '__main__':
    main()

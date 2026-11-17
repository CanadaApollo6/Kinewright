#!/usr/bin/env python3
"""Build the MS1 shared libraries, device-only recorder, or isolated test tools."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
PROFILE = json.loads((ROOT / 'scripts/ffmpeg-profile.json').read_text())


def configure(profile):
    args = ['--disable-everything', '--disable-autodetect', '--disable-network',
            '--disable-doc', '--disable-debug', '--enable-x86asm', '--x86asmexe=nasm', '--disable-iconv', '--enable-gpl',
            '--enable-version3', '--enable-avcodec', '--enable-avformat',
            '--enable-avfilter', '--enable-swscale', '--enable-swresample',
            '--enable-protocol=file', '--enable-libx264', '--enable-pic']
    if profile == 'libraries':
        args += ['--disable-programs', '--disable-avdevice', '--enable-shared',
                 '--disable-static', '--enable-libx265', '--enable-libdav1d', '--enable-zlib']
        for kind in ['decoders', 'demuxers', 'parsers', 'encoders', 'muxers', 'bsfs', 'filters']:
            args += [f'--enable-{kind[:-1]}=' + ','.join(PROFILE[kind])]
    elif profile == 'recorder':
        device_decoders = PROFILE['capture_decoders'] + (['bmp'] if platform.system() == 'Windows' else [])
        args += ['--disable-shared', '--enable-static', '--enable-ffmpeg', '--disable-ffprobe',
                 '--disable-ffplay', '--enable-avdevice', '--enable-encoder=libx264,aac,pcm_s16le',
                 '--enable-muxer=mp4,ipod,wav',
                 '--enable-decoder=' + ','.join(device_decoders),
                 '--enable-filter=' + ','.join(PROFILE['capture_filters'])]
        if platform.system() == 'Windows':
            args += ['--enable-indev=dshow,gdigrab']
        else:
            args += ['--enable-indev=v4l2,xcbgrab,alsa,pulse', '--enable-libxcb',
                     '--enable-libxcb-shm', '--enable-libxcb-shape', '--enable-libxcb-xfixes',
                     '--enable-libpulse', '--enable-alsa']
    else:
        # Test generators are static and have their own prefix, never FFMPEG_DIR.
        args += ['--disable-shared', '--enable-static', '--enable-ffmpeg', '--enable-ffprobe',
                 '--disable-ffplay', '--enable-libfreetype', '--enable-libharfbuzz', '--enable-protocol=pipe', '--enable-zlib', '--enable-libdav1d', '--enable-libx265',
                 '--enable-libvpx', '--enable-decoder=' + ','.join(PROFILE['decoders'] + ['qtrle', 'wrapped_avframe', 'subrip']),
                 '--enable-demuxer=' + ','.join(PROFILE['demuxers'] + ['rawvideo', 'concat', 'hls', 'srt']),
                 '--enable-parser=' + ','.join(PROFILE['parsers']),
                 '--enable-encoder=libx264,libx265,libvpx_vp9,ffv1,mpeg2video,mpeg4,mjpeg,png,rawvideo,pcm_s16le,pcm_f32le,aac,qtrle,subrip',
                 '--enable-muxer=mp4,ipod,mov,matroska,webm,avi,nut,mpeg1system,mpeg2video,mpegts,image2,image2pipe,rawvideo,framehash,pcm_f32le,wav',
                 '--enable-bsfs', '--enable-filters', '--enable-indev=lavfi']
    if platform.system() == 'Windows':
        args += ['--toolchain=msvc', '--arch=x86_64', '--target-os=win64',
                 '--pkg-config=' + (ROOT / 'third_party/pkgconf/pkgconf/.bin/pkgconf.exe').as_posix(),
                 '--pkg-config-flags=--static']
    return args


def run(command, cwd, heavy=False):
    if platform.system() != 'Windows':
        command = (['flock', '-o', '/tmp/kinewright-cargo-heavy.lock', 'nice', '-n', '10']
                   if heavy else ['nice', '-n', '10']) + command
    subprocess.run(command, cwd=cwd, check=True)


def ensure_nasm():
    if shutil.which('nasm'):
        return
    if platform.system() == 'Windows':
        raise SystemExit('Install MSYS2 nasm before building FFmpeg.')
    prefix = ROOT / 'third_party/nasm'
    executable = prefix / 'bin/nasm'
    if not executable.exists():
        scratch = ROOT / 'target/nasm-build'
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            work = Path(directory)
            archive = work / 'nasm.tar.xz'
            urllib.request.urlretrieve('https://www.nasm.us/pub/nasm/releasebuilds/2.16.03/nasm-2.16.03.tar.xz', archive)
            with archive.open('rb') as downloaded:
                actual = hashlib.file_digest(downloaded, 'sha256').hexdigest()
            if actual != '1412a1c760bbd05db026b6c0d1657affd6631cd0a63cddb6f73cc6d4aa616148':
                raise SystemExit('NASM source checksum mismatch')
            with tarfile.open(archive) as source:
                source.extractall(work, filter='data')
            source_dir = work / 'nasm-2.16.03'
            run(['bash', 'configure', '--prefix=' + str(prefix)], source_dir)
            run(['make', '-j4'], source_dir, heavy=True)
            run(['make', 'install'], source_dir, heavy=True)
    os.environ['PATH'] = str(executable.parent) + os.pathsep + os.environ['PATH']


def build(profile):
    args = configure(profile)
    key = hashlib.sha256((PROFILE['sha256'] + '\n' + '\n'.join(args)).encode()).hexdigest()
    prefix = ROOT / 'third_party' / {'libraries': 'ffmpeg', 'recorder': 'ffmpeg-recorder',
                                    'testgen': 'ffmpeg-testgen'}[profile]
    marker = prefix / '.build-key'
    if marker.exists() and marker.read_text() == key:
        print(f'{profile}: cached {key}', flush=True)
        return
    ensure_nasm()
    scratch = ROOT / 'target' / 'ffmpeg-build'
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=profile + '-', dir=scratch) as temporary:
        work = Path(temporary)
        archive = work / 'source.tar.xz'
        url = f'https://ffmpeg.org/releases/ffmpeg-{PROFILE["version"]}.tar.xz'
        urllib.request.urlretrieve(url, archive)
        with archive.open('rb') as downloaded:
            actual = hashlib.file_digest(downloaded, 'sha256').hexdigest()
        if actual != PROFILE['sha256']:
            raise SystemExit(f'FFmpeg source checksum mismatch: {actual}')
        # No source file is extracted or executed before verification.
        with tarfile.open(archive) as source:
            source.extractall(work, filter='data')
        source_dir = work / f'ffmpeg-{PROFILE["version"]}'
        if prefix.exists():
            shutil.rmtree(prefix)
        install_prefix = prefix.as_posix()
        if platform.system() == 'Windows':
            install_prefix = '/' + install_prefix[0].lower() + install_prefix[2:]
        args += ['--prefix=' + install_prefix]
        if platform.system() == 'Windows':
            # vcvars64 and static vcpkg libraries must be in the inherited environment.
            os.environ['PKG_CONFIG_PATH'] = str(ROOT / 'third_party/ffmpeg-deps/installed/x64-windows-static/lib/pkgconfig')
            bash = os.environ.get('KINEWRIGHT_MSYS_BASH', r'C:\msys64\usr\bin\bash.exe')
            run([bash, str(source_dir / 'configure'), *args], source_dir)
        else:
            run(['bash', str(source_dir / 'configure'), *args], source_dir)
        run(['make', '-j4', 'install' if profile == 'libraries' else 'install-progs'], source_dir, heavy=True)
        prefix.mkdir(parents=True, exist_ok=True)
        if platform.system() == 'Windows':
            for pc in prefix.glob('lib/pkgconfig/*.pc'):
                pc.write_text(pc.read_text().replace(install_prefix, prefix.as_posix()))
        shutil.copyfile(source_dir / 'COPYING.GPLv3', prefix / 'LICENSE.txt')
        (prefix / 'configure.txt').write_text('\n'.join(args) + '\n')
        # FFmpeg libraries are static in the tools. These programs cannot load the app's libav*.
        if profile == 'testgen':
            for name in ['ffmpeg', 'ffprobe']:
                suffix = '.exe' if platform.system() == 'Windows' else ''
                (prefix / 'bin' / (name + suffix)).rename(prefix / 'bin' / (name + '-testgen' + suffix))
        marker.write_text(key)
    print(f'{profile}: built {key}', flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=['libraries', 'recorder', 'testgen'], default='libraries')
    parser.add_argument('--print-key', action='store_true')
    options = parser.parse_args()
    if options.print_key:
        print(hashlib.sha256((PROFILE['sha256'] + '\n' + '\n'.join(configure(options.profile))).encode()).hexdigest())
    else:
        build(options.profile)

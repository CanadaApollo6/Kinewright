#!/usr/bin/env python3
"""Build the MS1 shared libraries, device-only recorder, or isolated test tools."""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import re
import shutil
import shlex
import subprocess
import tarfile
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
PROFILE = json.loads((ROOT / 'scripts/ffmpeg-profile.json').read_text())


def configure(profile, system=None):
    system = system or platform.system()
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
        device_decoders = PROFILE['capture_decoders'] + (['bmp'] if system == 'Windows' else [])
        args += ['--disable-shared', '--enable-static', '--enable-ffmpeg', '--disable-ffprobe',
                 '--disable-ffplay', '--enable-avdevice', '--enable-encoder=libx264,aac,pcm_s16le',
                 '--enable-muxer=mp4,ipod,wav',
                 '--enable-bsf=' + ','.join(PROFILE['capture_bsfs']),
                 '--enable-decoder=' + ','.join(device_decoders),
                 '--enable-filter=' + ','.join(PROFILE['capture_filters'])]
        if system == 'Windows':
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
                 '--enable-demuxer=' + ','.join(PROFILE['demuxers'] + ['mpegps', 'mpegts', 'rawvideo', 'concat', 'hls', 'srt']),
                 '--enable-parser=' + ','.join(PROFILE['parsers']),
                 '--enable-encoder=libx264,libx265,libvpx_vp9,ffv1,mpeg2video,mpeg4,mjpeg,png,rawvideo,pcm_s16le,pcm_f32le,aac,ac3,qtrle,subrip',
                 '--enable-muxer=mp4,ipod,mov,matroska,webm,avi,nut,mpeg1system,mpeg2video,mpegts,image2,image2pipe,rawvideo,framehash,pcm_f32le,wav',
                 '--enable-bsfs', '--enable-filters', '--enable-indev=lavfi']
        if system == 'Linux':
            # Native sandbox witnesses generate AV1 and MP3 before confinement.
            args += ['--enable-libaom', '--enable-libmp3lame',
                     '--enable-encoder=libaom_av1,libmp3lame', '--enable-muxer=mp3']
    if system == 'Windows':
        args += ['--toolchain=msvc', '--arch=x86_64', '--target-os=win64',
                 '--pkg-config=' + (ROOT / 'third_party/pkgconf/pkgconf/.bin/pkgconf.exe').as_posix(),
                 '--pkg-config-flags=--static']
    tools = tool_commands()
    args += [f'--{name}=' + shlex.join(tools[name]) for name in ['cc', 'cxx', 'ld', 'ar']]
    return args


# Where configure finds each --enable-<kind>= component list (configure's
# find_things_extern/find_filters_extern). Configure only warns when an enable
# matches nothing, and stays silent when one name of several is missing, so a
# component dropped upstream would silently leave a profile.
COMPONENT_SOURCES = {
    'decoder': ('libavcodec/allcodecs.c', 'decoder'),
    'encoder': ('libavcodec/allcodecs.c', 'encoder'),
    'parser': ('libavcodec/parsers.c', 'parser'),
    'bsf': ('libavcodec/bitstream_filters.c', 'bsf'),
    'demuxer': ('libavformat/allformats.c', 'demuxer'),
    'muxer': ('libavformat/allformats.c', 'muxer'),
    'protocol': ('libavformat/protocols.c', 'protocol'),
    'indev': ('libavdevice/alldevices.c', 'demuxer'),
    'outdev': ('libavdevice/alldevices.c', 'muxer'),
}


def source_components(read):
    """Component names per configure kind; `read` returns a source file's text."""
    components = {}
    for kind, (path, suffix) in COMPONENT_SOURCES.items():
        pattern = re.compile(rf'^[^#\n]*extern\b[^\n]*\bff_(\w+)_{suffix};', re.M)
        components[kind] = set(pattern.findall(read(path)))
    filters = read('libavfilter/allfilters.c')
    components['filter'] = set(re.findall(r'^extern const \w+ ff_[avfsinkrc]{2,5}_(\w+);', filters, re.M))
    # The buffer sources and sinks are declared `extern  const`, outside configure's
    # list, and always built. Their enables are inert; count them as present by the
    # names they register (ff_vsink_buffer registers "buffersink").
    for kind, name in re.findall(r'^extern  const \w+ ff_([avfsinkrc]{2,5})_(\w+);', filters, re.M):
        components['filter'].add(name + 'sink' if kind.endswith('sink') else name)
    return components


def missing_components(args, components):
    missing = []
    for argument in args:
        match = re.fullmatch(r'--enable-(\w+)=(.+)', argument)
        if match and match[1] in components:
            missing += [f'{match[1]} {name}' for name in match[2].split(',')
                        if name not in components[match[1]]]
    return missing


def check_components(read):
    """Refuse every profile, on both build systems, that names an absent component."""
    components = source_components(read)
    missing = sorted({f'{profile}: {entry}' for profile in ['libraries', 'recorder', 'testgen']
                      for system in ['Linux', 'Windows']
                      for entry in missing_components(configure(profile, system), components)})
    if missing:
        raise SystemExit(f'FFmpeg {PROFILE["version"]} source lacks profile components: ' + '; '.join(missing))


def archive_reader(archive):
    source = tarfile.open(archive)
    def read(path):
        member = source.extractfile(f'ffmpeg-{PROFILE["version"]}/{path}')
        if member is None:
            raise SystemExit(f'FFmpeg source has no {path}')
        return member.read().decode()
    return source, read


def tool_commands():
    windows = platform.system() == 'Windows'
    defaults = {'cc': 'cl.exe' if windows else 'gcc',
                'cxx': 'cl.exe' if windows else 'g++',
                'ld': 'link.exe' if windows else os.environ.get('CC', 'gcc'),
                'ar': 'lib.exe' if windows else 'ar'}
    result = {name: shlex.split(os.environ.get(name.upper(), default), posix=not windows)
              for name, default in defaults.items()}
    result.update(nasm=['nasm'], pkgconfig=[os.environ.get('PKG_CONFIG', 'pkg-config')],
                  make=[str(Path(os.environ.get('KINEWRIGHT_MSYS_ROOT', 'C:/msys64')) / 'usr/bin/make.exe')] if windows else ['make'],
                  shell=[str(Path(os.environ.get('KINEWRIGHT_MSYS_ROOT', 'C:/msys64')) / 'usr/bin/sh.exe')] if windows else ['bash'])
    return result


def canonical(value):
    if isinstance(value, str):
        return value.replace(str(ROOT), '<repo>').replace(ROOT.as_posix(), '<repo>')
    if isinstance(value, list):
        return [canonical(item) for item in value]
    if isinstance(value, dict):
        return {key: canonical(item) for key, item in value.items()}
    return value


def file_digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def verify_source(archive):
    actual = file_digest(archive)
    if actual != PROFILE['sha256']:
        raise SystemExit(f'FFmpeg source checksum mismatch: {actual}')


def verified_source():
    scratch = ROOT / 'target/ffmpeg-source'
    cache = scratch / PROFILE['sha256'] / 'source.tar.xz'
    if cache.exists():
        verify_source(cache)
        return cache
    scratch.mkdir(parents=True, exist_ok=True)
    url = f'https://ffmpeg.org/releases/ffmpeg-{PROFILE["version"]}.tar.xz'
    for attempt in range(1, 4):
        # Each attempt starts empty; partial responses never enter the cache.
        with tempfile.TemporaryDirectory(prefix='download-', dir=scratch) as temporary:
            archive = Path(temporary) / 'source.tar.xz'
            try:
                with urllib.request.urlopen(url, timeout=30) as response, archive.open('wb') as output:
                    shutil.copyfileobj(response, output)
            except (OSError, urllib.error.URLError, http.client.HTTPException) as error:
                if attempt == 3:
                    raise
                delay = 2 ** attempt
                print(f'FFmpeg source attempt {attempt}/3 failed: {error}; retrying in {delay}s', flush=True)
                time.sleep(delay)
                continue
            verify_source(archive)
            cache.parent.mkdir(parents=True, exist_ok=True)
            archive.replace(cache)
            return cache


def tool_identity(command, version_arguments):
    executable = shutil.which(command[0])
    if executable is None:
        raise SystemExit(f'Required build tool is missing: {command[0]}')
    result = subprocess.run([*command, *version_arguments], stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True,
                            env=dict(os.environ, LC_ALL='C', VSLANG='1033'), check=False)
    # GCC's linker version query prints a fresh LTO resolution scratch path.
    # It is an invocation nonce, not a toolchain identity.
    version = re.sub(r'(?<=-fresolution=)\S+', '<temporary>', result.stdout.strip())
    if not version:
        raise SystemExit(f'Cannot identify build tool: {command[0]}')
    return dict(command=command, executable=str(Path(executable).resolve()),
                sha256=file_digest(executable), version=version)


def fingerprint():
    """One bundle identity, used unchanged by CI cache keys and every local marker."""
    ensure_nasm()
    windows = platform.system() == 'Windows'
    if windows:
        os.environ['PKG_CONFIG_PATH'] = str(ROOT / 'third_party/ffmpeg-deps/installed/x64-windows-static/lib/pkgconfig')
        os.environ['PKG_CONFIG'] = str(ROOT / 'third_party/pkgconf/pkgconf/.bin/pkgconf.exe')
    commands = tool_commands()
    identities = {}
    for name, command in commands.items():
        option = ['/Bv'] if windows and name in ['cc', 'cxx'] else (['/?'] if windows and name in ['ld', 'ar'] else ['--version'])
        identities[name] = tool_identity(command, option)
    if not windows:
        linker = subprocess.check_output([*commands['cc'], '-print-prog-name=ld'], text=True).strip()
        identities['linker'] = tool_identity([linker], ['--version'])
        if any(name in Path(commands['ld'][0]).name for name in ['gcc', 'g++', 'clang', 'cc']):
            identities['linker_driver'] = tool_identity(commands['ld'], [*shlex.split(os.environ.get('LDFLAGS', '')), '-Wl,--version'])
    packages = ['x264', 'x265', 'dav1d', 'zlib', 'vpx', 'freetype2', 'harfbuzz']
    if not windows:
        packages += ['aom', 'lame', 'alsa', 'libpulse', 'xcb', 'xcb-shm', 'xcb-shape', 'xcb-xfixes']
    dependencies = {}
    for package in packages:
        query = subprocess.run([*commands['pkgconfig'], '--modversion', package],
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, check=False)
        if query.returncode != 0:
            dependencies[package] = None
            continue
        dependency = {'version': query.stdout.strip()}
        for option in ['--cflags', '--libs']:
            flags_query = subprocess.run([*commands['pkgconfig'], *(['--static'] if windows else []), option, package],
                                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, check=True)
            dependency[option[2:]] = flags_query.stdout.strip()
        dependencies[package] = dependency
    scripts = ['scripts/build-ffmpeg.py', 'scripts/ffmpeg-profile.json',
               'scripts/setup-ffmpeg.sh', 'scripts/setup-ffmpeg.ps1',
               'scripts/setup-ffmpeg-testgen.sh', 'scripts/prepare-ffmpeg-windows.ps1',
               '.github/actions/ffmpeg/action.yml']
    flags = {name: os.environ.get(name, '') for name in
             ['CFLAGS', 'CPPFLAGS', 'CXXFLAGS', 'LDFLAGS', 'WindowsSdkDir', 'WindowsSDKVersion', 'VCToolsVersion']}
    for name in ['INCLUDE', 'LIB', 'LIBPATH']:
        if windows:
            flags[name] = list(dict.fromkeys(os.environ.get(name, '').split(';')))
    record = canonical(dict(schema=2, system=platform.system(), machine=platform.machine(),
                            tools=identities, dependencies=dependencies, flags=flags,
                            profile=PROFILE, configure={p: configure(p) for p in ['libraries', 'recorder', 'testgen']},
                            scripts={name: file_digest(ROOT/name) for name in scripts}))
    if not windows:
        record['host'] = dict(distribution=platform.freedesktop_os_release(), libc=os.confstr('CS_GNU_LIBC_VERSION'))
    encoded = json.dumps(record, sort_keys=True, separators=(',', ':')).encode()
    return hashlib.sha256(encoded).hexdigest(), record


def log_digests():
    """Observation of trusted build/cache contents, not provenance authentication."""
    prefixes = ['ffmpeg', 'ffmpeg-recorder', 'ffmpeg-testgen', 'ffmpeg-deps/installed', 'pkgconf', 'libclang', 'nasm']
    for prefix in prefixes:
        directory = ROOT/'third_party'/prefix
        if not directory.exists():
            continue
        for path in sorted(directory.rglob('*')):
            if not path.is_file() or path.is_symlink():
                continue
            binary = path.suffix.lower() in ['.exe', '.dll', '.lib', '.a'] or '.so' in path.name
            if not binary:
                with path.open('rb') as source:
                    binary = source.read(4) == b'\x7fELF'
            if binary:
                print(f'SHA256 {file_digest(path)}  {path.relative_to(ROOT).as_posix()}', flush=True)


def depfile_shell_witness():
    """Exercise the upstream MSVC depfile awk expression through the chosen make/sh."""
    scratch = ROOT/'target'
    scratch.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='ffmpeg-shell-', dir=scratch) as temporary:
        work = Path(temporary)
        recipe = r"""all:
	@printf '%s\n' 'Note: including file: C:\sdk\foo.h' | awk '/including/ { sub(/^.*file: */, ""); gsub(/\\/, "/"); print $$0 }'
"""
        (work/'Makefile').write_text(recipe)
        command = [*tool_commands()['make'], '-f', 'Makefile']
        if platform.system() == 'Windows':
            command += ['SHELL=/usr/bin/sh']
        result = subprocess.check_output(command, cwd=work, text=True).strip()
        if result != 'C:/sdk/foo.h':
            raise SystemExit(f'MSYS depfile escaping failed: {result!r}')
    print('MSVC depfile make/sh/awk round trip passed', flush=True)


def run(command, cwd, heavy=False):
    if platform.system() == 'Windows' and command[0] == 'make':
        command = [*tool_commands()['make'], 'SHELL=/usr/bin/sh', *command[1:]]
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


def install_msvc_import_libraries(prefix):
    # Upstream installs these beside the DLLs, but pkg-config and Cargo use lib/.
    destination = prefix / 'lib'
    destination.mkdir(parents=True, exist_ok=True)
    for name in ['avcodec', 'avformat', 'avutil', 'avfilter', 'swscale', 'swresample']:
        (prefix / 'bin' / f'{name}.lib').replace(destination / f'{name}.lib')


def build(profile, no_binary_cache=False):
    args = configure(profile)
    key, identity = fingerprint()
    prefix = ROOT / 'third_party' / {'libraries': 'ffmpeg', 'recorder': 'ffmpeg-recorder',
                                    'testgen': 'ffmpeg-testgen'}[profile]
    marker = prefix / '.build-key'
    if not no_binary_cache and marker.exists() and marker.read_text() == key:
        print(f'{profile}: local/cache reuse {key}', flush=True)
        return
    ensure_nasm()
    scratch = ROOT / 'target' / 'ffmpeg-build'
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=profile + '-', dir=scratch) as temporary:
        work = Path(temporary)
        archive = work / 'source.tar.xz'
        shutil.copyfile(verified_source(), archive)
        verify_source(archive)
        # No source file is extracted or executed before verification.
        with tarfile.open(archive) as source:
            source.extractall(work, filter='data')
        source_dir = work / f'ffmpeg-{PROFILE["version"]}'
        check_components(lambda path: (source_dir / path).read_text())
        if prefix.exists():
            shutil.rmtree(prefix)
        install_prefix = prefix.as_posix()
        if platform.system() == 'Windows':
            install_prefix = '/' + install_prefix[0].lower() + install_prefix[2:]
        args += ['--prefix=' + install_prefix]
        if platform.system() == 'Windows':
            # vcvars64 and static vcpkg libraries must be in the inherited environment.
            os.environ['PKG_CONFIG_PATH'] = str(ROOT / 'third_party/ffmpeg-deps/installed/x64-windows-static/lib/pkgconfig')
            bash = str(Path(os.environ.get('KINEWRIGHT_MSYS_ROOT', 'C:/msys64')) / 'usr/bin/bash.exe')
            run([bash, str(source_dir / 'configure'), *args], source_dir)
        else:
            run(['bash', str(source_dir / 'configure'), *args], source_dir)
        run(['make', '-j4', 'install' if profile == 'libraries' else 'install-progs'], source_dir, heavy=True)
        prefix.mkdir(parents=True, exist_ok=True)
        if platform.system() == 'Windows':
            if profile == 'libraries':
                install_msvc_import_libraries(prefix)
            for pc in prefix.glob('lib/pkgconfig/*.pc'):
                pc.write_text(pc.read_text().replace(install_prefix, prefix.as_posix()))
        shutil.copyfile(source_dir / 'COPYING.GPLv3', prefix / 'LICENSE.txt')
        (prefix / 'configure.txt').write_text('\n'.join(args) + '\n')
        # FFmpeg libraries are static in the tools. These programs cannot load the app's libav*.
        if profile == 'testgen':
            for name in ['ffmpeg', 'ffprobe']:
                suffix = '.exe' if platform.system() == 'Windows' else ''
                (prefix / 'bin' / (name + suffix)).rename(prefix / 'bin' / (name + '-testgen' + suffix))
        (prefix / 'fingerprint.json').write_text(json.dumps(identity, sort_keys=True, indent=2) + '\n')
        marker.write_text(key)
    print(f'{profile}: built {key}', flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=['libraries', 'recorder', 'testgen'], default='libraries')
    parser.add_argument('--print-key', action='store_true')
    parser.add_argument('--no-binary-cache', action='store_true', help='Always compile verified source, ignoring the local marker')
    parser.add_argument('--write-key', action='store_true', help='Write the shared fingerprint to GitHub env/output files')
    parser.add_argument('--log-digests', action='store_true')
    parser.add_argument('--check-msvc-shell', action='store_true')
    parser.add_argument('--fetch-source', action='store_true', help='Fetch or verify the pinned source cache without building')
    parser.add_argument('--check-components', action='store_true',
                        help='Refuse profiles that name a component the pinned source lacks')
    options = parser.parse_args()
    if options.fetch_source:
        print(f'FFmpeg source verified {PROFILE["sha256"]}: {verified_source()}', flush=True)
    elif options.check_components:
        archive, read = archive_reader(verified_source())
        with archive:
            check_components(read)
        print(f'Every profile component exists in FFmpeg {PROFILE["version"]}', flush=True)
    elif options.log_digests:
        log_digests()
    elif options.check_msvc_shell:
        depfile_shell_witness()
    elif options.print_key or options.write_key:
        key, identity = fingerprint()
        print(key)
        if options.write_key:
            print(json.dumps(identity, sort_keys=True, indent=2))
            for variable, name in [('GITHUB_ENV', 'FFMPEG_BUILD_KEY'), ('GITHUB_OUTPUT', 'key')]:
                if output := os.environ.get(variable):
                    with Path(output).open('a', encoding='utf-8') as stream:
                        stream.write(f'{name}={key}\n')
    else:
        build(options.profile, options.no_binary_cache)

#!/usr/bin/env python3
"""Report the registered library attack surface and verify it against the profile."""
import ctypes as C
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shlex
import struct
import sys

SCRIPTS_DIR = Path(__file__).resolve().parent
DEMUXER_ALIASES = {
    'mov': 'mov,mp4,m4a,3gp,3g2,mj2',
    'matroska': 'matroska,webm',
    'image_png_pipe': 'png_pipe',
    'image_jpeg_pipe': 'jpeg_pipe',
}
PARSER_IDS = {
    'mjpeg': ('mjpeg', 'jpegls'),
    'mpeg4video': ('mpeg4',),
    'mpegaudio': ('mp1', 'mp2', 'mp3', 'mp3adu'),
    'mpegvideo': ('mpeg1video', 'mpeg2video'),
}
NON_EXTERNAL_ENABLE_FLAGS = {
    'x86asm', 'gpl', 'version3', 'pic', 'shared', 'static', 'programs',
    'avcodec', 'avformat', 'avfilter', 'avutil', 'avdevice', 'swscale', 'swresample',
    'ffmpeg', 'ffprobe', 'ffplay', 'bsfs', 'filters',
}
FFMPEG_LIBRARY_STEMS = {
    'avcodec', 'avformat', 'avfilter', 'avutil', 'swscale', 'swresample',
}
LINUX_RUNTIME_SONAME_RE = re.compile(
    r'^(ld-linux[^.]*\.so|lib(c|m|mvec|pthread|dl|rt|gcc_s|atomic)\.so)'
)
WINDOWS_RUNTIME_DLL_RE = re.compile(
    r'^(kernel32|bcrypt|user32|ole32|advapi32|ntdll|msvcrt|ucrtbase|'
    r'vcruntime140(_1)?|msvcp140(_[0-9]+)?|api-ms-win-(crt|core)-.+|ext-ms-win-.+)\.dll$'
)
PKGCONFIG_RUNTIME_LIBS = {
    'm', 'c', 'atomic', 'pthread', 'dl', 'rt', 'gcc_s', 'stdc++',
    'bcrypt', 'user32', 'ole32', 'kernel32', 'advapi32', 'shell32', 'psapi',
    'ntdll', 'msvcrt', 'ucrt', 'vcruntime', 'libcmt', 'libucrt', 'libvcruntime', 'oldnames',
}
EXTERNAL_LIB_ALIASES = {
    'z': 'zlib',
    'libz': 'zlib',
    'zlib': 'zlib',
    'zlibstatic': 'zlib',
    'x264': 'libx264',
    'libx264': 'libx264',
    'x265': 'libx265',
    'libx265': 'libx265',
    'x265-static': 'libx265',
    'libx265-static': 'libx265',
    'dav1d': 'libdav1d',
    'libdav1d': 'libdav1d',
}


class Named(C.Structure):
    _fields_ = [('name', C.c_char_p)]


class Parser(C.Structure):
    _fields_ = [('ids', C.c_int * 7)]


class CodecHWConfig(C.Structure):
    _fields_ = [('pix_fmt', C.c_int), ('methods', C.c_int), ('device_type', C.c_int)]


def _load_builder(build_script=None):
    path = Path(build_script) if build_script is not None else SCRIPTS_DIR / 'build-ffmpeg.py'
    spec = importlib.util.spec_from_file_location('ffmpeg_builder_inspect', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _c_string(blob, offset):
    end = blob.find(b'\x00', offset)
    if end < 0:
        raise ValueError(f'unterminated string at offset {offset}')
    return blob[offset:end].decode('utf-8', errors='replace')


def elf_needed(path):
    """Return sorted DT_NEEDED SONAME entries from an ELF64 shared library."""
    data = Path(path).read_bytes()
    if len(data) < 64 or data[:4] != b'\x7fELF':
        raise ValueError(f'{path}: not an ELF file')
    if data[4] != 2:
        raise ValueError(f'{path}: expected ELF64 (class 2), got {data[4]}')
    if data[5] == 1:
        endian = '<'
    elif data[5] == 2:
        endian = '>'
    else:
        raise ValueError(f'{path}: invalid ELF endianness {data[5]}')
    e_shoff = struct.unpack_from(f'{endian}Q', data, 40)[0]
    e_shentsize, e_shnum = struct.unpack_from(f'{endian}HH', data, 58)
    if e_shoff == 0 or e_shnum == 0 or e_shentsize < 64:
        raise ValueError(f'{path}: missing ELF section headers')
    sections = []
    for index in range(e_shnum):
        offset = e_shoff + index * e_shentsize
        sections.append(struct.unpack_from(f'{endian}IIQQQQIIQQ', data, offset))
    needed = []
    for section in sections:
        sh_type, sh_offset, sh_size, sh_link, sh_entsize = (
            section[1], section[4], section[5], section[6], section[9]
        )
        if sh_type != 6:  # SHT_DYNAMIC
            continue
        if sh_link >= len(sections):
            raise ValueError(f'{path}: invalid dynamic strtab index {sh_link}')
        str_off, str_size = sections[sh_link][4], sections[sh_link][5]
        strtab = data[str_off:str_off + str_size]
        step = sh_entsize or 16
        for pos in range(sh_offset, sh_offset + sh_size, step):
            d_tag, d_val = struct.unpack_from(f'{endian}qQ', data, pos)
            if d_tag == 0:  # DT_NULL
                break
            if d_tag == 1:  # DT_NEEDED
                needed.append(_c_string(strtab, d_val))
    return sorted(set(needed))


def pe_imports(path):
    """Return sorted imported DLL names from a PE32 or PE32+ binary."""
    data = Path(path).read_bytes()
    if len(data) < 0x40 or data[:2] != b'MZ':
        raise ValueError(f'{path}: not a PE file')
    pe_offset = struct.unpack_from('<I', data, 0x3C)[0]
    if data[pe_offset:pe_offset + 4] != b'PE\x00\x00':
        raise ValueError(f'{path}: missing PE signature')
    _, num_sections, _, _, _, opt_size, _ = struct.unpack_from('<HHIIIHH', data, pe_offset + 4)
    opt_offset = pe_offset + 24
    magic = struct.unpack_from('<H', data, opt_offset)[0]
    if magic == 0x10B:  # PE32
        dd_offset = opt_offset + 96
    elif magic == 0x20B:  # PE32+
        dd_offset = opt_offset + 112
    else:
        raise ValueError(f'{path}: unsupported PE optional header magic {magic:#x}')
    num_dd = struct.unpack_from('<I', data, dd_offset - 4)[0]
    sec_offset = opt_offset + opt_size
    sections = []
    for index in range(num_sections):
        entry = sec_offset + index * 40
        vsize, vaddr, rsize, rptr = struct.unpack_from('<IIII', data, entry + 8)
        sections.append((vaddr, max(vsize, rsize), rptr))

    def rva_to_offset(rva):
        for vaddr, span, rptr in sections:
            if vaddr <= rva < vaddr + span:
                return rptr + (rva - vaddr)
        raise ValueError(f'{path}: unmapped RVA {rva:#x}')

    imported = set()
    if num_dd > 1:
        imp_rva, _ = struct.unpack_from('<II', data, dd_offset + 1 * 8)
        if imp_rva:
            pos = rva_to_offset(imp_rva)
            while True:
                fields = struct.unpack_from('<IIIII', data, pos)
                if not any(fields):
                    break
                if fields[3]:
                    imported.add(_c_string(data, rva_to_offset(fields[3])))
                pos += 20
    if num_dd > 13:
        delay_rva, _ = struct.unpack_from('<II', data, dd_offset + 13 * 8)
        if delay_rva:
            pos = rva_to_offset(delay_rva)
            while True:
                attrs, name_rva = struct.unpack_from('<II', data, pos)
                if name_rva == 0:
                    break
                if attrs & 1:
                    imported.add(_c_string(data, rva_to_offset(name_rva)))
                pos += 32
    return sorted(imported)


def _normalize_external_token(raw):
    key = raw.lower()
    if key in EXTERNAL_LIB_ALIASES:
        return EXTERNAL_LIB_ALIASES[key]
    return raw


def _ffmpeg_stem_from_filename(name):
    lower = name.lower()
    if lower.endswith('.dll'):
        stem = lower[:-4].split('-', 1)[0]
    elif '.so' in lower:
        stem = lower.split('.so', 1)[0]
        if stem.startswith('lib'):
            stem = stem[3:]
    else:
        return None
    return stem


def _external_from_elf_soname(soname):
    if LINUX_RUNTIME_SONAME_RE.match(soname):
        return None
    stem = _ffmpeg_stem_from_filename(soname)
    if stem in FFMPEG_LIBRARY_STEMS:
        return None
    base = soname.split('.so', 1)[0]
    return _normalize_external_token(base)


def _external_from_pe_dll(dll_name):
    lower = dll_name.lower()
    if WINDOWS_RUNTIME_DLL_RE.match(lower):
        return None
    stem = _ffmpeg_stem_from_filename(lower)
    if stem in FFMPEG_LIBRARY_STEMS:
        return None
    base = lower[:-4] if lower.endswith('.dll') else lower
    return _normalize_external_token(base)


def binary_external_libraries(library_paths):
    """Enumerate linked non-OS, non-FFmpeg external libraries from ELF or PE binaries."""
    found = set()
    for path in library_paths:
        path = Path(path)
        if path.suffix.lower() == '.dll':
            for dll in pe_imports(path):
                name = _external_from_pe_dll(dll)
                if name is not None:
                    found.add(name)
        elif '.so.' in path.name or path.name.endswith('.so'):
            for soname in elf_needed(path):
                name = _external_from_elf_soname(soname)
                if name is not None:
                    found.add(name)
    return sorted(found)


def pkgconfig_external_libraries(root):
    """Enumerate external libraries declared in installed lib/pkgconfig/*.pc files."""
    pc_dir = Path(root) / 'lib/pkgconfig'
    pc_files = sorted(pc_dir.glob('*.pc'))
    if not pc_files:
        raise ValueError(f'no pkg-config files found under {pc_dir}')
    found = set()
    for pc_file in pc_files:
        for line in pc_file.read_text(encoding='utf-8').splitlines():
            key, sep, value = line.partition(':')
            if not sep or key.strip() not in {'Libs', 'Libs.private'}:
                continue
            tokens = [t.strip('"\'') for t in shlex.split(value, posix=False)]
            skip_next = False
            for token in tokens:
                if skip_next:
                    skip_next = False
                    continue
                if not token:
                    continue
                if token in {'-L', '-libpath', '/LIBPATH', '-framework', '-Xlinker'}:
                    skip_next = True
                    continue
                lower = token.lower()
                if token.startswith('-L') or lower.startswith(('-wl,', '-libpath:', '/libpath:', '-pthread', '-fopenmp', '-rpath')):
                    continue
                elif token.startswith('-l:') and len(token) > 3:
                    lib = token[3:]
                elif token.startswith('-l') and len(token) > 2:
                    lib = token[2:]
                elif token.startswith('-'):
                    continue
                elif token.startswith('/') and '/' not in token[1:] and not re.search(r'\.(lib|a|dll|so(\.\d+)*)$', lower):
                    continue
                else:
                    lib = token.replace('\\', '/').rsplit('/', 1)[-1]
                lib_lower = lib.lower()
                if '.so.' in lib_lower:
                    lib = lib[:lib_lower.index('.so.')]
                    lib_lower = lib.lower()
                else:
                    for suffix in ('.lib', '.a', '.dll', '.so'):
                        if lib_lower.endswith(suffix):
                            lib = lib[:-len(suffix)]
                            lib_lower = lib.lower()
                            break
                if lib_lower in PKGCONFIG_RUNTIME_LIBS:
                    continue
                stem = lib_lower[3:] if lib_lower.startswith('lib') else lib_lower
                if stem in FFMPEG_LIBRARY_STEMS:
                    continue
                found.add(_normalize_external_token(lib))
    return sorted(found)


def configured_external_libraries(config_args):
    """Extract enabled external libraries from FFmpeg configure arguments."""
    found = set()
    for arg in config_args:
        if not arg.startswith('--enable-') or '=' in arg:
            continue
        name = arg.removeprefix('--enable-')
        if name in NON_EXTERNAL_ENABLE_FLAGS:
            continue
        found.add(name)
    return sorted(found)


def _parse_config_string(text):
    return shlex.split(text)


def _enabled_list(config_args, kind):
    names = set()
    prefix = f'--enable-{kind}='
    for arg in config_args:
        if arg.startswith(prefix):
            names.update(n for n in arg.removeprefix(prefix).split(',') if n)
    return names


def expected_surface(profile_path=None, build_script=None, system=None):
    """Derive exact expected component sets from build-ffmpeg.py and ffmpeg-profile.json."""
    system = system or platform.system()
    builder = _load_builder(build_script)
    if profile_path is not None:
        profile = json.loads(Path(profile_path).read_text(encoding='utf-8'))
        original_profile = builder.PROFILE
        builder.PROFILE = profile
        try:
            args = builder.configure('libraries', system)
        finally:
            builder.PROFILE = original_profile
    else:
        args = builder.configure('libraries', system)

    decoders = _enabled_list(args, 'decoder') | {'h263'}
    demuxers = {DEMUXER_ALIASES.get(n, n) for n in _enabled_list(args, 'demuxer')}
    # 9.0.2's h263_decoder no longer selects the H.263 parser; MOV's AC3 parser remains.
    parsers = {tuple(sorted(PARSER_IDS.get(n, (n,)))) for n in _enabled_list(args, 'parser')} | {('ac3', 'eac3')}
    filters = _enabled_list(args, 'filter') | {'abuffer', 'abuffersink'}
    bsfs = _enabled_list(args, 'bsf') | {'pgs_frame_merge', 'vp9_superframe'}
    muxers = _enabled_list(args, 'muxer')
    encoders = _enabled_list(args, 'encoder')
    protocols = _enabled_list(args, 'protocol')
    hwaccels = _enabled_list(args, 'hwaccel')
    hwdevices = set()
    external_libraries = set(configured_external_libraries(args))
    built_libraries = {'avutil'} | {
        arg.removeprefix('--enable-') for arg in args
        if arg.startswith('--enable-') and arg.removeprefix('--enable-') in FFMPEG_LIBRARY_STEMS
    }
    return {
        'decoders': decoders,
        'encoders': encoders,
        'demuxers': demuxers,
        'parsers': parsers,
        'muxers': muxers,
        'filters': filters,
        'bsfs': bsfs,
        'hwdevices': hwdevices,
        'hwaccels': hwaccels,
        'protocols': {'input': set(protocols), 'output': set(protocols)},
        'external_libraries': external_libraries,
        'linked_external_libraries': set() if system == 'Windows' else set(external_libraries),
        'built_libraries': built_libraries,
    }


def _names(lib, symbol, predicate=None):
    iterator = getattr(lib, symbol)
    iterator.argtypes = [C.POINTER(C.c_void_p)]
    iterator.restype = C.POINTER(Named)
    opaque = C.c_void_p()
    result = []
    while entry := iterator(C.byref(opaque)):
        if predicate is None or predicate(entry):
            result.append(entry.contents.name.decode())
    return sorted(result)


def _enum_protocols(fmt, output):
    fmt.avio_enum_protocols.argtypes = [C.POINTER(C.c_void_p), C.c_int]
    fmt.avio_enum_protocols.restype = C.c_char_p
    opaque = C.c_void_p()
    result = []
    while name := fmt.avio_enum_protocols(C.byref(opaque), output):
        result.append(name.decode())
    return sorted(result)


def _enum_hwaccels(codec, util):
    codec.av_codec_iterate.argtypes = [C.POINTER(C.c_void_p)]
    codec.av_codec_iterate.restype = C.POINTER(Named)
    codec.avcodec_get_hw_config.argtypes = [C.POINTER(Named), C.c_int]
    codec.avcodec_get_hw_config.restype = C.POINTER(CodecHWConfig)
    util.av_hwdevice_get_type_name.argtypes = [C.c_int]
    util.av_hwdevice_get_type_name.restype = C.c_char_p
    util.av_get_pix_fmt_name.argtypes = [C.c_int]
    util.av_get_pix_fmt_name.restype = C.c_char_p
    opaque = C.c_void_p()
    found = set()
    while entry := codec.av_codec_iterate(C.byref(opaque)):
        codec_name = entry.contents.name.decode()
        index = 0
        while cfg := codec.avcodec_get_hw_config(entry, index):
            device = util.av_hwdevice_get_type_name(cfg.contents.device_type)
            pix_fmt = util.av_get_pix_fmt_name(cfg.contents.pix_fmt)
            device_name = device.decode() if device else 'none'
            pix_name = pix_fmt.decode() if pix_fmt else str(cfg.contents.pix_fmt)
            found.add(f'{codec_name}:{device_name}:{pix_name}')
            index += 1
    return sorted(found)


def _configuration(lib, symbol):
    fn = getattr(lib, symbol)
    fn.argtypes = []
    fn.restype = C.c_char_p
    return fn().decode()


def inspect(root):
    """Inspect a built FFmpeg installation directory and return its surface report."""
    root = Path(root).resolve()
    dll_search = None
    try:
        if os.name == 'nt':
            dll_search = os.add_dll_directory(str(root / 'bin'))
            codec = C.CDLL(str(root / 'bin/avcodec-63.dll'))
            fmt = C.CDLL(str(root / 'bin/avformat-63.dll'))
            filters = C.CDLL(str(root / 'bin/avfilter-12.dll'))
            util = C.CDLL(str(root / 'bin/avutil-61.dll'))
        else:
            codec = C.CDLL(str(root / 'lib/libavcodec.so'))
            fmt = C.CDLL(str(root / 'lib/libavformat.so'))
            filters = C.CDLL(str(root / 'lib/libavfilter.so'))
            util = C.CDLL(str(root / 'lib/libavutil.so'))

        codec.av_codec_is_decoder.argtypes = [C.POINTER(Named)]
        codec.av_codec_is_encoder.argtypes = [C.POINTER(Named)]
        parser = codec.av_parser_iterate
        parser.argtypes = [C.POINTER(C.c_void_p)]
        parser.restype = C.POINTER(Parser)
        codec.avcodec_get_name.argtypes = [C.c_int]
        codec.avcodec_get_name.restype = C.c_char_p
        opaque = C.c_void_p()
        parsers = []
        while entry := parser(C.byref(opaque)):
            parsers.append([codec.avcodec_get_name(i).decode() for i in entry.contents.ids if i])
        util.av_hwdevice_iterate_types.argtypes = [C.c_int]
        util.av_hwdevice_iterate_types.restype = C.c_int
        util.av_hwdevice_get_type_name.argtypes = [C.c_int]
        util.av_hwdevice_get_type_name.restype = C.c_char_p
        hardware = []
        kind = 0
        while kind := util.av_hwdevice_iterate_types(kind):
            hardware.append(util.av_hwdevice_get_type_name(kind).decode())
        hardware.sort()

        hwaccels = _enum_hwaccels(codec, util)
        protocols = {
            'input': _enum_protocols(fmt, 0),
            'output': _enum_protocols(fmt, 1),
        }
        configs = {
            'avcodec': _configuration(codec, 'avcodec_configuration'),
            'avformat': _configuration(fmt, 'avformat_configuration'),
            'avfilter': _configuration(filters, 'avfilter_configuration'),
            'avutil': _configuration(util, 'avutil_configuration'),
        }
        configured_libs = sorted({
            lib
            for text in configs.values()
            for lib in configured_external_libraries(_parse_config_string(text))
        })
        paths = sorted(
            p for p in root.rglob('*')
            if p.is_file() and not p.is_symlink() and ('.so.' in p.name or p.suffix == '.dll')
        )
        linked_libs = binary_external_libraries(paths)
        pc_libs = pkgconfig_external_libraries(root)
        external_libs = sorted(set(linked_libs) | set(pc_libs) | set(configured_libs))

        report = dict(
            hwdevices=hardware,
            hwaccels=hwaccels,
            protocols=protocols,
            external_libraries=external_libs,
            external_library_sources={
                'linked': linked_libs,
                'pkgconfig': pc_libs,
                'configured': configured_libs,
            },
            decoders=_names(codec, 'av_codec_iterate', codec.av_codec_is_decoder),
            encoders=_names(codec, 'av_codec_iterate', codec.av_codec_is_encoder),
            demuxers=_names(fmt, 'av_demuxer_iterate'),
            parsers=parsers,
            muxers=_names(fmt, 'av_muxer_iterate'),
            filters=_names(filters, 'av_filter_iterate'),
            bsfs=_names(codec, 'av_bsf_iterate'),
            libraries={p.name: p.stat().st_size for p in paths},
        )
        report['counts'] = {
            k: len(report[k])
            for k in ['decoders', 'demuxers', 'parsers', 'hwdevices', 'hwaccels', 'external_libraries']
        }
        report['counts']['protocols_input'] = len(protocols['input'])
        report['counts']['protocols_output'] = len(protocols['output'])
        report['library_bytes'] = sum(report['libraries'].values())
        return report
    finally:
        if dll_search is not None:
            dll_search.close()


def _format_entry(item):
    if isinstance(item, tuple):
        return ','.join(item)
    return str(item)


def compare_exact_set(category, actual, expected):
    """Return a named mismatch description when actual != expected as sets, else None."""
    actual_set = set(actual)
    expected_set = set(expected)
    unexpected = sorted(actual_set - expected_set)
    missing = sorted(expected_set - actual_set)
    if not unexpected and not missing:
        return None
    parts = []
    if unexpected:
        parts.append('unexpected=' + ', '.join(_format_entry(item) for item in unexpected))
    if missing:
        parts.append('missing=' + ', '.join(_format_entry(item) for item in missing))
    return f'{category}: ' + '; '.join(parts)


def check(report, expected=None):
    """Compare every reported surface category against exact expected sets."""
    if expected is None:
        expected = expected_surface()
    errors = []
    comparisons = [
        ('decoders', report['decoders'], expected['decoders']),
        ('encoders', report['encoders'], expected['encoders']),
        ('demuxers', report['demuxers'], expected['demuxers']),
        ('parsers', {tuple(sorted(p)) for p in report['parsers']}, expected['parsers']),
        ('muxers', report['muxers'], expected['muxers']),
        ('filters', report['filters'], expected['filters']),
        ('bsfs', report['bsfs'], expected['bsfs']),
        ('hwdevices', report['hwdevices'], expected['hwdevices']),
        ('hwaccels', report['hwaccels'], expected['hwaccels']),
        ('protocols.input', report['protocols']['input'], expected['protocols']['input']),
        ('protocols.output', report['protocols']['output'], expected['protocols']['output']),
        ('external_libraries', report['external_libraries'], expected['external_libraries']),
        ('external_libraries.linked', report['external_library_sources']['linked'],
         expected['linked_external_libraries']),
        ('external_libraries.pkgconfig', report['external_library_sources']['pkgconfig'],
         expected['external_libraries']),
        ('external_libraries.configured', report['external_library_sources']['configured'],
         expected['external_libraries']),
        ('libraries', [_ffmpeg_stem_from_filename(name) or name for name in report['libraries']],
         expected['built_libraries']),
    ]
    for category, actual, want in comparisons:
        mismatch = compare_exact_set(category, actual, want)
        if mismatch is not None:
            errors.append(mismatch)
    if errors:
        raise SystemExit('FFmpeg surface mismatch:\n  ' + '\n  '.join(errors))


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if not argv:
        raise SystemExit('usage: inspect-ffmpeg.py <ffmpeg-root> [--check]')
    report = inspect(argv[0])
    print(json.dumps(report, indent=2))
    if '--check' in argv[1:]:
        check(report)


if __name__ == '__main__':
    main()

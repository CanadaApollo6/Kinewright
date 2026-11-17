#!/usr/bin/env python3
"""Report the registered library attack surface and physical library bytes."""
import ctypes as C
import json
import os
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve()
if os.name == 'nt':
    dll_search = os.add_dll_directory(str(root / 'bin'))
    codec = C.CDLL(str(root / 'bin/avcodec-62.dll'))
    fmt = C.CDLL(str(root / 'bin/avformat-62.dll'))
    filters = C.CDLL(str(root / 'bin/avfilter-11.dll'))
    util = C.CDLL(str(root / 'bin/avutil-60.dll'))
else:
    codec = C.CDLL(str(root / 'lib/libavcodec.so'))
    fmt = C.CDLL(str(root / 'lib/libavformat.so'))
    filters = C.CDLL(str(root / 'lib/libavfilter.so'))
    util = C.CDLL(str(root / 'lib/libavutil.so'))

class Named(C.Structure):
    _fields_ = [('name', C.c_char_p)]

class Parser(C.Structure):
    _fields_ = [('ids', C.c_int * 7)]


def names(lib, symbol, predicate=None):
    iterator = getattr(lib, symbol)
    iterator.argtypes = [C.POINTER(C.c_void_p)]
    iterator.restype = C.POINTER(Named)
    opaque = C.c_void_p()
    result = []
    while entry := iterator(C.byref(opaque)):
        if predicate is None or predicate(entry):
            result.append(entry.contents.name.decode())
    return sorted(result)

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
paths = sorted(p for p in root.rglob('*') if p.is_file() and not p.is_symlink()
               and ('.so.' in p.name or p.suffix == '.dll'))
report = dict(hwdevices=hardware, decoders=names(codec, 'av_codec_iterate', codec.av_codec_is_decoder),
              encoders=names(codec, 'av_codec_iterate', codec.av_codec_is_encoder),
              demuxers=names(fmt, 'av_demuxer_iterate'), parsers=parsers,
              muxers=names(fmt, 'av_muxer_iterate'),
              filters=names(filters, 'av_filter_iterate'),
              bsfs=names(codec, 'av_bsf_iterate'),
              libraries={p.name: p.stat().st_size for p in paths})
report['counts'] = {k: len(report[k]) for k in ['decoders', 'demuxers', 'parsers']}
report['library_bytes'] = sum(report['libraries'].values())
print(json.dumps(report, indent=2))
if '--check' in sys.argv:
    profile = json.loads((Path(__file__).parent / 'ffmpeg-profile.json').read_text())
    assert set(report['decoders']) == set(profile['decoders']) | {'h263'}, report['decoders']
    demuxer_names = {'mov': 'mov,mp4,m4a,3gp,3g2,mj2', 'matroska': 'matroska,webm',
                     'image_png_pipe': 'png_pipe', 'image_jpeg_pipe': 'jpeg_pipe'}
    assert set(report['demuxers']) == {demuxer_names.get(n, n) for n in profile['demuxers']}, report['demuxers']
    parser_ids = {'mjpeg': ('mjpeg', 'jpegls'), 'mpeg4video': ('mpeg4',),
                  'mpegaudio': ('mp1', 'mp2', 'mp3', 'mp3adu'),
                  'mpegvideo': ('mpeg1video', 'mpeg2video')}
    expected_parsers = {tuple(sorted(parser_ids.get(n, (n,)))) for n in profile['parsers']}
    expected_parsers |= {('ac3', 'eac3'), ('h263',)}
    assert {tuple(sorted(p)) for p in report['parsers']} == expected_parsers, report['parsers']
    assert set(report['filters']) == set(profile['filters']) | {'abuffer', 'abuffersink'}, report['filters']
    assert set(report['bsfs']) == set(profile['bsfs']) | {'pgs_frame_merge', 'vp9_superframe'}, report['bsfs']
    assert set(report['muxers']) == set(profile['muxers']), report['muxers']
    assert set(report['encoders']) == set(profile['encoders']), report['encoders']
    assert not hardware, hardware

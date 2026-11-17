#!/usr/bin/env python3
"""Verify the shipped tool exposes device inputs and the approved capture codecs only."""
from pathlib import Path
import json
import os
import re
import subprocess
import tempfile
import wave

root = Path(__file__).resolve().parent.parent
exe = root / 'third_party/ffmpeg-recorder/bin' / ('ffmpeg.exe' if os.name == 'nt' else 'ffmpeg')
def listing(flag):
    return subprocess.check_output([str(exe), '-hide_banner', flag], text=True)

def codecs(flag):
    return set(re.findall(r'^\s*[VAS][A-Z.]{5}\s+(\w+)\s', listing(flag), re.M))

decoders = codecs('-decoders')
expected_decoders = {'rawvideo', 'mjpeg', 'pcm_s16le', 'pcm_s32le', 'pcm_f32le', 'pcm_u8'}
if os.name == 'nt':
    expected_decoders.add('bmp')
assert decoders == expected_decoders, decoders
encoders = codecs('-encoders')
assert encoders == {'libx264', 'aac', 'pcm_s16le'}, encoders
demuxers = set()
for line in listing('-demuxers').splitlines():
    fields = line.split()
    if fields and fields[0] == 'D':
        assert fields[1] == 'd', f'File demuxer in recorder: {line}'
        demuxers.add(fields[2])
expected = {'dshow', 'gdigrab'} if os.name == 'nt' else {'alsa', 'pulse', 'video4linux2,v4l2', 'x11grab'}
assert demuxers == expected, demuxers
filters = set(re.findall(r'^\s*[TS.]{2}\s+(\w+)\s', listing('-filters'), re.M))
profile = json.loads((root / 'scripts/ffmpeg-profile.json').read_text())
# FFmpeg's executable selects these orientation/crop dependencies itself.
assert filters == set(profile['capture_filters']) | {'crop', 'hflip', 'vflip', 'rotate', 'transpose'}, filters
muxers = {fields[1] for line in listing('-muxers').splitlines()
          if len(fields := line.split()) > 1 and fields[0] == 'E'}
assert muxers == {'mov', 'mp4', 'ipod', 'wav'}, muxers
print(f'Recorder verified: {sorted(demuxers)}, {sorted(decoders)}, {sorted(encoders)}')

if os.name != 'nt':
    # A private ALSA null device exercises feedback PCM/WAV without capture hardware.
    scratch = root / 'target'
    scratch.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='ms1-recorder-', dir=scratch) as directory:
        work = Path(directory)
        config = work / 'alsa.conf'
        config.write_text('pcm.ms1_null { type null }\n')
        output = work / 'feedback.wav'
        child_env = {'PATH': os.defpath, 'HOME': str(work),
                     'XDG_CONFIG_HOME': str(work), 'XDG_DATA_HOME': str(work),
                     'ALSA_CONFIG_PATH': str(config)}
        subprocess.run([str(exe), '-hide_banner', '-loglevel', 'error',
                        '-f', 'alsa', '-i', 'ms1_null', '-t', '0.05',
                        '-ac', '1', '-ar', '16000', '-c:a', 'pcm_s16le',
                        '-f', 'wav', str(output)], env=child_env,
                       check=True, timeout=15, capture_output=True)
        with wave.open(str(output), 'rb') as audio:
            assert (audio.getnchannels(), audio.getframerate(), audio.getsampwidth()) == (1, 16000, 2)
            assert audio.getnframes() > 0
    print('Feedback recorder verified: software ALSA input to 16 kHz mono PCM WAV')

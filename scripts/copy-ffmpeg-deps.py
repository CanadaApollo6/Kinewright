#!/usr/bin/env python3
"""Bundle the non-glibc shared dependencies of the verified local FFmpeg outputs."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

prefix, recorder, destination = map(Path, sys.argv[1:])
skip = re.compile(r'^(ld-linux|lib(c|m|mvec|pthread|dl|rt)\.so)')
files = [p for p in (prefix / 'lib').glob('*.so.*') if not p.is_symlink()]
files.append(recorder / 'bin/ffmpeg')
env = dict(os.environ, LD_LIBRARY_PATH=str(prefix.resolve() / 'lib'))
for binary in files:
    output = subprocess.check_output(['ldd', str(binary)], text=True, env=env)
    for line in output.splitlines():
        if '=> not found' in line:
            raise SystemExit(line)
        match = re.match(r'\s*([^/\s]+) => (/\S+) ', line)
        if match and not skip.match(match[1]):
            source = Path(match[2])
            if source.parent.resolve() != (prefix / 'lib').resolve():
                shutil.copyfile(source, destination / match[1])

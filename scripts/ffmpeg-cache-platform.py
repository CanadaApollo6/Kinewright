#!/usr/bin/env python3
"""Keep Linux native caches separate across distribution and glibc ABI changes."""
import os
from pathlib import Path
import platform

if platform.system() == 'Linux':
    release = platform.freedesktop_os_release()
    libc = os.confstr('CS_GNU_LIBC_VERSION').replace(' ', '-')
    abi = '-'.join([release['ID'], release['VERSION_ID'], libc])
else:
    abi = 'msvc14'

print(f'FFmpeg cache ABI: {abi}')
if environment := os.environ.get('GITHUB_ENV'):
    with Path(environment).open('a', encoding='utf-8') as output:
        output.write(f'FFMPEG_CACHE_ABI={abi}\n')

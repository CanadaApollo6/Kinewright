import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile


def restore(destination, archive=None):
    folder = Path(__file__).resolve().parent
    manifest = json.loads((folder/'manifest.json').read_text())
    raw = (folder/'runners.tar' if archive is None else Path(archive)).read_bytes()
    if hashlib.sha256(raw).hexdigest() != manifest['sha256']:
        raise ValueError('runner archive SHA-256 mismatch')
    expected = {'lane.py', 'final-measure.py', 'final-figures.py'}
    with tarfile.open(fileobj=io.BytesIO(raw)) as bundle:
        members = bundle.getmembers()
        if set(manifest['members']) != expected or len(members) != 3 or {m.name for m in members} != expected or not all(m.isfile() for m in members):
            raise ValueError('runner archive members mismatch')
        files = {m.name: bundle.extractfile(m).read() for m in members}
    if any(hashlib.sha256(raw).hexdigest() != manifest['members'][name] for name, raw in files.items()):
        raise ValueError('runner member SHA-256 mismatch')
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=False)
    for name, raw in files.items():
        (destination/name).write_bytes(raw)
    print('verified PF1 runner archive ' + manifest['sha256'])


if __name__ == '__main__':
    try:
        restore(sys.argv[1], sys.argv[2] if len(sys.argv) == 3 else None)
    except (ValueError, OSError, KeyError, tarfile.TarError) as error:
        print('invalid runner archive: ' + str(error), file=sys.stderr)
        sys.exit(2)

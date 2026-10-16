import json
from pathlib import Path
import validate as v

def collect(plan, root, work, launch):
    work.mkdir(parents=True, exist_ok=True)
    manifest = work/'series.json'
    if manifest.exists():
        v.require(v.read(manifest) == plan, 'changed immutable series manifest')
    else:
        with manifest.open('x') as output:
            json.dump(plan, output)
    verdicts = []
    for index, item in enumerate(plan['gates']):
        v.artifact(root, item['pins'])
        pinfile = root/item['pins']['path']
        pins, builds, host = v.spec(pinfile, root)
        series = dict(item['series'], id=plan['id'], pins=v.digest(pinfile))
        v.series_check(dict(series=series, runs=[]), pinfile, root, frozenset())
        destination = work/f'status-{index}.json'
        if not destination.exists():
            rows = []
            with (work/f'attempts-{index}.jsonl').open('x') as journal:
                for ordinal, entry in enumerate(pins['protocol']['order']):
                    role = 'reference' if entry['arm'] == pins['arms'].get('reference') else 'candidate'
                    try:
                        code, row = launch(index, ordinal, entry, role, pinfile, pins, builds, item)
                    except (OSError, ValueError) as error:
                        code, row = 2, dict(validity='invalid_evidence', exit=2, reason=str(error))
                    journal.write(json.dumps(dict(entry=entry, exit=code, row=row))+'\n')
                    journal.flush()
                    rows.append(row)
                    if code:
                        break
            snapshot = dict(lane=pins['lane'], gate=pins['gate'], series=series,
                            runs=[row for row in rows if 'fingerprint' in row],
                            artifacts=[dict(path=path.relative_to(root).as_posix(), sha256=v.digest(path)) for path in [Path(journal.name), *sorted(work.glob(f'{index}-*'))] if path.is_file()])
            if code and 'receipt' in rows[-1]:
                snapshot['failure'] = dict(exit=code, receipt=rows[-1]['receipt'])
            with destination.open('x') as output:
                json.dump(snapshot, output)
            if code:
                return code if code in [2, 75, 101] else 2
        v.require(v.read(destination)['series'] == series, 'cached series differs from frozen plan')
        verdicts.append(v.evaluate(pinfile, destination, root))
    return next((code for code in [2, 101, 1, 75] if code in verdicts), 0 if verdicts else 2)

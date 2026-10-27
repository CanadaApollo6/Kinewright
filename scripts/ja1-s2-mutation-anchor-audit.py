#!/usr/bin/env python3
"""Check every declarative mutation-case catalog in scripts against current source."""
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent.parent


def audit():
    catalogs = []
    for path in sorted((ROOT / "scripts").glob("*.json")):
        values = json.loads(path.read_text())
        schema = isinstance(values, list) and bool(values) and all(
            isinstance(value, dict) and "changes" in value and "name" in value
            for value in values
        )
        named_catalog = "mutation" in path.name and "cases" in path.name
        if not named_catalog and not schema:
            continue
        if not schema:
            raise ValueError(f"invalid or empty mutation catalog: {path}")
        rows = []
        for value in values:
            if value.get("superseded_by"):
                rows.append(dict(case=value["name"], superseded_by=value["superseded_by"]))
                continue
            changed = {}
            for name, before, after in value["changes"]:
                source = changed.get(name, (ROOT / name).read_text())
                count = source.count(before)
                rows.append(dict(case=value["name"], path=name, matches=count))
                if count == 1:
                    changed[name] = source.replace(before, after, 1)
        catalogs.append(dict(path=str(path.relative_to(ROOT)), cases=len(values), anchors=rows))
    anchors = [row for catalog in catalogs for row in catalog["anchors"] if "matches" in row]
    return dict(catalogs=catalogs, catalog_count=len(catalogs), active_anchors=len(anchors),
                superseded=sum("superseded_by" in row for catalog in catalogs for row in catalog["anchors"]),
                zero_match=sum(row["matches"] == 0 for row in anchors),
                multi_match=sum(row["matches"] > 1 for row in anchors))


if __name__ == "__main__":
    result = audit()
    print(json.dumps(result, indent=2, sort_keys=True))
    sys.exit(bool(result["zero_match"] or result["multi_match"] or not result["catalog_count"]))

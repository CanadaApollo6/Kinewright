#!/usr/bin/env python3
"""Edit and verify one C11 mutation. The parent shell runs Cargo at native nice 10."""
import hashlib
import json
from pathlib import Path
import re
import sys
ROOT = Path(__file__).resolve().parent.parent
CASES = json.loads((ROOT / "scripts/ja1-s2-c11-mutation-cases.json").read_text())
def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
def case(name):
    return next(value for value in CASES if value["name"] == name)
def check(value):
    changed = {}
    for name, before, after in value["changes"]:
        text = changed.get(name, (ROOT / name).read_text())
        if text.count(before) != 1:
            raise RuntimeError(f"non-unique mutation anchor: {value['name']} {name}")
        changed[name] = text.replace(before, after, 1)
    return changed
def main():
    operation = sys.argv[1]
    if operation == "list":
        for value in CASES:
            check(value)
            print(value["name"])
        return
    directory = Path(sys.argv[2])
    if operation == "prepare":
        value = case(sys.argv[3]); changed = check(value)
        directory.mkdir(parents=True)
        write(directory / "case.json", value)
        originals = {name: (ROOT / name).read_text() for name in changed}
        write(directory / "originals.json", originals)
        write(directory / "source.json", {name: hashlib.sha256(data.encode()).hexdigest() for name, data in originals.items()})
        for name, text in changed.items(): (ROOT / name).write_text(text)
    elif operation == "restore":
        for name, data in json.loads((directory / "originals.json").read_text()).items():
            (ROOT / name).write_text(data)
            if (ROOT / name).read_text() != data: raise RuntimeError("restoration differs")
    elif operation == "command":
        value = json.loads((directory / "case.json").read_text())
        command = ["cargo", "test", "-j", "4", "-p", value["package"]]
        if value["production"]: command.append("--no-default-features")
        if value["features"]: command += ["--features", value["features"]]
        command += [value["kind"]]
        if value["kind"] != "--lib": command.append(value["target"])
        command += [value["witness"], "--", "--exact", "--test-threads=1"]
        print("\n".join(command))
    elif operation == "host-command":
        value = json.loads((directory / "case.json").read_text())
        if value.get("build_host"):
            print("\n".join(["cargo", "build", "-j", "4", "-p", "kinewright-agent", "--no-default-features", "--features", "kinewright-project/test-util", "--bin", "kinewright"]))
    elif operation == "verify":
        phase, code = sys.argv[3], int(sys.argv[4])
        value = json.loads((directory / "case.json").read_text())
        output = (directory / (phase + ".stdout")).read_text()
        errors = (directory / (phase + ".stderr")).read_text()
        summaries = re.findall(r"^test result:.*$", output, re.M)
        outcome = "FAILED" if phase == "red" else "ok"
        expected = code == (101 if phase == "red" else 0) and len(summaries) == 1 and re.findall(r"^test (.+?) \.\.\. (ok|FAILED)$", output, re.M) == [(value["witness"], outcome)]
        paths = re.findall(r"^\s*Running .* \(([^()]+)\)$", errors, re.M)
        binary = (ROOT / paths[0]).resolve() if len(paths) == 1 else None
        expected = expected and binary is not None
        row = dict(phase=phase, exit=code, accepted=expected, summaries=summaries, binary=str(binary), binary_sha256=hashlib.file_digest(binary.open("rb"), "sha256").hexdigest() if binary else None)
        write(directory / (phase + "-result.json"), row)
        print(value["name"], phase, code, *summaries, flush=True)
        if not expected: raise RuntimeError("required runtime outcome was not observed")
    else: raise RuntimeError("unknown operation")
if __name__ == "__main__": main()

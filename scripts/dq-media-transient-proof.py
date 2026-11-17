"""R6 listening evidence from explicit baseline/replay paths; no data discovery."""
import argparse
import csv
import io
import hashlib
from bisect import bisect_left
from fractions import Fraction
import importlib.util
import json
from pathlib import Path
import sys


def load(path):
    return json.loads(path.read_text())


def inventory(measurement, candidates):
    profile, words = measurement["profile"], measurement["words"]
    hop = profile["hop_micros"]
    threshold = measurement["settings"]["quiet_rms_max"] + profile["levels"]["speech_rms_hundredths"] + 2000
    assert all(profile["rms"][i] <= threshold for gap in measurement["gaps"] for i in range(gap["start_micros"] // hop, gap["end_micros"] // hop)), "pause candidates need their own component inventory on this take"
    components = []
    for i, level in enumerate(profile["rms"]):
        if level <= threshold:
            continue
        a, b = i * hop, min((i + 1) * hop, profile["duration_micros"])
        if components and components[-1][1] == a:
            components[-1][1] = b
        else:
            components.append([a, b])
    raw, audible, physical = {}, {}, {}
    for row in candidates:
        c = row["candidate"]
        if c is not None:
            raw[row["index"]] = c["raw_source_us"]
            if c.get("audible_acoustic_source_us") is not None:
                audible[row["index"]] = [Fraction(*x) for x in c["audible_acoustic_source_us"]]
            if c.get("inside_clip"):
                physical[row["index"]] = [Fraction(*x) for x in c["physical_source_us"]]
    starts = [w["start_micros"] for w in words]
    rows = []
    for number, (a, b) in enumerate(components):
        first, end = a // hop, (b + hop - 1) // hop
        if first == 0 or end == len(profile["rms"]) or profile["rms"][first - 1] > threshold or profile["rms"][end] > threshold:
            continue
        rows.append({"component": number, "start_us": a, "end_us": b, "length_us": b-a,
                     "voice_hops": sum(profile["voice"][first:end]), "hops": end-first,
                     "left_rms_cdb": profile["rms"][first-1], "right_rms_cdb": profile["rms"][end],
                     "left_voice": profile["voice"][first-1], "right_voice": profile["voice"][end],
                     "core_proposal_contains": [i for i,(x,y) in raw.items() if x <= a and b <= y],
                     "filler_audible_proposal_contains": [i for i,(x,y) in audible.items() if x <= a and b <= y],
                     "filler_framed_candidate_contains": [i for i,(x,y) in physical.items() if x <= a and b <= y],
                     "filler_framed_candidate_intersects": [i for i,(x,y) in physical.items() if max(x,a) < min(y,b)],
                     "pause_candidate_contains": [], "asr_starts_inside": [{"index": i, "text": words[i]["text"], "start_us": starts[i]}
                                           for i in range(bisect_left(starts,a),bisect_left(starts,b))]})
    output = io.StringIO(newline="")
    writer = csv.DictWriter(output, fieldnames=list(rows[0]), lineterminator="\n")
    writer.writeheader()
    for row in rows:
        writer.writerow({k: json.dumps(v,ensure_ascii=False) if isinstance(v,list) else v for k,v in row.items()})
    return rows, output.getvalue().encode()


def proof(previous, current, audio, preflight):
    spec = importlib.util.spec_from_file_location("early_proof", Path(__file__).with_name("dq-media-early-onset-proof.py"))
    helper = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = helper
    spec.loader.exec_module(helper)
    measurement = load(current / "measurement.json")
    assert measurement == load(previous / "measurement.json")
    first = load(preflight)
    assert first["clear_count"] == len(first["eligible_initial_refusals"]) <= 10
    inventory_rows, inventory_csv = inventory(measurement, first["candidates"])
    assert len(inventory_rows) == first["detached_components"]
    assert hashlib.sha256(inventory_csv).hexdigest() == first["inventory_sha256"]
    (current / "detached-components.csv").write_bytes(inventory_csv)
    old = {r["action"]["index"]: r for r in load(previous / "run1-results.json") if r["action"]["kind"] == "filler"}
    new = {r["action"]["index"]: r for r in load(current / "run1-results.json") if r["action"]["kind"] == "filler"}
    components = {(r["start_us"],r["end_us"]): r for r in inventory_rows}
    eligible = []
    for candidate in first["candidates"]:
        i = candidate["index"]
        reason = old[i].get("reason", {})
        component = reason.get("component")
        if reason.get("violation") != "unattributed_sound" or component is None:
            continue
        a, b = component["start_micros"], component["end_micros"]
        row = components.get((a,b))
        c = candidate["candidate"]
        if row is not None and b-a <= 20_000 and c is not None:
            left,right = [Fraction(*x) for x in c["audible_acoustic_source_us"]]
            if left <= a and b <= right and all(w["index"] == i for w in row["asr_starts_inside"]):
                eligible.append(i)
    assert eligible == first["eligible_initial_refusals"]
    candidates = {r["index"]: r for r in first["candidates"]}
    ready = lambda r: r["status"] in ("moved", "as_asked")
    newly_applied = sorted(i for i in new if ready(new[i]) and not ready(old[i]))
    selected = sorted(set(first["eligible_initial_refusals"]) | set(newly_applied))
    assert {115, 148, 662}.issubset(selected)
    rows = []
    source = helper.WaveInput.read(audio)
    edited = helper.WaveInput.read(current / "edited.wav")
    run = load(current / "summary.json")["runs"][0]
    fps = Fraction(run["fps"]["numerator"], run["fps"]["denominator"])
    applied = [helper.cut_extent(r) for r in run["results"] if ready(r)]
    clips = load(current / "run1-document.json")["tracks"][0]["clips"]
    listen = current / "listen"
    listen.mkdir(exist_ok=False)
    index = ["R6 transient listening pack", "", "Before: original PCM, candidate/cut plus 1.5 seconds either side.",
             "After READY: production edited export, final join plus/minus 1.5 seconds.",
             "After REFUSED: production export around the retained filler ASR start; there is no join for that request.",
             "Refused candidates are included to cover the six initially eligible blockers, including #115, #148, #662.", ""]
    for number, i in enumerate(selected, 1):
        row, word = new[i], measurement["words"][i]
        if ready(row):
            a, b = helper.cut_extent(row)
            center = a - sum(max(Fraction(0), min(a, y) - x) for x, y in applied)
            kind = "applied join"
        else:
            a, b = map(Fraction, candidates[i]["candidate"]["raw_source_us"])
            at = Fraction(word["start_micros"])
            matches = [c for c in clips if c["source_range"]["start"] * 1_000_000 / fps <= at < c["source_range"]["end"] * 1_000_000 / fps]
            assert len(matches) == 1, (i, matches)
            c = matches[0]
            center = at + (c["timeline_start"] - c["source_range"]["start"]) * 1_000_000 / fps
            kind = "retained filler (refused)"
        predecessors = [(j,w) for j,w in enumerate(measurement["words"][:i])
                        if w["start_micros"] < a and not any(x <= w["start_micros"] < y for x,y in applied)]
        previous_kept = predecessors[-1] if predecessors else None
        component = old[i].get("reason",{}).get("component")
        before = source.crop(listen / f"{number:02}-before.wav", a - 1_500_000, b + 1_500_000)
        after = edited.crop(listen / f"{number:02}-after.wav", center - 1_500_000, center + 1_500_000)
        record = {"index": i, "word": word["text"], "asr_start_us": word["start_micros"], "newly_applied": i in newly_applied,
                  "status": row["status"], "reason": row.get("reason"), "initial_component": component,
                  "previous_kept": ({"index": previous_kept[0], **previous_kept[1]} if previous_kept else None),
                  "previous_start_to_cut_us": float(a-previous_kept[1]["start_micros"]) if previous_kept else None, "source_window_us": [float(a), float(b)],
                  "edited_center_us": float(center), "after_kind": kind, "before": before, "after": after}
        rows.append(record)
        index += [f"{number:02}: #{i} {word['text']!r}, ASR {word['start_micros']/1e6:.6f} s; {kind}",
                  f"candidate/cut {float(a/1e6):.6f}..{float(b/1e6):.6f} s; export center {float(center/1e6):.6f} s",
                  "initial refused component " + json.dumps(component),
                  "previous kept word (ASR start only) " + json.dumps(record["previous_kept"]),
                  "previous start to cut left, us " + json.dumps(record["previous_start_to_cut_us"]),
                  "reason " + json.dumps(record["reason"]), "before " + json.dumps(before), "after " + json.dumps(after), ""]
    (listen / "index.txt").write_text("\n".join(index) + "\n")
    result = {"eligible_initial_refusals": first["eligible_initial_refusals"], "newly_applied_fillers": newly_applied, "rows": rows}
    (current / "transients.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("previous", type=Path)
    parser.add_argument("current", type=Path)
    parser.add_argument("audio", type=Path)
    parser.add_argument("preflight", type=Path)
    args = parser.parse_args()
    print(json.dumps(proof(args.previous, args.current, args.audio, args.preflight)))

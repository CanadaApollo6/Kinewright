"""Audit explicit rehearsal outputs and copy early-onset listening windows."""

import argparse
import array
from dataclasses import dataclass
from fractions import Fraction
import json
import math
from pathlib import Path
import struct
import sys


@dataclass(frozen=True)
class WaveInput:
    path: Path
    fmt: bytes
    offset: int
    frames: int
    encoding: int
    channels: int
    rate: int
    align: int
    bits: int

    @classmethod
    def read(cls, path):
        with path.open("rb") as stream:
            if stream.read(4) != b"RIFF":
                raise ValueError(f"not a RIFF WAV: {path}")
            stream.seek(8)
            if stream.read(4) != b"WAVE":
                raise ValueError(f"not a WAVE file: {path}")
            fmt = data = None
            while header := stream.read(8):
                kind, size = struct.unpack("<4sI", header)
                position = stream.tell()
                if kind == b"fmt ":
                    fmt = stream.read(size)
                elif kind == b"data":
                    data = (position, size)
                stream.seek(position + size + size % 2)
            if fmt is None or data is None:
                raise ValueError(f"missing WAV format or samples: {path}")
            encoding, channels, rate, _, align, bits = struct.unpack("<HHIIHH", fmt[:16])
            if encoding not in (1, 3) or align != channels * bits // 8:
                raise ValueError(f"unsupported WAV format: {path}")
            if data[1] % align or data[0] + data[1] > path.stat().st_size:
                raise ValueError(f"incomplete WAV frames: {path}")
            return cls(path, fmt, data[0], data[1] // align,
                       encoding, channels, rate, align, bits)

    def sample_range(self, start_us, end_us):
        first = max(0, math.ceil(start_us * self.rate / 1_000_000))
        end = min(self.frames, math.ceil(end_us * self.rate / 1_000_000))
        if not 0 <= first < end <= self.frames:
            raise ValueError(f"empty or invalid WAV window: {start_us}, {end_us}")
        return first, end

    def bytes_at(self, first, end):
        with self.path.open("rb") as stream:
            stream.seek(self.offset + first * self.align)
            samples = stream.read((end - first) * self.align)
        if len(samples) != (end - first) * self.align:
            raise ValueError(f"short sample read: {self.path}")
        return samples

    def rms_dbfs(self, start_us, end_us):
        if (self.encoding, self.channels, self.bits) != (1, 1, 16):
            raise ValueError("source RMS requires mono 16-bit PCM")
        first, end = self.sample_range(start_us, end_us)
        values = array.array("h", self.bytes_at(first, end))
        if sys.byteorder != "little":
            values.byteswap()
        power = sum(value * value for value in values) / len(values)
        return 10 * math.log10(power / 32768**2) if power else -math.inf

    def crop(self, destination, start_us, end_us):
        first, end = self.sample_range(start_us, end_us)
        samples = self.bytes_at(first, end)
        chunks = b"fmt " + struct.pack("<I", len(self.fmt)) + self.fmt
        if len(self.fmt) % 2:
            chunks += b"\0"
        chunks += b"data" + struct.pack("<I", len(samples)) + samples
        if len(samples) % 2:
            chunks += b"\0"
        with destination.open("xb") as stream:
            stream.write(b"RIFF" + struct.pack("<I", len(chunks) + 4) + b"WAVE" + chunks)
        return {"file": destination.name, "frames": end - first,
                "sample_rate": self.rate, "channels": self.channels,
                "window_us": [first * 1_000_000 / self.rate, end * 1_000_000 / self.rate]}


def cut_extent(row):
    return tuple(Fraction(*value) for value in row["actual_source_us"])


def rms_components(profile, threshold):
    result = []
    hop = profile["hop_micros"]
    for index, level in enumerate(profile["rms"]):
        if level <= threshold:
            continue
        start, end = index * hop, min((index + 1) * hop, profile["duration_micros"])
        if result and result[-1][1] == start:
            result[-1][1] = end
        else:
            result.append([start, end])
    return result


def early_cuts(measurement, cuts):
    profile, words = measurement["profile"], measurement["words"]
    threshold = (measurement["settings"]["quiet_rms_max"]
                 + profile["levels"]["speech_rms_hundredths"] + 2000)
    components = rms_components(profile, threshold)
    extents = [cut_extent(row) for row in cuts]
    result = []
    for row in cuts:
        if row["action"]["kind"] != "filler":
            continue
        index = row["action"]["index"]
        start = words[index]["start_micros"]
        a, b = cut_extent(row)
        for left, right in components:
            if not (max(a, left) < min(b, right) and left <= start < right
                    and start - left > 300_000):
                continue
            assert start - left <= 750_000
            hop = profile["hop_micros"]
            assert any(profile["voice"][left // hop:math.ceil(right / hop)])
            assert not any(left <= word["start_micros"] <= start
                           for i, word in enumerate(words) if i != index)
            predecessors = [(i, word) for i, word in enumerate(words[:index])
                            if word["start_micros"] < a
                            and not any(x <= word["start_micros"] < y for x, y in extents)]
            previous = predecessors[-1] if predecessors else None
            result.append({"word_index": index, "word": words[index]["text"],
                           "asr_start_us": start, "component_us": [left, right],
                           "component_early_us": start - left,
                           "cut_exact_us": row["actual_source_us"],
                           "cut_us": [float(a), float(b)],
                           "previous_kept": ({"index": previous[0], "text": previous[1]["text"],
                                              "start_micros": previous[1]["start_micros"]}
                                             if previous else None),
                           "previous_start_to_cut_us": (float(a - previous[1]["start_micros"])
                                                        if previous else None)})
    return sorted(result, key=lambda row: (row["cut_us"][0], row["word_index"]))


def prefix_hops(source, row, hop_us, quiet_dbfs):
    a, b = (Fraction(*value) for value in row["cut_exact_us"])
    stop = min(b, row["asr_start_us"])
    found = []
    for grid in range(int(a // hop_us), math.ceil(stop / hop_us)):
        left, right = max(a, grid * hop_us), min(stop, (grid + 1) * hop_us)
        if left >= right:
            continue
        level = source.rms_dbfs(left, right)
        if level > quiet_dbfs:
            found.append({"grid_start_us": grid * hop_us, "start_us": float(left),
                          "end_us": float(right), "rms_dbfs": level,
                          "above_quiet_db": level - quiet_dbfs})
    return found


def audit(root, audio):
    read = lambda name: json.loads((root / name).read_text())
    summary, measurement = read("summary.json"), read("measurement.json")
    run = summary["runs"][0]
    cuts = [row for row in run["results"] if row["status"] in ("as_asked", "moved")]
    rows = early_cuts(measurement, cuts)
    profile = measurement["profile"]
    quiet = (measurement["settings"]["quiet_rms_max"]
             + profile["levels"]["speech_rms_hundredths"] + 2000) / 100
    source, edited = WaveInput.read(audio), WaveInput.read(root / "edited.wav")
    fps = Fraction(run["fps"]["numerator"], run["fps"]["denominator"])
    extents = [cut_extent(row) for row in cuts]
    assert sum((b - a) * fps / 1_000_000 for a, b in extents) == run["before_frames"] - run["after_frames"]
    boundaries = {0, run["after_frames"]}
    for track in read("run1-document.json")["tracks"]:
        for clip in track["clips"]:
            boundaries.update((clip["timeline_start"], clip["timeline_start"]
                               + clip["source_range"]["end"] - clip["source_range"]["start"]))
    listen = root / "listen"
    listen.mkdir(exist_ok=False)
    index = ["R5 early-onset listening pack", "",
             "Before: source PCM, full cut plus 1.5 s on each side.",
             "After: production edited export, 1.5 s on each side of the final join.",
             "Previous kept word means its ASR start survives all final source cuts.",
             "Word ends are not used. Above-quiet sound is measured, not identified as a word.", ""]
    for number, row in enumerate(rows, 1):
        a, b = (Fraction(*value) for value in row["cut_exact_us"])
        hops = prefix_hops(source, row, profile["hop_micros"], quiet)
        row["prefix_above_quiet_hops"] = hops
        row["loudest_prefix_hop"] = max(hops, key=lambda value: value["rms_dbfs"], default=None)
        join = a - sum(max(Fraction(0), min(a, y) - x) for x, y in extents)
        join_frame = join * fps / 1_000_000
        assert join_frame.denominator == 1 and join_frame in boundaries
        row["edited_join_us"] = float(join)
        row["before"] = source.crop(listen / f"{number:02}-before.wav", a - 1_500_000, b + 1_500_000)
        row["after"] = edited.crop(listen / f"{number:02}-after.wav", join - 1_500_000, join + 1_500_000)
        previous = row["previous_kept"]
        label = (f"#{previous['index']} {previous['text']!r} at {previous['start_micros']/1e6:.6f} s"
                 if previous else "none")
        loudest = row["loudest_prefix_hop"]
        index.extend([f"{number:02}: filler #{row['word_index']} {row['word']!r}, ASR {row['asr_start_us']/1e6:.6f} s",
                      f"component {row['component_us'][0]/1e6:.6f}..{row['component_us'][1]/1e6:.6f} s; early {row['component_early_us']/1000:.3f} ms",
                      f"cut {float(a/1_000_000):.6f}..{float(b/1_000_000):.6f} s; final join {float(join/1_000_000):.6f} s",
                      f"previous kept {label}; ASR-start to cut-left {row['previous_start_to_cut_us']} us",
                      "loudest above-quiet pre-filler hop: " + json.dumps(loudest),
                      f"before {json.dumps(row['before'])}", f"after {json.dumps(row['after'])}", ""])
    (listen / "index.txt").write_text("\n".join(index) + "\n")
    result = {"quiet_dbfs": quiet, "count": len(rows), "rows": rows,
              "method": "Production RMS-only components from the saved centidB profile. Prefix RMS uses source-aligned 10 ms hops clipped to the exact cut and filler ASR start. Listening files copy original/export sample frames. No ASR end establishes ownership."}
    (root / "early-onsets.json").write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("audio", type=Path)
    options = parser.parse_args()
    result = audit(options.root, options.audio)
    print(json.dumps({"early_applied_fillers": result["count"], "quiet_dbfs": result["quiet_dbfs"]}))

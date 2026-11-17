"""Synthetic controls for the on-demand early-onset measurement script."""

import math
from pathlib import Path
import runpy
import struct
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
PROOF = runpy.run_path(str(ROOT / "scripts/dq-media-early-onset-proof.py"))


class EarlyOnsetTests(unittest.TestCase):
    def test_component_offset_selects_the_cut_and_previous_surviving_start(self):
        profile = {"rms": [-9000] * 300, "voice": [True] * 300,
                   "hop_micros": 10000, "duration_micros": 3000000,
                   "levels": {"speech_rms_hundredths": -4000}}
        profile["rms"][70:150] = [-4000] * 80
        measurement = {"profile": profile, "settings": {"quiet_rms_max": -4000},
                       "words": [{"text": "kept", "start_micros": 0},
                                 {"text": "uh", "start_micros": 100000},
                                 {"text": "um", "start_micros": 1200000}]}
        cuts = [{"action": {"kind": "filler", "index": 1},
                 "actual_source_us": [[100000, 1], [120000, 1]]},
                {"action": {"kind": "filler", "index": 2},
                 "actual_source_us": [[800000, 1], [1450000, 1]]}]
        result = PROOF["early_cuts"](measurement, cuts)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["component_early_us"], 500000)
        self.assertEqual(result[0]["previous_kept"]["index"], 0)
        self.assertEqual(result[0]["previous_start_to_cut_us"], 800000)
        profile["rms"][70:90] = [-9000] * 20
        self.assertEqual(PROOF["early_cuts"](measurement, cuts), [])

    def test_prefix_rms_is_clipped_before_the_filler_and_copy_keeps_samples(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "target") as directory:
            path = Path(directory) / "source.wav"
            samples = struct.pack("<160h", *([1000] * 80 + [30000] * 80))
            fmt = struct.pack("<HHIIHH", 1, 1, 16000, 32000, 2, 16)
            chunks = b"JUNK" + struct.pack("<I", 3) + b"abc\0"
            chunks += b"fmt " + struct.pack("<I", len(fmt)) + fmt
            chunks += b"data" + struct.pack("<I", len(samples)) + samples
            path.write_bytes(b"RIFF" + struct.pack("<I", len(chunks) + 4) + b"WAVE" + chunks)
            source = PROOF["WaveInput"].read(path)
            row = {"cut_exact_us": [[0, 1], [10000, 1]], "asr_start_us": 5000}
            hops = PROOF["prefix_hops"](source, row, 10000, -60)
            self.assertEqual(len(hops), 1)
            self.assertAlmostEqual(hops[0]["rms_dbfs"], 20 * math.log10(1000 / 32768))
            self.assertEqual(hops[0]["end_us"], 5000)
            copy = Path(directory) / "copy.wav"
            source.crop(copy, 1250, 3750)
            copied = PROOF["WaveInput"].read(copy)
            self.assertEqual(copied.frames, 40)
            self.assertEqual(copied.bytes_at(0, 40), struct.pack("<40h", *([1000] * 40)))

    def test_truncated_wave_is_refused_before_it_can_be_measured(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "target") as directory:
            path = Path(directory) / "bad.wav"
            fmt = struct.pack("<HHIIHH", 1, 1, 16000, 32000, 2, 16)
            path.write_bytes(b"RIFF" + struct.pack("<I", 40) + b"WAVEfmt "
                             + struct.pack("<I", len(fmt)) + fmt + b"data"
                             + struct.pack("<I", 4) + b"\0\0")
            with self.assertRaises(ValueError):
                PROOF["WaveInput"].read(path)

    def test_edited_crop_preserves_stereo_float_frames_and_clock(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "target") as directory:
            path = Path(directory) / "edited.wav"
            samples = struct.pack("<1920f", *(value / 1920 for value in range(1920)))
            fmt = struct.pack("<HHIIHH", 3, 2, 48000, 384000, 8, 32)
            chunks = b"fmt " + struct.pack("<I", len(fmt)) + fmt
            chunks += b"data" + struct.pack("<I", len(samples)) + samples
            path.write_bytes(b"RIFF" + struct.pack("<I", len(chunks) + 4) + b"WAVE" + chunks)
            source = PROOF["WaveInput"].read(path)
            copy = Path(directory) / "after.wav"
            window = source.crop(copy, 5000, 15000)
            copied = PROOF["WaveInput"].read(copy)
            self.assertEqual((copied.encoding, copied.channels, copied.rate), (3, 2, 48000))
            self.assertEqual(window["window_us"], [5000, 15000])
            self.assertEqual(copied.frames, 480)
            self.assertEqual(copied.bytes_at(0, 480), samples[240 * 8:720 * 8])


if __name__ == "__main__":
    unittest.main()

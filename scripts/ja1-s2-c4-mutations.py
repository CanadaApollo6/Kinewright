#!/usr/bin/env python3
"""Run the explicit native-history C4 catalogue through the sealed-control runner."""
import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location('native_mutations', Path(__file__).with_name('ja1-s2-c6-mutations.py'))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)

if __name__ == '__main__':
    runner.main('c4')

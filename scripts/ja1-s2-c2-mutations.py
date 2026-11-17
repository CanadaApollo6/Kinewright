#!/usr/bin/env python3
"""Native cut and authority witnesses; obsolete save-ticket cases are listed explicitly."""
from pathlib import Path
import runpy

if __name__ == "__main__":
    runpy.run_path(str(Path(__file__).with_name("ja1-s2-c6-mutations.py")))["main"]("c2")

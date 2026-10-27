"""Check real libtest capture and NOT RUN accounting with an unavailable session bus."""
from pathlib import Path
import runpy
import gzip

ROOT = Path(__file__).resolve().parents[3]
module = runpy.run_path(str(ROOT / "scripts/slow_tests.py"))
context = module["verify_slow"].__globals__
name = "s4_c2_proc_shows_only_the_namespace"
context["read_manifest"] = lambda: [("ja_s4_guardian", name, "source", "linux-only")]
context["host_os"] = lambda: "linux"
path = ROOT / "docs/reports/ja1-s4/c2-fix2/skip-capture.log"
log = path.read_text() if path.exists() else gzip.decompress(path.with_suffix(path.suffix + ".gz").read_bytes()).decode()
assert "---- " + name + " stdout ----" in log
assert "C2_NATIVE_NOT_RUN test=" + name + " reason=" in log
assert module["verify_slow"](log) == 0
print("real captured prerequisite refusal accounted as NOT RUN")

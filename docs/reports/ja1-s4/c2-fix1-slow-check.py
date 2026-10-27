"""Exercise explicit native skips and platform-only accounting without Cargo."""
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
import runpy

ROOT = Path(__file__).resolve().parents[3]
module = runpy.run_path(str(ROOT / "scripts/slow_tests.py"))
context = module["verify_slow"].__globals__
context["read_manifest"] = lambda: [
    ("ja_s4_guardian", "s4_probe", "source", "linux-only"),
    ("ja_s4_guardian", "s4_portable", "source", "any"),
]
context["read_allowlist"] = lambda: []
context["host_os"] = lambda: "linux"
header = "Running tests/ja_s4_guardian.rs (target/debug/deps/ja_s4_guardian-1234567890abcdef)\n\n"
linux = header + """running 2 tests
test s4_probe ...
C2_NATIVE_NOT_RUN test=s4_probe reason=no user manager
ok
test s4_portable ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
"""
# libtest leaves a space after its three dots before a direct stderr write.
linux = linux.replace("s4_probe ...\n", "s4_probe ... \n")
windows = header + """running 1 test
test s4_portable ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
"""

def check(label, function, log, expected, *args):
    output = StringIO()
    with redirect_stdout(output):
        result = function(log, *args)
    assert result == expected, (label, result, output.getvalue())
    print(label + ": passed")

check("slow explicit prerequisite skip counted", module["verify_slow"], linux, 0)
check("slow unknown skip refused", module["verify_slow"], linux.replace("test=s4_probe", "test=s4_unknown"), 1)
check("slow malformed skip refused", module["verify_slow"], linux.replace("reason=no user manager", "invalid"), 1)
context["host_os"] = lambda: "windows"
check("slow other-platform absence accounted", module["verify_slow"], windows, 0)
# The portable fixture stays fast; only the Linux probe is a slow entry.
context["read_manifest"] = lambda: [("ja_s4_guardian", "s4_probe", "source", "linux-only")]
check("fast other-platform absence accounted", module["verify_fast"], windows, 0, "windows")
fast = linux.replace("C2_NATIVE_NOT_RUN test=s4_probe reason=no user manager\nok", "ignored, slow tier: cargo test --features slow-tests").replace("2 passed; 0 failed; 0 ignored", "1 passed; 0 failed; 1 ignored")
check("fast local-platform skip required", module["verify_fast"], fast, 0, "linux")
print("6 slow-tier accounting checks passed")

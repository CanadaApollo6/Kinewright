#!/usr/bin/env python3
"""Regression tests for scripts/slow_tests.py's cargo-output parsing (stdlib unittest).

CI exports CARGO_TERM_COLOR=always (dtolnay/rust-toolchain does), which puts ANSI
escapes in cargo's `Running` lines; a parser that only knows plain text then sees no
binaries at all. These tests build the logs from the real manifest and allowlist, so
they cannot drift from them.

    python3 scripts/slow_tests_selftest.py
"""

import contextlib
import io
import os
import sys
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import slow_tests as st  # noqa: E402

RESET = "\x1b[0m"


def running(binary: str, colour: bool) -> str:
    target = f"unittests src/lib.rs (target/debug/deps/{binary}-0123456789abcdef)"
    if colour:
        return f"\x1b[1m\x1b[92m     Running{RESET} {target}"
    return f"     Running {target}"


def outcome(word: str, colour: bool) -> str:
    return f"\x1b[32m{word}{RESET}" if colour else word


def build_log(rows: list[tuple[str, str, str]], colour: bool) -> str:
    """rows are (binary, test path, outcome text); grouped into one section per binary."""
    sections: dict[str, list[str]] = {}
    for binary, path, text in rows:
        head, _, tail = text.partition(",")
        sections.setdefault(binary, []).append(
            f"test {path} ... {outcome(head, colour)}{',' + tail if tail else ''}"
        )
    lines = []
    for binary, tests in sections.items():
        lines += [running(binary, colour), "", f"running {len(tests)} tests", *tests, ""]
    return "\n".join(lines) + "\n"


def fast_rows() -> list[tuple[str, str, str]]:
    return [(b, p, f"ignored, {st.REASON}") for b, p, _ in st.read_manifest()] + [
        (b, p, "ignored, allowlisted") for b, p, _ in st.read_ignored()
    ]


def slow_rows() -> list[tuple[str, str, str]]:
    return [(b, p, "ok") for b, p, _ in st.read_manifest()]


def check(verify, log: str) -> tuple[int, str]:
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = verify(log)
    return code, out.getvalue()


class ParseTests(unittest.TestCase):
    def test_plain_fast_log_passes(self):
        code, out = check(st.verify_fast, build_log(fast_rows(), colour=False))
        self.assertEqual(code, 0, out)

    def test_coloured_fast_log_passes(self):
        code, out = check(st.verify_fast, build_log(fast_rows(), colour=True))
        self.assertEqual(code, 0, out)

    def test_plain_slow_log_passes(self):
        code, out = check(st.verify_slow, build_log(slow_rows(), colour=False))
        self.assertEqual(code, 0, out)

    def test_coloured_slow_log_passes(self):
        code, out = check(st.verify_slow, build_log(slow_rows(), colour=True))
        self.assertEqual(code, 0, out)

    def test_coloured_log_still_reports_a_real_unexpected_skip(self):
        rows = fast_rows() + [("kinewright_core", "some::ordinary_test", "ignored, flaky")]
        code, out = check(st.verify_fast, build_log(rows, colour=True))
        self.assertEqual(code, 1)
        self.assertIn("some::ordinary_test", out)
        self.assertNotIn("not skipped by the fast tier", out)

    def test_empty_or_unrecognised_output_is_a_parse_failure_not_missing_skips(self):
        for verify in (st.verify_fast, st.verify_slow):
            for log in ("", "error: could not compile `kinewright-core`\n", "garbage\nmore garbage\n"):
                code, out = check(verify, log)
                self.assertEqual(code, 1, (verify.__name__, log))
                self.assertIn("could not parse cargo test output", out)
                self.assertNotIn("not skipped by the fast tier", out)
                self.assertNotIn("did not run", out)

    def test_running_lines_without_any_test_line_is_a_parse_failure(self):
        log = running("kinewright_core", colour=True) + "\n\nrunning 0 tests\n"
        for verify in (st.verify_fast, st.verify_slow):
            code, out = check(verify, log)
            self.assertEqual(code, 1)
            self.assertIn("could not parse cargo test output", out)

    def test_test_lines_without_any_running_line_is_a_parse_failure(self):
        log = "test some::test ... ok\n"
        code, out = check(st.verify_slow, log)
        self.assertEqual(code, 1)
        self.assertIn("could not parse cargo test output", out)


class RunCargoTests(unittest.TestCase):
    def test_child_gets_no_colour_even_when_the_parent_asks_for_it(self):
        old = os.environ.get("CARGO_TERM_COLOR")
        os.environ["CARGO_TERM_COLOR"] = "always"
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                code, captured = st.run_cargo(
                    [sys.executable, "-c", "import os; print('COLOR=' + os.environ['CARGO_TERM_COLOR'])"]
                )
        finally:
            if old is None:
                del os.environ["CARGO_TERM_COLOR"]
            else:
                os.environ["CARGO_TERM_COLOR"] = old
        self.assertEqual(code, 0)
        self.assertIn("COLOR=never", captured)


if __name__ == "__main__":
    unittest.main()

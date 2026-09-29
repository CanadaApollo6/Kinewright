#!/usr/bin/env python3
"""Regression tests for scripts/slow_tests.py's parsing and skipped-set checks (stdlib unittest).

The logs are built from the real manifest and allowlist in the shape cargo and libtest
print: unit, integration (`tests/x.rs`), Windows (`.exe`, backslashes) and `Doc-tests`
sections, each with `running N tests` and a `test result:` summary. Cases cover ANSI
colour (dtolnay/rust-toolchain exports CARGO_TERM_COLOR=always), lost headers, orphan
results, count mismatches, and the per-OS observation of every allowlist entry.

    python3 scripts/slow_tests_selftest.py
"""

import contextlib
import io
import os
import re
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import slow_tests as st  # noqa: E402

RESET = "\x1b[0m"
HASH = "0123456789abcdef"
INTEGRATION = {"au3_fixtures", "mcp_server", "generated_media", "new_harness_e2e"}
DOCTEST = "doc-tests:kinewright_core"
SCRATCH_DOCTEST = (
    "crates/kinewright-core/src/lib.rs - Scratch::probe (line 291)",
    "crates/kinewright-core/src/lib.rs",
)


def header(binary: str, colour: bool, windows: bool) -> str:
    if binary.startswith("doc-tests:"):
        return f"   Doc-tests {binary.split(':', 1)[1]}"
    if binary in INTEGRATION:
        target = f"tests/{binary}.rs (target/debug/deps/{binary}-{HASH})"
    else:
        target = f"unittests src/lib.rs (target/debug/deps/{binary}-{HASH})"
    if windows:
        target = target.replace("/", "\\").replace(f"{HASH})", f"{HASH}.exe)")
    if colour:
        return f"\x1b[1m\x1b[92m     Running{RESET} {target}"
    return f"     Running {target}"


def outcome(text: str, colour: bool) -> str:
    head, sep, tail = text.partition(",")
    return (f"\x1b[32m{head}{RESET}" if colour else head) + sep + tail


def build_lines(rows, colour=False, windows=False, filtered=3) -> list[str]:
    """rows are (binary, test path, outcome text); one section per binary, in first-seen order."""
    sections: dict[str, list[tuple[str, str]]] = {}
    for binary, path, text in rows:
        sections.setdefault(binary, []).append((path, text))
    lines = []
    for binary, tests in sections.items():
        lines += [header(binary, colour, windows), "", f"running {len(tests)} test{'s' if len(tests) != 1 else ''}"]
        for path, text in tests:
            lines.append(f"test {path} ... {outcome(text, colour)}")
        ignored = sum(1 for _, t in tests if t.startswith("ignored"))
        passed = sum(1 for _, t in tests if t == "ok")
        lines += [
            "",
            f"test result: ok. {passed} passed; 0 failed; {ignored} ignored; 0 measured; {filtered} filtered out; "
            "finished in 0.01s",
            "",
        ]
    return lines


def build_log(rows, **kwargs) -> str:
    return "\n".join(build_lines(rows, **kwargs)) + "\n"


def fast_rows(os_name: str, extra_ok=True):
    rows = [(b, p, f"ignored, {st.REASON}") for b, p, _ in st.read_manifest()]
    applies = st.allowlist_for(os_name)
    rows += [(b, p, "ignored, allowlisted") for b, p in sorted(applies)]
    if extra_ok:
        rows += [("kinewright_core", "ordinary::passes", "ok"), ("mcp_server", "ordinary::also_passes", "ok")]
    return rows


def slow_rows():
    return [(b, p, "ok") for b, p, _ in st.read_manifest()]


def check(verify, log: str, *args) -> tuple[int, str]:
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = verify(log, *args)
    return code, out.getvalue()


@contextlib.contextmanager
def allowlist(edit=lambda lines: lines):
    """Point slow_tests at a temporary copy of ci/ignored-tests.txt with `edit` applied."""
    original = st.IGNORED
    text = original.read_text(encoding="utf-8").splitlines()
    with tempfile.TemporaryDirectory(prefix="kinewright-ci-selftest-") as directory:
        path = Path(directory) / "ignored-tests.txt"
        path.write_text("\n".join(edit(text)) + "\n", encoding="utf-8")
        st.IGNORED = path
        try:
            yield
        finally:
            st.IGNORED = original


def set_os(name_fragment: str, os_name: str):
    """An `edit` that rewrites the os column of the entry whose line contains `name_fragment`."""

    def edit(lines):
        out, hit = [], False
        for line in lines:
            if name_fragment in line and not line.startswith("#"):
                code, sep, reason = line.partition("  #")
                head, _, rest = code.rpartition(" any ")
                assert rest, line
                line, hit = f"{head} {os_name} {rest}{sep}{reason}", True
            out.append(line)
        assert hit, name_fragment
        return out

    return edit


CC7_HARDWARE = "cc7_canonical_node_stack_matches_the_cpu_reference_on_hardware"


class PlainAndColour(unittest.TestCase):
    def test_plain_and_coloured_fast_logs_pass_on_both_oses(self):
        for os_name, windows in (("linux", False), ("windows", True)):
            for colour in (False, True):
                log = build_log(fast_rows(os_name), colour=colour, windows=windows)
                code, out = check(st.verify_fast, log, os_name)
                self.assertEqual(code, 0, (os_name, colour, out))

    def test_plain_and_coloured_slow_logs_pass(self):
        for windows in (False, True):
            for colour in (False, True):
                code, out = check(st.verify_slow, build_log(slow_rows(), colour=colour, windows=windows))
                self.assertEqual(code, 0, out)

    def test_coloured_log_still_reports_a_real_unexpected_skip(self):
        rows = fast_rows("linux") + [("kinewright_core", "some::ordinary_test", "ignored, flaky")]
        code, out = check(st.verify_fast, build_log(rows, colour=True), "linux")
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


class PartialParse(unittest.TestCase):
    """Each mutation of a good log must be rejected as unparsable, never accepted."""

    def lines(self, windows=False):
        return build_lines(fast_rows("windows" if windows else "linux"), windows=windows)

    def assert_unparsable(self, lines, message=None):
        for verify in (st.verify_fast, st.verify_slow):
            code, out = check(verify, "\n".join(lines) + "\n")
            self.assertEqual(code, 1, (verify.__name__, out[:300]))
            self.assertIn("could not parse cargo test output", out)
            if message:
                self.assertIn(message, out)

    def index_of(self, lines, fragment):
        return next(i for i, line in enumerate(lines) if fragment in line)

    def test_native_ffmpeg_log_prefixes_on_result_lines_still_parse(self):
        # CI run 36536620605: FFmpeg's av_log writes `[swscaler @ 0x...] ` to stderr without
        # a newline, and libtest's next result line landed after it on the same line.
        for windows in (False, True):
            lines = self.lines(windows)
            at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
            lines[at] = "[swscaler @ 0x7f073d6b8740] " + lines[at]
            at = next(i for i, line in enumerate(lines) if line.startswith("test ") and "ignored" in line)
            lines[at] = "[h264 @ 0x55d1c0a1b2c0] [swscaler @ 0x7f073d6b8740] " + lines[at]
            # Windows prints the address without 0x, and a message can also trail the outcome.
            at = next(
                i for i, line in enumerate(lines)
                if line.startswith("test ") and line.endswith(" ok") and i > at
            )
            lines[at] = (
                "[swscaler @ 000001F4F555A800] " + lines[at]
                + "[swscaler @ 000001F4A934BE80] No accelerated colorspace conversion found from yuv420p to rgba64le."
            )
            os_name = "windows" if windows else "linux"
            code, out = check(st.verify_fast, "\n".join(lines) + "\n", os_name)
            self.assertEqual(code, 0, out)

    def test_foreign_text_that_is_not_an_ffmpeg_prefix_is_still_a_parse_failure(self):
        lines = self.lines()
        at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
        lines[at] = "garbage " + lines[at]
        self.assert_unparsable(lines, "summary says")

    def test_a_lost_header_is_rejected(self):
        for windows in (False, True):
            lines = self.lines(windows)
            del lines[self.index_of(lines, "kinewright_app-")]
            self.assert_unparsable(lines, "outside any")

    def test_a_lost_integration_header_is_rejected(self):
        lines = self.lines()
        del lines[self.index_of(lines, "tests/mcp_server.rs")]
        self.assert_unparsable(lines)

    def test_a_lost_header_with_an_extra_ignored_test_named_like_an_allowlisted_one_is_rejected(self):
        # The scenario from the review: the extra test hides in the previous binary's section.
        lines = self.lines()
        at = self.index_of(lines, "kinewright_media-")
        name = next(p for b, p in sorted(st.allowlist_for("linux")) if b == "kinewright_agent")
        del lines[at]
        lines.insert(at + 3, f"test {name} ... ignored, sneaked in")
        self.assert_unparsable(lines)

    def test_a_lost_header_and_the_previous_summary_is_rejected_by_the_counts(self):
        lines = self.lines()
        at = self.index_of(lines, "kinewright_media-")
        del lines[at]
        del lines[max(i for i, line in enumerate(lines[:at]) if line.startswith("test result:"))]
        self.assert_unparsable(lines)

    def test_an_orphan_result_before_the_first_header_is_rejected(self):
        lines = self.lines()
        self.assert_unparsable(["test stray::ignored_test ... ignored, flaky", *lines], "outside any")

    def test_an_orphan_result_between_sections_is_rejected(self):
        lines = self.lines()
        at = self.index_of(lines, "kinewright_app-")
        lines.insert(at, "test stray::ignored_test ... ignored, flaky")
        self.assert_unparsable(lines, "outside any")

    def test_a_dropped_test_line_is_a_count_mismatch(self):
        lines = self.lines()
        at = next(i for i, line in enumerate(lines) if line.startswith("test ") and "ignored" in line)
        del lines[at]
        self.assert_unparsable(lines)

    def test_a_wrong_summary_is_a_count_mismatch(self):
        lines = self.lines()
        at = next(i for i, line in enumerate(lines) if line.startswith("test result:"))
        lines[at] = re.sub(r"\d+ passed", "9999 passed", lines[at])
        self.assert_unparsable(lines, "summary says")

    def test_a_section_with_no_summary_is_rejected(self):
        lines = self.lines()
        del lines[max(i for i, line in enumerate(lines) if line.startswith("test result:"))]
        self.assert_unparsable(lines, "no `test result:` summary")

    def test_an_empty_section_merged_into_the_next_by_a_lost_summary_and_header_is_rejected(self):
        # Review 3: an empty section loses its summary, the next binary loses its header, and
        # the second `running N tests` overwrote the first, so both tiers accepted the merge.
        lines = self.lines()
        at = self.index_of(lines, "kinewright_media-")
        merged = [header("au3_fixtures", False, False), "", "running 0 tests", ""]
        lines[at:at + 1] = merged
        self.assert_unparsable(lines, "second `running N tests`")

    def test_unrelated_text_starting_with_running_is_not_a_header(self):
        lines = ["Running kernel seems to be up-to-date.", *self.lines()]
        code, out = check(st.verify_fast, "\n".join(lines) + "\n", "linux")
        self.assertEqual(code, 0, out)

    def test_doctest_sections_parse(self):
        rows = fast_rows("linux")
        lines = build_lines(rows)
        lines += build_lines([(DOCTEST, "crates/x.rs - item (line 3)", "ok")])
        code, out = check(st.verify_fast, "\n".join(lines) + "\n", "linux")
        self.assertEqual(code, 0, out)


class PerOsObservation(unittest.TestCase):
    def test_an_entry_listed_any_but_not_ignored_on_windows_fails(self):
        # Astra's case: a Linux-only cfg_attr ignore. The entry is `any`, so Windows must see it ignored.
        rows = [
            (b, p, "ok") if p.endswith(CC7_HARDWARE) else (b, p, t) for b, p, t in fast_rows("windows")
        ]
        code, out = check(st.verify_fast, build_log(rows, windows=True), "windows")
        self.assertEqual(code, 1)
        self.assertIn(f"allowlisted but not observed as ignored on windows: kinewright_media cc7_fixtures::{CC7_HARDWARE}", out)

    def test_an_entry_missing_from_the_run_fails(self):
        rows = [r for r in fast_rows("linux") if not r[1].endswith(CC7_HARDWARE)]
        code, out = check(st.verify_fast, build_log(rows), "linux")
        self.assertEqual(code, 1)
        self.assertIn("allowlisted but not observed as ignored on linux", out)

    def test_a_linux_entry_is_not_required_on_windows_but_is_unexpected_there(self):
        with allowlist(set_os(CC7_HARDWARE, "linux")):
            windows_without = [r for r in fast_rows("windows") if not r[1].endswith(CC7_HARDWARE)]
            code, out = check(st.verify_fast, build_log(windows_without, windows=True), "windows")
            self.assertEqual(code, 0, out)
            windows_with = fast_rows("windows") + [("kinewright_media", f"cc7_fixtures::{CC7_HARDWARE}", "ignored, x")]
            code, out = check(st.verify_fast, build_log(windows_with, windows=True), "windows")
            self.assertEqual(code, 1)
            self.assertIn("unexpected skip on windows", out)
            code, out = check(st.verify_fast, build_log(fast_rows("linux")), "linux")
            self.assertEqual(code, 0, out)

    def test_a_fabricated_or_stale_doctest_entry_is_never_observed(self):
        entry = f"{DOCTEST} {SCRATCH_DOCTEST[0]} {SCRATCH_DOCTEST[1]} any on-demand  # scratch"
        with allowlist(lambda lines: [*lines, entry]):
            self.assertIn((DOCTEST, SCRATCH_DOCTEST[0]), st.allowlist_for("linux"))
            real = fast_rows("linux") + [(DOCTEST, SCRATCH_DOCTEST[0], "ignored")]
            code, out = check(st.verify_fast, build_log(real), "linux")
            self.assertEqual(code, 0, out)
            missing = [r for r in fast_rows("linux") if r[0] != DOCTEST]
            code, out = check(st.verify_fast, build_log(missing), "linux")
            self.assertEqual(code, 1)
            self.assertIn("allowlisted but not observed as ignored on linux: " + DOCTEST, out)
            forged = [r for r in fast_rows("linux") if r[0] != DOCTEST] + [(DOCTEST, "crates/kinewright-core/src/lib.rs - Scratch::ghost (line 291)", "ignored")]
            code, out = check(st.verify_fast, build_log(forged), "linux")
            self.assertEqual(code, 1)
            self.assertIn("Scratch::probe", out)
            self.assertIn("unexpected skip", out)


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

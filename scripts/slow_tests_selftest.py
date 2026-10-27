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
from unittest import mock

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
    rows = [
        (b, p, f"ignored, {st.REASON}" if scope in ("any", os_name, os_name + "-only") else "ok")
        for b, p, _, scope in st.read_manifest()
        if not scope.endswith("-only") or scope == os_name + "-only"
    ]
    applies = st.allowlist_for(os_name)
    rows += [(b, p, "ignored, allowlisted") for b, p in sorted(applies)]
    if extra_ok:
        rows += [("kinewright_core", "ordinary::passes", "ok"), ("mcp_server", "ordinary::also_passes", "ok")]
    return rows


def slow_rows():
    return [(b, p, "ok") for b, p, _, scope in st.read_manifest()
            if not scope.endswith("-only") or scope == st.host_os() + "-only"]


def check(verify, *args) -> tuple[int, str]:
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = verify(*args)
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
R79_PROBE_LOG = """test decode::tests::still_probe_failure_rides_backend_unclassified ... [png @ 0x7f9d54000e80] Invalid PNG signature 0x6E6F742061207374.
[image2 @ 0x7f9d5405ba80] Could not find codec parameters for stream 0 (Video: png, none): unspecified size
Consider increasing the value for the 'analyzeduration' (0) and 'probesize' (5000000) options
[png @ 0x7f9d54019f40] Invalid PNG signature 0x6E6F742061207374.
[image2 @ 0x7f9d54001580] Could not find codec parameters for stream 0 (Video: png, none): unspecified size
Consider increasing the value for the 'analyzeduration' (0) and 'probesize' (5000000) options
ok"""


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
                with mock.patch.object(st.sys, "platform", "win32" if windows else "linux"):
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

    def test_an_ffmpeg_fragment_before_the_outcome_keeps_the_outcome(self):
        # Libtest writes the name and the outcome separately, so a fragment can land between them.
        for windows in (False, True):
            lines = self.lines(windows)
            at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
            lines[at] = lines[at][: -len("ok")] + "[swscaler @ 0x1] ok[swscaler @ 0x2] trailing message"
            at = next(i for i, line in enumerate(lines) if line.startswith("test ") and "ignored" in line)
            name, _, outcome = lines[at].partition(" ... ")
            lines[at] = f"{name} ... [h264 @ 000001F4A934BE80] {outcome}"
            code, out = check(st.verify_fast, "\n".join(lines) + "\n", "windows" if windows else "linux")
            self.assertEqual(code, 0, out)

    def test_a_result_split_by_a_whole_ffmpeg_message_is_rejoined(self):
        # pf1 R43 gate log: a complete message (newline included) landed after `test x ... `,
        # and libtest's `ok` followed on a line of its own, sometimes after further messages.
        for windows in (False, True):
            lines = self.lines(windows)
            at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
            lines[at: at + 1] = [
                lines[at][: -len("ok")] + "[swscaler @ 0x1] [swscaler @ 0x2] No accelerated colorspace conversion.",
                "[swscaler @ 0x3] No accelerated colorspace conversion.",
                "ok[swscaler @ 000001F4A934BE80] No accelerated colorspace conversion.",
            ]
            at = next(i for i, line in enumerate(lines) if line.startswith("test ") and "ignored" in line)
            name, _, outcome = lines[at].partition(" ... ")
            lines[at: at + 1] = [f"{name} ... [h264 @ 000001F4A934BE80] late frame", outcome]
            code, out = check(st.verify_fast, "\n".join(lines) + "\n", "windows" if windows else "linux")
            self.assertEqual(code, 0, out)

    def test_r79_literal_multiline_probe_log_keeps_the_result(self):
        for windows in (False, True):
            rows = fast_rows("windows" if windows else "linux")
            rows.append(("kinewright_media", "decode::tests::still_probe_failure_rides_backend_unclassified", "ok"))
            lines = build_lines(rows, windows=windows)
            at = next(i for i, line in enumerate(lines) if line.startswith("test decode::tests::still_probe_failure"))
            lines[at: at + 1] = R79_PROBE_LOG.splitlines()
            code, out = check(st.verify_fast, "\n".join(lines) + "\n", "windows" if windows else "linux")
            self.assertEqual(code, 0, out)

    def test_r79_continuations_require_the_exact_message_and_immediate_context(self):
        advice = R79_PROBE_LOG.splitlines()[2]
        for text in (
            R79_PROBE_LOG.replace(advice, "arbitrary text"),
            R79_PROBE_LOG.replace("Could not find codec parameters", "unrelated FFmpeg message"),
            R79_PROBE_LOG.replace(advice, advice + " trailing garbage"),
            R79_PROBE_LOG.replace(advice, advice + "\n" + advice),
            R79_PROBE_LOG.replace(advice, "[png @ 0x1] other message\n" + advice),
        ):
            with self.subTest(text=text):
                lines = self.lines()
                at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
                lines[at: at + 1] = text.splitlines()
                self.assert_unparsable(lines, "summary says")

    def test_a_result_split_with_the_message_prefix_before_test_is_rejoined(self):
        # pf1 R47 gate log: the prefix landed before `test`, the message after " ... ", and `ok`
        # on the next line.
        for windows in (False, True):
            lines = self.lines(windows)
            at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
            lines[at: at + 1] = [
                "[swscaler @ 0x73c2e0b48d80] " + lines[at][: -len("ok")]
                + "No accelerated colorspace conversion found from yuv420p to rgba64le.",
                "ok",
            ]
            code, out = check(st.verify_fast, "\n".join(lines) + "\n", "windows" if windows else "linux")
            self.assertEqual(code, 0, out)

    def test_a_message_glued_to_ok_without_its_prefix_is_cut(self):
        # pf1 R47 media log: `[swscaler @ A] test x ... [swscaler @ B] okNo accelerated ...`.
        self.assertEqual(
            st.strip_ffmpeg_log("[swscaler @ 0x1] test a::b ... [swscaler @ 0x2] okNo accelerated conversion."),
            "test a::b ... ok",
        )
        self.assertEqual(st.strip_ffmpeg_log("test a::b ... FAILED[x @ 0x1]"), "test a::b ... FAILED")
        # Without any fragment on the line nothing is cut, so `okay` stays a non-outcome.
        self.assertEqual(st.strip_ffmpeg_log("test a::b ... okay"), "test a::b ... okay")
        # review-c4: a fragment elsewhere on the line, or lower-case text after the outcome,
        # does not license the cut.
        self.assertEqual(st.strip_ffmpeg_log("[x @ 0x1] test a::b ... okay"), "test a::b ... okay")
        self.assertEqual(st.strip_ffmpeg_log("test a::b ... [x @ 0x1] okay"), "test a::b ... okay")
        self.assertEqual(
            st.strip_ffmpeg_log("test a::b ... [x @ 0x1] okay FAILED"), "test a::b ... okay FAILED"
        )
        # An ignore reason is never cut without a fragment in it.
        self.assertEqual(
            st.strip_ffmpeg_log("[x @ 0x1] test a::b ... ignored, slow tier"), "test a::b ... ignored, slow tier"
        )
        # Windows run 39003986680: FFmpeg's lowercase deprecation warning glued to `ok`.
        self.assertEqual(
            st.strip_ffmpeg_log(
                "test a::b ... [swscaler @ 0000028DABD9D300] okdeprecated pixel format used, make sure you did set range correctly"
            ),
            "test a::b ... ok",
        )
        # Linux run 40486022371: h264's lowercase reference-list warning glued to `ok`.
        self.assertEqual(
            st.strip_ffmpeg_log("test a::b ... [h264 @ 0x7f75681b1440] okmmco: unref short failure"),
            "test a::b ... ok",
        )
        # Windows run 39009547685: both prefixes before `test`, the message glued to `ok`.
        self.assertEqual(
            st.strip_ffmpeg_log(
                "[swscaler @ 000001CAD4F80880] [swscaler @ 000001CA56845FC0] test a::b ... okNo accelerated colorspace conversion found from yuv420p to rgba64le."
            ),
            "test a::b ... ok",
        )

    def test_a_result_line_with_no_ffmpeg_fragment_and_no_outcome_is_not_joined(self):
        lines = self.lines()
        at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
        lines[at: at + 1] = [lines[at][: -len("ok")] + "some message", "ok"]
        self.assert_unparsable(lines, "summary says")

    def test_foreign_text_before_a_fragment_is_not_rejoined(self):
        # review-c4: `test x ... unexpected [swscaler @ 0x1] message` then `ok` must not join.
        lines = self.lines()
        at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
        lines[at: at + 1] = [lines[at][: -len("ok")] + "unexpected [swscaler @ 0x1] message", "ok"]
        self.assert_unparsable(lines, "summary says")
        lines = self.lines()
        at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
        lines[at] = "[x @ 0x1] " + lines[at][: -len("ok")] + "okay"
        self.assert_unparsable(lines, "summary says")

    def test_a_split_result_whose_next_line_is_not_an_outcome_still_fails(self):
        lines = self.lines()
        at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
        lines[at: at + 1] = [lines[at][: -len("ok")] + "[swscaler @ 0x1] message", "garbage ok"]
        self.assert_unparsable(lines, "summary says")
        # ... and a split result never borrows the outcome of the next test line.
        lines = self.lines()
        at = next(i for i, line in enumerate(lines) if line.startswith("test ") and line.endswith(" ok"))
        lines[at] = lines[at][: -len("ok")] + "[swscaler @ 0x1] message"
        self.assert_unparsable(lines, "summary says")

    def test_ffmpeg_stripping_leaves_names_and_outcomes_intact(self):
        self.assertEqual(st.strip_ffmpeg_log("test a::b ... [x @ 0x1] ok"), "test a::b ... ok")
        self.assertEqual(st.strip_ffmpeg_log("test a::b ... FAILED[x @ 0x1] msg"), "test a::b ... FAILED")
        # A fragment inside the name is not stripped, so the row no longer matches its allowlist entry.
        self.assertEqual(st.strip_ffmpeg_log("test a[x @ 0x1] b ... ok"), "test a[x @ 0x1] b ... ok")
        # Nothing but a fragment where the outcome belongs: the outcome is not invented.
        self.assertEqual(st.strip_ffmpeg_log("test a::b ... [x @ 0x1] "), "test a::b ... ")

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
    def test_windows_slow_tests_must_pass_on_linux_and_be_skipped_on_windows(self):
        scoped = [(b, p) for b, p, _, scope in st.read_manifest() if scope == "windows"]
        self.assertEqual(len(scoped), 34)
        for os_name in ("linux", "windows"):
            for binary, path in scoped:
                for replacement in (None, "ignored", f"ignored, {st.REASON}", "ok"):
                    if replacement == ("ok" if os_name == "linux" else f"ignored, {st.REASON}"):
                        continue
                    with self.subTest(os=os_name, path=path, replacement=replacement):
                        rows = [r for r in fast_rows(os_name) if r[:2] != (binary, path)]
                        if replacement is not None:
                            rows.append((binary, path, replacement))
                        code, out = check(st.verify_fast, build_log(rows), os_name)
                        self.assertEqual(code, 1, out)
                rows = [r for r in fast_rows(os_name) if r[:2] != (binary, path)]
                rows.append(("wrong_binary", path, "ok" if os_name == "linux" else f"ignored, {st.REASON}"))
                code, out = check(st.verify_fast, build_log(rows), os_name)
                self.assertEqual(code, 1, out)

    def test_both_slow_tiers_require_every_windows_scoped_test_to_pass(self):
        for windows in (False, True):
            for binary, path, _, scope in st.read_manifest():
                if scope != "windows":
                    continue
                for replacement in (None, "ignored"):
                    with self.subTest(windows=windows, path=path, replacement=replacement):
                        rows = [r for r in slow_rows() if r[:2] != (binary, path)]
                        if replacement is not None:
                            rows.append((binary, path, replacement))
                        code, out = check(st.verify_slow, build_log(rows, windows=windows))
                        self.assertEqual(code, 1, out)

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


class ManifestLint(unittest.TestCase):
    def test_manifest_scope_must_match_the_source_marker_in_both_directions(self):
        for scope, condition in (("any", 'not(feature = "slow-tests")'),
                                 ("windows", 'all(windows, not(feature = "slow-tests"))'),
                                 ("linux", 'all(target_os = "linux", not(feature = "slow-tests"))')):
            marker = f'#[cfg_attr({condition}, ignore = "{st.REASON}")]'
            for listed_scope in ("any", "windows", "linux"):
                with self.subTest(marker_scope=scope, manifest_scope=listed_scope):
                    code, out = self.lint_fixture(listed_scope, marker)
                    self.assertEqual(code, 0 if listed_scope == scope else 1, out)
                    if code:
                        self.assertIn(f"not marked slow for {listed_scope}", out)
                        self.assertIn(f"marked slow for {scope}", out)

    def test_windows_scoped_entry_with_plain_ignore_fails_lint(self):
        code, out = self.lint_fixture("windows", "#[ignore]")
        self.assertEqual(code, 1, out)
        self.assertIn("in the manifest but not marked slow for windows", out)

    def lint_fixture(self, scope, marker):
        with tempfile.TemporaryDirectory(prefix="kinewright-ci-lint-") as directory:
            root = Path(directory)
            source = root / "crates" / "witness" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text(f"#[test]\n{marker}\nfn witness() {{}}\n", encoding="utf-8")
            (source.parent.parent / "Cargo.toml").write_text('[features]\nslow-tests = []\n', encoding="utf-8")
            manifest = root / "ci" / "slow-tests.txt"
            manifest.parent.mkdir()
            manifest.write_text(f"witness witness crates/witness/src/lib.rs {scope}\n", encoding="utf-8")
            ignored = manifest.with_name("ignored-tests.txt")
            ignored.write_text("", encoding="utf-8")
            with mock.patch.multiple(st, ROOT=root, MANIFEST=manifest, IGNORED=ignored):
                return check(st.lint)


class RunCargoTests(unittest.TestCase):
    def test_slow_runner_selects_the_whole_manifest_with_features_on_both_oses(self):
        for platform in ("linux", "win32"):
            with mock.patch.object(st.sys, "platform", platform), mock.patch.object(
                st, "run_cargo", return_value=(0, build_log(slow_rows()))
            ) as run:
                code, out = check(st.main, ["slow_tests.py", "slow"])
                self.assertEqual(code, 0, out)
                args = run.call_args.args[0]
                self.assertIn("kinewright-media/slow-tests", args[args.index("--features") + 1])
                self.assertIn("--show-output", args)
                self.assertEqual(args[args.index("--exact") + 1:], [p for _, p, _, _ in st.read_manifest()])

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


class NativeGuardianAccounting(unittest.TestCase):
    def test_platform_only_fixtures_match_compiled_fast_skips(self):
        windows_test = "s4_c2_windows_claim_ignores_localappdata"
        linux_test = "s4_c2_blocked_guardian_exits_and_manager_kills"
        for os_name in ("linux", "windows"):
            rows = fast_rows(os_name)
            present = {path: result for binary, path, result in rows if binary == "ja_s4_guardian"}
            local, foreign = (linux_test, windows_test) if os_name == "linux" else (windows_test, linux_test)
            self.assertEqual(present[local], f"ignored, {st.REASON}")
            self.assertNotIn(foreign, present)
            code, out = check(st.verify_fast, build_log(rows, windows=os_name == "windows"), os_name)
            self.assertEqual(code, 0, out)

    def test_hosted_deletion_signal_markers_between_names_and_outcomes(self):
        reason = "kernel_deletion_signal_unavailable: live_nlink=2, deleted_nlink=2"
        with mock.patch.object(st.sys, "platform", "linux"):
            rows = slow_rows()
            guardians = [path for binary, path, _, scope in st.read_manifest()
                         if binary == "ja_s4_guardian" and scope == "linux-only"]
            for glued in (False, True):
                log = build_log(rows)
                for path in guardians:
                    marker = f"C2_NATIVE_NOT_RUN test={path} reason={reason}"
                    split = "" if glued else "\n"
                    log = log.replace(f"test {path} ... ok", f"test {path} ... {split}{marker}\nok")
                code, out = check(st.verify_slow, log)
                self.assertEqual(code, 0, out)
                self.assertIn(f"{len(guardians)} equipped-session NOT RUN", out)
                code, out = check(st.verify_slow, log.replace("reason=", "invalid=", 1))
                self.assertEqual(code, 1, out)

    def test_hosted_fifteen_skips_with_four_split_results(self):
        names = [
            "s4_precommit_cannot_execute", "s4_cancel_tree_and_retire_once",
            "s4_guardian_death_and_observation_loss", "s4_real_host_death_skips_grace",
            "s4_lost_startup_acknowledgments", "s4_retained_inode_deletion_signal",
            "s4_reaped_guardian_and_identity_are_required", "s4_deleted_cgroup_zombies_still_block_retirement",
            "s4_guardian_subreaps_real_orphans", "s4_startup_deadline_kills_a_contained_shim",
            "s4_c2_migration_outside_service_refused", "s4_c2_migration_to_service_root_refused",
            "s4_c2_runtime_dir_masked", "s4_c2_launch_refuses_without_nsdelegate",
            "s4_c2_blocked_guardian_exits_and_manager_kills",
        ]
        entries = [("ja_s4_guardian", name, "source", "linux-only") for name in names]
        log = build_log([("ja_s4_guardian", name, "ok") for name in names], filtered=4)
        for index, name in enumerate(names):
            marker = f"C2_NATIVE_NOT_RUN test={name} reason=kernel_deletion_signal_unavailable: live_nlink=2, deleted_nlink=2"
            old = f"test {name} ... ok"
            replacement = f"test {name} ... {marker}\nok" if index < 4 else f"{marker}\n{old}"
            log = log.replace(old, replacement)
        with mock.patch.object(st, "read_manifest", return_value=entries), mock.patch.object(st.sys, "platform", "linux"):
            code, out = check(st.verify_slow, log)
        self.assertEqual(code, 0, out)
        self.assertIn("0 passed; 15 equipped-session NOT RUN", out)

    def test_captured_marker_output_counts_not_run(self):
        with mock.patch.object(st.sys, "platform", "linux"):
            log = build_log(slow_rows())
            name = "s4_c2_blocked_guardian_exits_and_manager_kills"
            marker = f"\nC2_NATIVE_NOT_RUN test={name} reason=kernel_deletion_signal_unavailable: live_nlink=2, deleted_nlink=2\n"
            at = log.index("test result:", log.index("ja_s4_guardian-"))
            log = log[:at] + f"successes:\n\n---- {name} stdout ----\n{marker}test captured_noise ... ok\nRunning tests/fake.rs (target/debug/deps/fake-0123456789abcdef)\nsuccesses:\n    {name}\n\n" + log[at:]
            code, out = check(st.verify_slow, log)
            self.assertEqual(code, 0, out)
            self.assertIn("1 equipped-session NOT RUN", out)


if __name__ == "__main__":
    unittest.main()

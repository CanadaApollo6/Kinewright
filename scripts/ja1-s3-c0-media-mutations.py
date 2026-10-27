#!/usr/bin/env python3
"""Run focused C0 media security mutations after the lead assigns the gate lane."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "crates/kinewright-media/src/owned_io.rs"
PREFIX = "owned_io::tests::"
PROJECT_CASES = {"production-certified-through-use", "production-incomplete-stream-table"}


def replace_once(source, old, new):
    if source.count(old) != 1:
        raise RuntimeError(f"expected one mutation anchor, found {source.count(old)}: {old!r}")
    return source.replace(old, new, 1)


def scoped(source, start, end, old, new):
    left = source.index(start)
    right = source.index(end, left)
    return source[:left] + replace_once(source[left:right], old, new) + source[right:]


def chunk_guard(source):
    return replace_once(source, "if digest.finalize()\n", "if false && digest.finalize()\n")


def short_read_guard(source):
    return replace_once(source, '''                Ok(0) => {
                    return self.fail(OwnedIoError::Damaged(
                        "premature short descriptor read".into(),
                    ));
                }''', '                Ok(0) => break,')


def size_guard(source):
    return replace_once(source,
        "if metadata.len() != self.source.identity.source_length {",
        "if false && metadata.len() != self.source.identity.source_length {")


def verified_only_guard(source):
    source = replace_once(source, "        self.slots[slot_index].index = Some(index);\n", "")
    return replace_once(source, "        slot.index = None;", "        slot.index = Some(index);")


def avio_routing_guard(source):
    return replace_once(source, "            cursor.read(output)", '''            let count = super::positioned_read(&cursor.source.file, output, cursor.position).map_err(super::unavailable)?;
            cursor.position += count as u64;
            Ok(count)''')


def output_guard(source):
    return scoped(source, "impl CertifiedInput {", "    fn packet(",
                  "        self.avio.check()", "        Ok(())")


def lifetime_guard(source):
    return replace_once(source, "            _lifetime: lifetime,",
                        "            _lifetime: { drop(lifetime); Arc::new(()) },")


def cursor_charge_guard(source):
    return replace_once(source, "            _charge: charge,",
                        "            _charge: { drop(charge); Arc::new(()) },")


def decoder_charge_guard(source):
    return scoped(source, "    pub fn open_video(", "    pub fn open_audio(",
        "let charge = (self.0.reserve)(bound.bytes).map_err(OwnedIoError::Capacity)?;",
        "let charge: LifetimeGuard = Arc::new(());")


def frame_charge_guard(source):
    return scoped(source, "impl CertifiedVideoDecoder {", "pub struct CertifiedAudioDecoder",
        '''            let frame_charge =
                (self.source.0.reserve)(self.bound.frame_bytes).map_err(OwnedIoError::Capacity)?;''',
        '            let frame_charge: LifetimeGuard = Arc::new(());')


def selected_video_guard(source):
    return replace_once(source, "        self.check_selected(stream_index, true)?;\n", "")


def selected_audio_guard(source):
    return replace_once(source, "        self.check_selected(stream_index, false)?;\n", "")


def nested_protocol_guard(source):
    return replace_once(source, 'let protocols = c"none";', 'let protocols = c"file";')


def nested_format_guard(source):
    return scoped(source, "let format_option = ffi::av_dict_set(", "if protocol_option < 0",
                  "formats.as_ptr()", "ptr::null()")


def incomplete_stream_table_guard(source):
    return replace_once(source,
        "if input.format.nb_streams() == 0 || bridge::dynamic_stream_table(&input.format) {",
        "if false {")


def non_av_probe_guard(source):
    return replace_once(source,
        'return Err(decoder_error("non-audio/video stream kind unsupported"));',
        'bounds.push(None);')


def probe_stream_limit_guard(source):
    return replace_once(source,
        'i32::try_from(bounds.len()).expect("bounded reserved stream count");',
        'i32::try_from(MAX_CERTIFIED_STREAMS).expect("bounded reserved stream count");')


def secondary_callback_guard(source):
    return scoped(source, '    unsafe extern \"C\" fn deny_open(', '    #[cfg(test)]\n    pub(super) fn callback_read',
        '''        let _ = cursor.fail::<()>(OwnedIoError::Unavailable(
            "secondary input open denied".into(),
        ));
        ffi::AVERROR(ffmpeg::error::EACCES)''',
        '''        unsafe { ffi::avio_open2(_io, _url, _flags, &raw const (*format).interrupt_callback, _options) }''')


def selected_audio_probe_index_guard(source):
    return replace_once(source, "(audio_index, ffmpeg::media::Type::Audio),",
                        "(None, ffmpeg::media::Type::Audio),")


def selected_audio_probe_kind_guard(source):
    return replace_once(source, "if stream.parameters().medium() != medium {",
                        "if medium == ffmpeg::media::Type::Video && stream.parameters().medium() != medium {")


MUTATIONS = [
    ("chunk-sha256", "s3_certified_cache_is_private_and_evict_refetch_verifies_again", chunk_guard),
    ("tamper-after-open-video", "s3_certified_uncached_video_seek_tamper_after_open_is_latched", chunk_guard),
    ("tamper-after-open-audio", "s3_certified_uncached_audio_seek_tamper_after_open_is_latched", chunk_guard),
    ("premature-short-read", "s3_certified_short_read_with_unchanged_descriptor_length_refuses", short_read_guard),
    ("size-change-cache-hit", "s3_certified_size_changes_refuse_cache_hits_seek_and_size_queries", size_guard),
    ("size-change-during-fill", "s3_certified_size_change_during_fill_refuses_before_publication", size_guard),
    ("verified-only-publication", "s3_certified_avio_exposes_zero_unverified_scratch_bytes_at_barrier", verified_only_guard),
    ("certified-avio-routing", "s3_certified_short_read_with_unchanged_descriptor_length_refuses", avio_routing_guard),
    ("latched-output-rejection", "s3_certified_latched_error_rejects_already_buffered_video_and_audio", output_guard),
    ("source-pin-lifetime", "s3_certified_cursor_capacity_and_decoder_pin_charge_lifetime", lifetime_guard),
    ("cursor-charge-lifetime", "s3_certified_cursor_charge_survives_source_until_final_cursor_close", cursor_charge_guard),
    ("decoder-allocation-preflight", "s3_certified_decoder_and_output_frame_preflight_refuse_before_use", decoder_charge_guard),
    ("frame-allocation-preflight", "s3_certified_decoder_and_output_frame_preflight_refuse_before_use", frame_charge_guard),
    ("selected-video-interpretation", "s3_certified_selected_streams_cannot_be_opened_or_widened", selected_video_guard),
    ("selected-audio-interpretation", "s3_certified_selected_streams_cannot_be_opened_or_widened", selected_audio_guard),
    ("nested-protocol-inheritance", "s3_certified_nested_protocol_policy_is_inherited_by_concat", nested_protocol_guard),
    ("nested-format-policy", "s3_certified_nested_format_policy_precedes_concat_and_hls_opens", nested_format_guard),
    ("hls-format-policy", "s3_certified_hls_format_policy_precedes_secondary_open", nested_format_guard),
    ("secondary-open-callback", "s3_certified_secondary_open_is_denied_and_latched", secondary_callback_guard),
    ("selected-audio-probe-index", "s3_certified_probe_rejects_missing_and_wrong_kind_selected_streams", selected_audio_probe_index_guard),
    ("selected-audio-probe-kind", "s3_certified_probe_rejects_missing_and_wrong_kind_selected_streams", selected_audio_probe_kind_guard),
    ("incomplete-stream-table", "s3_certified_mpegps_refuses_incomplete_table_before_probe", incomplete_stream_table_guard),
    ("non-av-probe-kind", "s3_certified_subtitle_refuses_unsupported_kind_before_probe", non_av_probe_guard),
    ("probe-stream-limit", "s3_certified_probe_stream_limit_refuses_before_new_stream_allocation", probe_stream_limit_guard),
    ("production-incomplete-stream-table", "s3_owned_import_refuses_dynamic_mpegps_before_probe", incomplete_stream_table_guard),
    ("production-certified-through-use", "s3_certified_input_through_use", chunk_guard),
]

RED_MARKERS = {
    "chunk-sha256": "C0_DAMAGED_INPUT_REFUSED",
    "tamper-after-open-video": "C0_DAMAGED_INPUT_REFUSED",
    "tamper-after-open-audio": "C0_DAMAGED_INPUT_REFUSED",
    "premature-short-read": "C0_SHORT_READ_REFUSED",
    "size-change-cache-hit": "C0_DAMAGED_INPUT_REFUSED",
    "size-change-during-fill": "C0_SIZE_DURING_FILL_REFUSED",
    "verified-only-publication": "C0_VERIFIED_ONLY_PUBLICATION",
    "certified-avio-routing": "C0_SHORT_READ_REFUSED",
    "latched-output-rejection": "C0_DAMAGED_INPUT_REFUSED",
    "source-pin-lifetime": "C0_SOURCE_AUTHORITY_RETAINED",
    "cursor-charge-lifetime": "C0_CURSOR_CHARGE_RETAINED",
    "decoder-allocation-preflight": "C0_DECODER_ALLOCATION_PREFLIGHT",
    "frame-allocation-preflight": "C0_FRAME_ALLOCATION_PREFLIGHT",
    "selected-video-interpretation": "C0_SELECTED_VIDEO_REFUSED",
    "selected-audio-interpretation": "C0_SELECTED_AUDIO_REFUSED",
    "nested-protocol-inheritance": "C0_PROTOCOL_OUTPUT_ZERO",
    "nested-format-policy": "C0_FORMAT_OUTPUT_ZERO",
    "hls-format-policy": "C0_HLS_NO_SECONDARY_ATTEMPT",
    "secondary-open-callback": "C0_SECONDARY_OPEN_DENIED",
    "selected-audio-probe-index": "C0_SELECTED_STREAM_TYPED_REFUSAL",
    "selected-audio-probe-kind": "C0_SELECTED_STREAM_TYPED_REFUSAL",
    "incomplete-stream-table": "C0_INCOMPLETE_PROBE_NOT_ENTERED",
    "non-av-probe-kind": "C0_NON_AV_PROBE_NOT_ENTERED",
    "probe-stream-limit": "C0_PROBE_NEW_STREAM_ALLOCATION_REFUSED",
    "production-incomplete-stream-table": "C0_PRODUCTION_INCOMPLETE_PROBE_NOT_ENTERED",
    "production-certified-through-use": "no accepted video output after uncached payload tamper",
}


def full_selector(name, selector):
    return selector if name in PROJECT_CASES else PREFIX + selector


def available_memory():
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            if int(line.split()[1]) < 6 * 1024 * 1024:
                raise RuntimeError("less than 6 GiB available, gate not started")
            return
    raise RuntimeError("cannot establish available memory")


def gate(name, selector, log_path):
    available_memory()
    target = ("-p kinewright-project --features test-util --test ja_s3_owned_inputs"
              if name in PROJECT_CASES else "-p kinewright-media --lib")
    command = f'''set -e
available_kb=$(awk '/MemAvailable:/ {{print $2}}' /proc/meminfo)
if [ "$available_kb" -lt 6291456 ]; then echo "REFUSED less than 6 GiB available"; exit 2; fi
source scripts/setup-ffmpeg.sh
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo test {target} {shlex.quote(selector)} -- --exact --nocapture --test-threads=1
'''
    process = subprocess.Popen(["bash", "-c", command], cwd=ROOT,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               start_new_session=True)
    try:
        output = process.communicate(timeout=600)[0]
    except BaseException:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            output = process.communicate(timeout=10)[0]
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            output = process.communicate()[0]
        with gzip.open(log_path, "wb") as log:
            log.write(output)
        raise
    with gzip.open(log_path, "wb") as log:
        log.write(output)
    return process.returncode, output.decode(errors="replace")


RUST_TOKENS = re.compile(
    r'//[^\n]*|/\*.*?\*/|(?:br|cr|r)(?P<hashes>\#*)".*?"(?P=hashes)|'
    r'(?:b|c)?"(?:\\.|[^"\\])*"|'
    r"(?:b)?'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^'\\\r\n])'|"
    r'[A-Za-z_][A-Za-z_0-9]*|[^\s]',
    re.DOTALL,
)


def assertion_sites(source, marker):
    """Find marker-bearing assertion macro starts, including shared helpers."""
    tokens = [token for token in RUST_TOKENS.finditer(source)
              if not token.group().startswith(("//", "/*"))]
    sites = set()
    closing = {"(": ")", "[": "]", "{": "}"}
    for index, token in enumerate(tokens[:-2]):
        if (token.group() not in {"assert", "assert_eq", "assert_ne", "panic"}
                or tokens[index + 1].group() != "!"
                or tokens[index + 2].group() not in closing):
            continue
        stack = []
        has_marker = False
        for inner in tokens[index + 2:]:
            value = inner.group()
            if value in closing:
                stack.append(closing[value])
            elif value in closing.values():
                if not stack or stack.pop() != value:
                    raise RuntimeError("unbalanced witness assertion macro")
                if not stack:
                    if has_marker:
                        sites.add(source.count("\n", 0, token.start()) + 1)
                    break
            elif '"' in value and marker in value:
                has_marker = True
    return sites


def assertion_red(name, selector, status, output):
    if status != 101 or "error[E" in output or "could not compile" in output:
        raise RuntimeError("mutation did not produce a runtime test assertion failure")
    summaries = re.findall(r"^test result: [^\n]*", output, re.MULTILINE)
    if len(summaries) != 1 or not summaries[0].startswith(
            "test result: FAILED. 0 passed; 1 failed; 0 ignored;"):
        raise RuntimeError("expected exactly one focused failing witness")
    if re.search(r"called\s+[`'\"]?(?:Result|Option)::(?:unwrap|expect)\(\)", output):
        raise RuntimeError("fixture/setup unwrap or expect panic is not a protection assertion")
    marker = RED_MARKERS[name]
    panics = list(re.finditer(
        r"^thread '([^'\r\n]+)'(?: \(\d+\))? panicked at ([^\r\n]+):(\d+):(\d+):\s*$",
        output, re.MULTILINE,
    ))
    if len(panics) != 1 or len(re.findall(r"\bpanicked at\b", output)) != 1:
        raise RuntimeError("expected exactly one attributable test-thread panic")
    panic = panics[0]
    if panic[1] != selector:
        raise RuntimeError(f"panic thread {panic[1]!r} does not match selector {selector!r}")
    relative = ("crates/kinewright-project/tests/ja_s3_owned_inputs.rs" if name in PROJECT_CASES
                else "crates/kinewright-media/src/owned_io.rs")
    reported = panic[2].replace("\\", "/")
    if reported != relative and reported != str(ROOT / relative):
        raise RuntimeError(f"panic came from unexpected witness file {reported!r}")
    sites = assertion_sites((ROOT / relative).read_text(), marker)
    if not sites or int(panic[3]) not in sites:
        raise RuntimeError(f"panic line is not a source assertion macro for {marker!r}: {sorted(sites)}")
    if marker not in output[panic.end():]:
        raise RuntimeError(f"panic did not report protection assertion {marker!r}")
    return {"thread": selector, "file": relative, "line": int(panic[3]),
            "marker_macro_lines": sorted(sites),
            "witness_source_sha256": hashlib.sha256((ROOT / relative).read_bytes()).hexdigest()}


def green(status, output, count=1):
    if status or f"test result: ok. {count} passed; 0 failed; 0 ignored;" not in output:
        raise RuntimeError("restored source did not pass the focused witness")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--only", action="append", choices=[item[0] for item in MUTATIONS])
    parser.add_argument("--evidence-dir", type=Path, default=ROOT / "docs/reports/ja1-s3/evidence/media-mutations")
    args = parser.parse_args()
    original = SOURCE.read_bytes()
    text = original.decode()
    selected = [item for item in MUTATIONS if not args.only or item[0] in args.only]
    for _, _, mutation in selected:
        if mutation(text) == text:
            raise RuntimeError("mutation did not change source")
    if args.list:
        for name, selector, _ in selected:
            print(f"{name}\t{full_selector(name, selector)}\t{RED_MARKERS[name]}")
        return
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    baseline_digest = hashlib.sha256(original).hexdigest()
    results = []
    metadata = {"source": str(SOURCE.relative_to(ROOT)), "source_sha256": baseline_digest,
                "command": "nice -n 19 python3 -I scripts/ja1-s3-c0-media-mutations.py",
                "results": results, "restored": False}
    try:
        for name, selector, mutation in selected:
            selector = full_selector(name, selector)
            print(f"{name} baseline green", flush=True)
            status, output = gate(name, selector, args.evidence_dir / f"{name}-baseline.log.gz")
            green(status, output)
            print(f"{name} red", flush=True)
            SOURCE.write_text(mutation(text))
            status, output = gate(name, selector, args.evidence_dir / f"{name}-red.log.gz")
            red_assertion = assertion_red(name, selector, status, output)
            SOURCE.write_bytes(original)
            if hashlib.sha256(SOURCE.read_bytes()).hexdigest() != baseline_digest:
                raise RuntimeError("source restore digest mismatch")
            print(f"{name} restored green", flush=True)
            status, output = gate(name, selector, args.evidence_dir / f"{name}-green.log.gz")
            green(status, output)
            results.append({"guard": name, "selector": selector, "red_marker": RED_MARKERS[name],
                            "red": "1 assertion failure", "green": "1 passed",
                            "red_assertion": red_assertion,
                            "restored_source_sha256": baseline_digest})
    finally:
        SOURCE.write_bytes(original)
        metadata["restored"] = hashlib.sha256(SOURCE.read_bytes()).hexdigest() == baseline_digest
        (args.evidence_dir / "results.json").write_text(json.dumps(metadata, indent=2) + "\n")
        print(f"restored source sha256 {baseline_digest}, matches={metadata['restored']}", flush=True)
    print(f"{len(results)} mutation pairs passed", flush=True)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    main()

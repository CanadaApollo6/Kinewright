# Pinned FFmpeg 8 custom AVIO and secondary-open audit

Inspected the official FFmpeg source archive at commit prefix `d1f31a829d`, matching the provisioned binary's `n8.0-23-gd1f31a829d-20251022` version. This is a source inspection, not a runtime acceptance result. It covers the supplied top-level custom AVIO context, rather than private demuxer/codec buffers covered by separate reservations.

## Supplied AVIO buffer

The archive contained 690 direct `libavformat` C/header files. Searches covered calls to `ffio_realloc_buf`, `ffio_ensure_seekback`, `ffio_rewind_with_probe_data`, `set_buf_size`, and writes to AVIO buffer size/pointer/seekability.

- [`aviobuf.c:55`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/aviobuf.c#L55) initializes both original/current size to the supplied 65,536 bytes and marks the context seekable when a seek callback exists.
- [`seek.c:174`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/seek.c#L174) contains the only external caller of `ffio_realloc_buf` in this source set. Its index/network heuristic returns immediately for protocol `file`, `pipe`, or `cache`. The inert `file:certified-owned-input` label selects that early return. It carries no real filename or locator and does not replace the supplied custom `pb`.
- [`aviobuf.c:1023`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/aviobuf.c#L1023) returns before seekback growth when `s->seekable` is nonzero. The original input AVIO retains its seek callback. Other seekability writes found apply to output contexts, URL-protocol contexts opened separately, or SWF's private decompression AVIO, rather than this original input context.
- [`format.c:285`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/format.c#L285) caps initial format probing at `format_probesize`, set here to 32,768. Its rewind can reuse at most the 65,536-byte buffered input window. [`img2dec.c:367`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/img2dec.c#L367) seeks and frees its separate 2,048-byte probe scratch under CUSTOM_IO instead of invoking another rewind allocation on the supplied context.
- [`aviobuf.c:533`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/aviobuf.c#L533) can shrink to the original buffer size. The remaining direct buffer-size assignments belong to the allocation, seekback, explicit-reallocation, and probe-rewind paths above.

Under these pinned-source conditions, the audit found no remaining growth path beyond 65,536 bytes for the supplied context. This conclusion is conditional on that FFmpeg implementation and the configured local label, seekability, CUSTOM_IO and probe bound. The runtime size check is a defensive refusal; it is not the argument for pre-allocation compliance. Native Windows execution and future FFmpeg revisions require their own verification.

Source file SHA-256 values:

```
318a36bcb5dffd145ff74619190a023458cba95ee90c1ce316e0e1a946f78f1b  aviobuf.c
68fa644ba14c782a779017e8faa7cd52bd3e3b4fac3995f8807f954d7ad26f43  seek.c
82824e50b3e21240c92f4052619828f76bd486ec96e5a577d833ff2cb06441fa  format.c
16886a564b194ef3eac45685a258263785820ac359e8a3627b683d238f8d3f3e  img2dec.c
```

`s3_certified_inert_label_has_no_provenance_fallback` renames away the actual source pathname, probes through the descriptor, decodes the original landmark, observes the AVIO buffer bound, denies a callback open, and requires latched output refusal. That callback witness does not establish denial of nested contexts that install FFmpeg's default callback.

## Secondary inputs and inherited policy

Opus C1 exposed a separate security defect in the earlier source. The top-level `io_open = deny_open` assignment does not cover every nested format context. The fixed label still has `file` semantics for relative URL resolution. The label is an AVIO sizing choice, not a secondary-open security boundary.

The pinned [`concatdec.c:311-338`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/concatdec.c#L311) allocates a fresh nested context, removes `AVFMT_FLAG_CUSTOM_IO`, and copies the interrupt callback and white/blacklists. It does not copy our `io_open` or `opaque`. It calls `avformat_open_input` on the playlist's URL. [`options.c:128-163`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/options.c#L128) installs the default callback in newly allocated contexts; that callback passes the context's protocol whitelist to `ffio_open_whitelist`.

The fix passes `protocol_whitelist="none"` and the supported `format_whitelist` through the options dictionary to `avformat_open_input`. It checks allocation errors and frees the dictionary after open. [`demux.c:221-255`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/demux.c#L221) applies those context options, uses an existing custom `pb` for probing, and rejects a format outside the whitelist before `read_header` runs. The certified top-level descriptor therefore needs no permitted URL protocol. [`avformat.c:770-794`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/avformat.c#L770) copies `format_whitelist` and `protocol_whitelist` into concat's nested context. Its default callback then cannot open a `file`, network, or wrapper protocol.

The format whitelist is `avi,mov,matroska,webm,wav,mp3,flac,ogg,aac,nut,mpegts,mpeg,mpegvideo,image2,png_pipe,jpeg_pipe,webp_pipe,bmp_pipe,tiff_pipe`. It excludes concat, HLS, DASH, and playlist demuxers. A listed demuxer is not sufficient for admission. The complete stream table and decoder bounds below must also pass. This source review does not certify every listed container/codec combination. The existing callback denial remains a separate guard and latches failure through output acceptance.

Pinned [`hls.c:604-665`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/hls.c#L604) opens local segments through its parent's `io_open`. Its nested demuxer also copies white/blacklists at [`hls.c:2195-2204`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/hls.c#L2195). HLS is refused before these operations by the production format whitelist.

## Stream-info decoder admission

Round 2 C1 identified a reservation bypass during automatic stream discovery. The earlier implementation supplied decoder options only for the initial stream table. Pinned [`demux.c:2520`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/demux.c#L2520) saves that initial count. At [`demux.c:2834`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/demux.c#L2834), newly discovered streams receive no caller options. The initial decoder loop at [`demux.c:2594`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/demux.c#L2594) also opens subtitle decoders regardless of otherwise complete metadata.

The revised production path refuses an empty table, `AVFMTCTX_NOHEADER`, and every stream kind other than audio/video before `avformat_find_stream_info`. This includes subtitles, attachments, data, and unknown kinds. Every admitted initial audio/video stream retains its conservative reservation, one decoder thread, `max_pixels`, and `max_samples` options. Reservations live through format teardown. A post-probe count/kind comparison refuses a changed table. That comparison is defensive and does not establish allocation safety.

The pinned flag contract in [`avformat.h:1324`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/avformat.h#L1324) allows streams after header parsing when `AVFMTCTX_NOHEADER` is set. The flag alone is insufficient as an allocation boundary. Immediately before probing, the implementation sets `AVFormatContext.max_streams` to the exact number of initial streams whose decoder bounds were reserved. It keeps that cap for subsequent packet reads. In pinned [`options.c:244-281`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/options.c#L244), `avformat_new_stream` returns null at the cap before growing the stream array, allocating an `AVStream`, or allocating its codec context. A later stream therefore cannot enter automatic probe decoding without bounds, even if a demuxer omits or clears `NOHEADER`. The initial header read still uses the existing 16-stream limit.

The whitelist audit found these stream-table behaviors:

| Demuxers | Pinned-source stream creation and admission |
| --- | --- |
| `avi` | [`avidec.c:607`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/avidec.c#L607) creates streams in `avi_read_header`. The packet path has no further top-level stream creation. Existing subtitle streams refuse before probing. |
| `matroska,webm` | [`matroskadec.c:3203`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/matroskadec.c#L3203) creates tracks through `matroska_parse_tracks`, called by `read_header`. Its other creation site at line 3397 creates header attachments. Non-A/V attachments and subtitles refuse. |
| `wav` | [`wavdec.c:418`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/wavdec.c#L418) and line 496 create header streams. The SPDIF packet helper's [`spdifdec.c:229`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/spdifdec.c#L229) creates a stream only when the context has none, which admission already refuses. |
| `mp3,flac,aac` | [`mp3dec.c:385`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/mp3dec.c#L385), [`flacdec.c:64`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/flacdec.c#L64), and [`aacdec.c:112`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/aacdec.c#L112) create their audio stream in the header. FLAC pictures and generic initial ID3 picture parsing also run before return from open. AAC's later `handle_id3` updates metadata without parsing additional picture streams. |
| `nut` | [`nutdec.c:364`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/nutdec.c#L364) creates the header's declared streams. [`nut_read_packet:1174`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/nutdec.c#L1174) skips later main/stream headers instead of creating streams. |
| `mpeg` | [`mpeg.c:134`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/mpeg.c#L134) sets `NOHEADER` and creates no initial streams. Its packet path creates streams at line 633. Production refuses before any stream-info probe. |
| `mpegts` | [`mpegts.c:3171`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/mpegts.c#L3171) sets `NOHEADER`, but the PMT path can clear it at line 2901. Packet-driven creation sites remain. Any still-dynamic table refuses, and the exact cap blocks new streams even after the flag clears. |
| `mov` | [`mov_read_packet:11120`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/mov.c#L11120) can return to the root atom parser through `mov_switch_root`. The atom table includes `trak` and other stream-producing handlers. No complete-header claim is made for every MOV input. Admission requires existing bounded A/V streams, and the exact cap blocks later stream creation before codec allocation. |
| `ogg` | [`ogg_read_page:431`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/oggdec.c#L431) can call `ogg_new_stream` from the packet path while no data packet has been seen. It does not set `NOHEADER`. The exact cap supplies the required pre-allocation bound for additional streams. |
| `mpegvideo` and image demuxers | [`rawdec.c:69`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/rawdec.c#L69) creates the raw video header stream. Missing dimensions refuse the existing decoder preflight. [`img2dec.c:196`](https://github.com/FFmpeg/FFmpeg/blob/d1f31a829d/libavformat/img2dec.c#L196) sets `NOHEADER`, including the listed image pipe demuxers. These inputs refuse the new dynamic-table guard. |

MOV, Ogg, and MPEG-TS stay listed only because the pinned `max_streams` check provides the prebound proof for later streams. Removing that check, or updating FFmpeg without rechecking it, invalidates this admission argument. This audit concerns automatic decoder allocations, not a new certification of all private demuxer buffers.

The source was downloaded into this worktree's `.review/c0-probe-audit` from the official [FFmpeg commit archive](https://codeload.github.com/FFmpeg/FFmpeg/tar.gz/d1f31a829d). Archive SHA-256 is `3b7d3230644037ae1fd84f09ba1d23b3fb56264610a520354835ca8e97ea05d3`. Additional source SHA-256 values are:

```
3abd044fbd0c6be3cb38b821c4e7f32f1da3f623cb0db6b552dd93374cbb4fe1  avformat.h
bf9583f3cb2a0f13f2269ffef302e577488473fd92e476eef400ac8988d8c329  options.c
4398a1ee4fed8025693285b8f793de7e1393d78f68983c7027ca6f46f3f3a697  demux.c
2f9f67ba9d07e3f90b3e089442d24faaf3741207bd48c9517938a0fc282c20ee  mpeg.c
d3af210b765416a9c1a6aceadbabe51a233a18a26fa8c3071b8b1c8307a3935a  mov.c
201d8bba496dfa342e0e289776c1856a26cd8d2cf11b58fee5700eba2824fb08  oggdec.c
0fd02d78127d4287b8f1942b12742c0ec763be4e71cf086e7f1a9a38a23ba857  mpegts.c
```

## Witnesses and independent mutations

The root assigned one exclusive focused Linux Cargo lane after these source changes. The lane is released. Against `owned_io.rs` SHA-256 `5e9dd06d37adca9c95c8f23c24cfc7e3f765c48483c4f8a98b942702d7dbbf8d`, all owned-I/O tests passed, including the new MPEG-PS, subtitle, stream-cap, and concat PCM controls. The new public production MPEG-PS import witness also passed. Only the protocol pair was authorized and run. Its baseline passed; changing only protocol `none` to `file` accepted 96,000 uncertified PCM bytes and failed `C0_PROTOCOL_OUTPUT_ZERO`; restoring the identical source passed again. Logs are under `.review/c0-probe-audit/focused-lane`.

Raw summaries:

```
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 1199 filtered out; finished in 1.06s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.59s
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1225 filtered out; finished in 0.03s
restored source sha256 5e9dd06d37adca9c95c8f23c24cfc7e3f765c48483c4f8a98b942702d7dbbf8d, matches=True
1 mutation pairs passed
```

The complete 26-case campaign, the new MPEG/cap/non-A/V mutation pairs, the format-policy pair, full suites, and native Windows remain **NOT RUN by this worker**. Root will rerun every media pair after the shared final source and review 3 snapshot are fixed. The old 15 pairs are not final-source evidence. Native Windows is separate from durable-write certification.

Fixture generation alone was checked with the provisioned `n8.0-23-gd1f31a829d-20251022` CLI. `ffprobe` reported `stream|codec_name=mpeg2video|codec_type=video|` for the 25 fps MPEG-PS and `stream|codec_name=pcm_s16le|codec_type=audio` plus `stream|codec_name=subrip|codec_type=subtitle` for the Matroska control. The generated files are in `.review/c0-probe-audit/fixtures`. These CLI checks do not establish the Rust guard observations or mutation results.

| Witness or case | Required observation |
| --- | --- |
| `s3_certified_wav_control_accepts_only_descriptor_bytes` | Rename the WAV locator away. Probe and decode all 48,000 samples through the pinned descriptor. The acceptance counter must observe at least 96,000 first-plane bytes. |
| `s3_certified_mpegps_refuses_incomplete_table_before_probe` | Generate real MPEG-PS. Inspect its successful header-only open to require zero streams and `NOHEADER`. Production probe must return the incomplete-table refusal with zero stream-info calls and zero accepted bytes. |
| `s3_certified_subtitle_refuses_unsupported_kind_before_probe` | Generate a complete Matroska header containing PCM audio and a real SubRip subtitle stream. Require the subtitle kind and no `NOHEADER` before exercising the production refusal. Its stream-info call count stays zero. A WAV control must make one observed call. |
| `s3_certified_probe_stream_limit_refuses_before_new_stream_allocation` | On a real WAV context, the initial header cap permits a second `avformat_new_stream`. After production probe admission, the same FFmpeg entry point must refuse and preserve the original count. |
| `s3_owned_import_refuses_dynamic_mpegps_before_probe` | Public project import first accepts a WAV control. MPEG-PS then reaches the exact dynamic-table refusal with zero stream-info calls. Document history, current manifest, durable root bytes, and staging reservations remain unchanged. |
| `s3_certified_concat_counterfactual_accepts_uncertified_pcm` | The test-only control permits `concat,wav` and protocol `file`. The same playlist fixture must successfully decode its uncertified one-second WAV, accepting at least 96,000 PCM bytes without calling the parent callback. |
| `s3_certified_probe_video_audio_seek_reopen_and_landmarks` | Existing certified A/V probe, landmarks, independent audio/video decode, seek and reopen stay valid under both whitelists. |
| `s3_certified_ffconcat_and_hls_refuse_uncertified_nested_output` | Certify only the playlist, leaving an existing distinguishable WAV uncertified. Both production opens must refuse with zero accepted decoded bytes and zero callback opens. |
| `nested-protocol-inheritance` | Unit-test policy admits `concat,hls,wav,mpegts`, retaining protocol `none`. Change only `none` to `file`. The intended red is `C0_PROTOCOL_OUTPUT_ZERO` after concat decodes uncertified WAV bytes without calling our callback. |
| `nested-format-policy` | Unit-test policy permits local `file`, retaining the production format whitelist. Delete only the format option. The intended red is `C0_FORMAT_OUTPUT_ZERO` after concat decodes uncertified WAV bytes. |
| `hls-format-policy` | Under the same isolated format arm, deletion must reach `C0_HLS_NO_SECONDARY_ATTEMPT` when HLS calls the parent callback. No accepted output is required for this red; the protected behavior is rejecting the unsupported demuxer before a secondary attempt. |
| `secondary-open-callback` | Replace only callback denial with an actual `avio_open2` against an existing WAV. The direct callback witness must fail `C0_SECONDARY_OPEN_DENIED`, then close that test handle. |
| `incomplete-stream-table` and `production-incomplete-stream-table` | Remove only the empty/dynamic-table refusal. The unit and public-import witnesses must fail their zero-probe-call assertion, even if another bound causes a later refusal. |
| `non-av-probe-kind` | Replace only the non-A/V refusal with the former unbounded `None` entry. The real subtitle witness must fail `C0_NON_AV_PROBE_NOT_ENTERED`. |
| `probe-stream-limit` | Widen only the probe cap back to 16. The actual FFmpeg stream-allocation entry point must fail `C0_PROBE_NEW_STREAM_ALLOCATION_REFUSED`. |
| `production-certified-through-use` | Remove only chunk SHA-256 verification and run project integration `s3_certified_input_through_use`. The intended red is its uncached-tamper output-refusal assertion after successful production admission and decoded landmarks. |

The isolation policies and unrestricted concat control compile only under `cfg(test)`. Feature `test-util` exposes `CertifiedSource::accepted_output_bytes()`, `observe_stream_info_probes`, and the existing decoder delivered-byte/cache observations. It cannot relax either whitelist. The bounded probe observer stores one saturating thread-local counter and reports the number of actual FFmpeg stream-info entries during a synchronous operation. It retains no media or allocation history. The output counter counts the first plane of accepted frames across decoders sharing the source. Zero rules out accepted decoded output, but does not claim to count denied protocol traffic. The protocol test deliberately admits the concat demuxer so the format guard cannot mask a missing protocol guard. The format test deliberately admits the file protocol so protocol denial cannot mask a missing format guard.

The original protocol mutant survived because concat's playlist omitted a duration. A later metadata refusal masked the missing protocol guard. The fixture now declares `duration 1.0` after its known one-second WAV entry. The counterfactual control must accept decoded PCM before the repaired protocol and format mutation witnesses can count. The previous survivor and its 15 completed pairs remain partial historical results. No previous pair is promoted to evidence for this source.

The media mutation runner has 26 cases. Every case requires a baseline pass, exactly one focused runtime failure containing its named protection assertion, restored-source SHA-256 equality, and a restored pass. `assertion_red` rejects default Result/Option unwrap/expect failures and requires exactly one panic from the exact selected test thread. Its file and line must match a marker-bearing assertion macro in the current mutant media source or the selected project witness file. The scanner includes shared helpers such as `assert_damaged`, outside the selected test function. It skips comments and balances macro delimiters with strings treated as tokens. Each accepted red records the thread, file, line, permitted macro lines, and witness-source SHA-256. Compilation errors, fixture-setup panics, barrier timeouts, missing prerequisites and unrelated assertions do not count. Pure Python checks resolved all 26 marker sites, accepted a shared-helper assertion, and rejected ten setup/attribution failures. The actual protocol red also passed these stricter checks.

The two production cases select `kinewright-project --test ja_s3_owned_inputs`; the remaining cases select the exact media unit test. Every Cargo invocation uses same-shell FFmpeg setup and `flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo ...`, a 6 GiB available-memory preflight, and one test thread. The `-o` rule prevents child processes from inheriting the lock file descriptor. Python runs as `nice -n 19 python3 -I`.

Selected-stream validation separately returns `InvalidCertificate` for missing or wrong-kind video/audio indices, including selected audio when video is present. `s3_certified_probe_rejects_missing_and_wrong_kind_selected_streams` covers those cases and valid narrowed A/V controls. Its index/kind mutation cases must fail `C0_SELECTED_STREAM_TYPED_REFUSAL`. The new admission guards retain the existing per-decoder allocation bounds and ledger ownership.

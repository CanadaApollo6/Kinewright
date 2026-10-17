//! PF1 S2c C-4: the S-2 witness fixtures (design §8 S-2) and what is known
//! about each from outside the decoder under test: a demux-only shadow
//! context (the anchor `A(t)` of today's seek, its key packets, the
//! tick-exact anchor boundaries) and the pinned FFmpeg CLI's own
//! linear decode (`ffprobe -show_frames`, `ffmpeg -f framehash`). Generated
//! with the CLI into the temp directory (deleted on drop), the way
//! `perf_fixtures` does; nothing is bundled. Every number is read from the
//! generated file; x264 runs single-threaded, bit-exact and CPU-independent so
//! the files are reproducible across machines' SIMD levels (their SHA-256 is
//! pinned in the witness module).

use std::process::Command;

use kinewright_core::{AssetId, ColorDescription, Rational, TimeCode};

use crate::{
    cache::FrameCache,
    decode::{
        Anchor, VideoDecoder, frame_to_global_timestamp, media_input, normalized_start, probe_path,
        stream_timestamp_to_global,
    },
    ffmpeg,
    frame::WorkingFrame,
    render::d65_assumption,
    sha256::sha256_file,
    test_support::{GeneratedMedia, ffmpeg_executable, ffprobe_executable},
};

/// One of the six S-2 files. All are 320x180, 90 frames, BT.709, GOP 24;
/// 30 fps except `Pyramid` and `EditList` (30000/1001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// libx264 defaults: B-frames; the mp4 muxer writes an `elst` for the DTS lead.
    Default,
    /// `-bf 3`, b-pyramid normal, `negative_cts_offsets` (`ctts` v1, DTS shifted).
    Pyramid,
    /// `-bf 3`, b-pyramid normal, plus an empty edit of 0.5 s (`elst` offset).
    EditList,
    /// `open-gop=1`: key packets with leading frames that cross the GOP.
    OpenGop,
    /// Variable frame rate (2 frames per 3 ticks of 1/30 s, alternating 3/2).
    Vfr,
    /// AVI: no packet PTS, so the decoder guesses timestamps from the DTS,
    /// and the frames drained at end of stream have none at all (the CLI and
    /// the decoder both see them). Covers S-2 rule 4 (not the `mov`
    /// demuxer) together with rule 3's guessed and missing timestamps; the
    /// `mov` rule 3 cases (missing, repeated) are injected (`Tamper`).
    AviDtsGuess,
}

pub(super) const KINDS: [Kind; 6] = [
    Kind::Default,
    Kind::Pyramid,
    Kind::EditList,
    Kind::OpenGop,
    Kind::Vfr,
    Kind::AviDtsGuess,
];

/// A raw-timestamp boundary: `tick` is the smallest stream tick whose seek
/// picks the next anchor (`after`); one tick earlier picks `before`.
#[derive(Clone, Debug)]
pub(super) struct TickBound {
    pub(super) tick: i64,
    pub(super) before: Option<Anchor>,
    pub(super) after: Option<Anchor>,
}

/// What the witnesses know about a file (cloneable, so tests can corrupt a copy).
#[derive(Clone)]
pub(super) struct Facts {
    pub(super) kind: Kind,
    /// A(t) for t in 0..=last, from today's seek on a shadow context.
    pub(super) anchors: Vec<Option<Anchor>>,
    /// (`pos`, grid frame) of each key packet (PTS, else DTS), by grid frame.
    pub(super) keys: Vec<(isize, i64)>,
    /// Grid frame of the last packet (the end of stream).
    pub(super) last: i64,
    /// The stream's start offset in grid frames (the edit list's, if any).
    pub(super) offset_frames: i64,
    pub(super) time_base: (i64, i64),
    pub(super) start: i64,
    pub(super) ticks: Vec<TickBound>,
    /// The stream tick each frame's seek time rescales to.
    pub(super) seek_ticks: Vec<i64>,
}

impl Facts {
    /// The `avformat_seek_file` timestamp (microseconds) that rescales to `tick`.
    pub(super) fn us_of_tick(&self, tick: i64) -> i64 {
        let (n, d) = self.time_base;
        i64::try_from(
            (i128::from(tick) * 1_000_000 * i128::from(n) * 2 + i128::from(d))
                / (2 * i128::from(d)),
        )
        .unwrap()
    }

    pub(super) fn tick_of_us(&self, us: i64) -> i64 {
        let (n, d) = self.time_base;
        i64::try_from(
            (i128::from(us) * i128::from(d) * 2 + 1_000_000 * i128::from(n))
                / (2 * 1_000_000 * i128::from(n)),
        )
        .unwrap()
    }
}

pub(super) struct Fixture {
    pub(super) kind: Kind,
    media: GeneratedMedia,
    pub(super) fps: Rational,
    pub(super) description: ColorDescription,
    pub(super) facts: Facts,
}

/// A frame of the CLI's linear decode: `best_effort_timestamp`, grid frame
/// and the SHA-256 `framehash` gives its raw planes (`sha`) and, after the
/// CLI's own conversion to full-range RGBA64LE with the arguments the design
/// specifies for the managed graph (`out`, see [`CONVERSION`]).
pub(super) struct RefFrame {
    pub(super) ts: Option<i64>,
    pub(super) grid: Option<i64>,
    pub(super) sha: String,
    pub(super) out: String,
}

/// The managed decoder's colour conversion (decode.rs `managed_filter_graph`
/// for these BT.709 limited-range files, no scaling), as CLI filter arguments.
/// Written out here rather than read from the decoder: it is the oracle.
pub(super) const CONVERSION: &str = "scale=w=320:h=180:flags=bicubic:in_color_matrix=bt709:\
     out_color_matrix=bt709:in_range=mpeg:out_range=jpeg,format=rgba64le";

/// The same conversion with swscale's bit-exact flags: the CPU-invariant
/// reference. `CONVERSION` (the production flags) picks CPU-specific scalers,
/// so its bytes are only comparable on the machine that produced them; this
/// one is byte-identical with and without SIMD
/// (`the_canonical_conversion_does_not_depend_on_the_cpu`).
pub(super) const CANONICAL: &str = "scale=w=320:h=180:flags=bicubic+bitexact+accurate_rnd+\
     full_chroma_int:in_color_matrix=bt709:out_color_matrix=bt709:in_range=mpeg:out_range=jpeg,\
     format=rgba64le";

/// The CLI arguments per file (the VUI colour tags let the managed decoder open).
///
/// CPU-independent generation: x264 runs in its `cpu-independent` mode (no
/// CPU-selected algorithms) and the RGB-to-YUV conversion of the test source
/// uses swscale's bit-exact flags, so the bytes do not depend on the machine's
/// SIMD level. `limited` runs the same recipe with every assembly path
/// switched off (`FFmpeg` `-cpuflags 0`, x264 `asm=0`) to show it
/// (`the_fixture_files_do_not_depend_on_the_cpu`).
fn arguments(kind: Kind, limited: bool) -> (Vec<String>, &'static str) {
    let params = |extra: &str| {
        format!(
            "colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv:keyint=24:min-keyint=24:threads=1:cpu-independent=1{extra}{}",
            if limited { ":asm=0" } else { "" }
        )
    };
    // The CLI has no `-b_pyramid` here: x264's own parameter is the same knob.
    let (x264, extra, ext): (_, &[&str], _) = match kind {
        Kind::Default => (params(""), &[], "mp4"),
        Kind::Pyramid => (
            params(":b-pyramid=normal"),
            &["-bf", "3", "-movflags", "+negative_cts_offsets"],
            "mp4",
        ),
        Kind::EditList => (
            params(":b-pyramid=normal"),
            &["-bf", "3", "-output_ts_offset", "0.5"],
            "mp4",
        ),
        Kind::OpenGop => (params(":open-gop=1:bframes=3"), &[], "mp4"),
        Kind::Vfr => (
            params(""),
            &[
                "-vf",
                "setpts=(floor(N/2)*3+mod(N\\,2)*2)/(30*TB)",
                "-fps_mode",
                "vfr",
            ],
            "mp4",
        ),
        Kind::AviDtsGuess => (params(""), &["-bf", "3"], "avi"),
    };
    // 30000/1001 where a 1-tick boundary matters: the grid's microsecond
    // truncation then rounds a frame's seek time to a different tick.
    let rate = if matches!(kind, Kind::Pyramid | Kind::EditList) {
        "30000/1001"
    } else {
        "30"
    };
    let base = "-f lavfi -i testsrc2=size=320x180:rate=RATE -frames:v 90 -c:v libx264 -preset \
                veryfast -pix_fmt yuv420p -color_primaries bt709 -color_trc bt709 -colorspace \
                bt709 -color_range tv -fflags +bitexact -flags:v +bitexact -sws_flags \
                +bitexact+accurate_rnd+full_chroma_int -x264-params";
    let args = (limited.then(|| ["-cpuflags", "0"].map(str::to_owned)))
        .into_iter()
        .flatten()
        .chain(
            (base
                .replace("RATE", rate)
                .split(' ')
                .filter(|a| !a.is_empty())
                .map(str::to_owned))
            .collect::<Vec<_>>(),
        )
        .chain([x264])
        .chain(extra.iter().map(|a| (*a).to_owned()))
        .collect();
    (args, ext)
}

/// Ceil of `ts * time_base * fps`, the decoder's `timestamp_to_grid_ceil`.
fn grid(ts: i64, time_base: ffmpeg::Rational, fps: Rational) -> i64 {
    let num = i128::from(time_base.numerator()) * i128::from(fps.numerator());
    let den = i128::from(time_base.denominator()) * i128::from(fps.denominator());
    if ts <= 0 || den == 0 {
        return 0;
    }
    i64::try_from((i128::from(ts) * num + den - 1) / den).unwrap_or(i64::MAX)
}

/// The anchor of one `avformat_seek_file` call on a shadow (demux-only) context.
fn shadow_anchor(
    input: &mut ffmpeg::format::context::Input,
    index: usize,
    us: i64,
) -> Option<Anchor> {
    input.seek(us, ..us).ok()?;
    let packet = input.packets().find(|(s, _)| s.index() == index)?.1;
    Some((packet.position(), packet.dts(), packet.is_key()))
}

/// SHA-256 of the file `kind`'s recipe generates, with all CPU-specific
/// assembly available (`limited` false) or switched off (`limited` true).
pub(super) fn generated_sha256(kind: Kind, limited: bool) -> String {
    let (args, ext) = arguments(kind, limited);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let media = GeneratedMedia::ffmpeg(&format!("pf1-s2c-{kind:?}-cpu"), &args, ext);
    sha256_file(media.path()).expect("hash the fixture")
}

impl Fixture {
    pub(super) fn new(kind: Kind) -> Self {
        Self::new_as(kind, kind)
    }

    /// A `kind` file generated as `made_like` would be: the substitute the
    /// shape checks must reject when the two differ.
    pub(super) fn new_as(kind: Kind, made_like: Kind) -> Self {
        let (args, ext) = arguments(made_like, false);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let media = GeneratedMedia::ffmpeg(&format!("pf1-s2c-{kind:?}"), &args, ext);
        let asset = probe_path(media.path(), AssetId(1)).expect("the S-2 fixture probes");
        let fps = asset.fps;
        let mut input = media_input(media.path()).expect("the fixture opens");
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .expect("video");
        let (index, time_base) = (stream.index(), stream.time_base());
        let start = normalized_start(stream.start_time());
        let (mut keys, mut last) = (Vec::new(), 0);
        for (s, packet) in input.packets() {
            if s.index() != index {
                continue;
            }
            let at = grid(
                packet.pts().or(packet.dts()).unwrap_or(0) - start,
                time_base,
                fps,
            );
            last = last.max(at);
            if packet.is_key() {
                keys.push((packet.position(), at));
            }
        }
        keys.sort_unstable_by_key(|k| k.1);
        let seek_us = |t: i64| {
            frame_to_global_timestamp(TimeCode(t), fps)
                + stream_timestamp_to_global(start, time_base)
        };
        // The identical call `VideoDecoder::decode_window` makes.
        let anchors: Vec<_> = (0..=last)
            .map(|t| shadow_anchor(&mut input, index, seek_us(t)))
            .collect();
        let mut facts = Facts {
            kind,
            anchors,
            keys,
            last,
            offset_frames: grid(start, time_base, fps),
            time_base: (
                i64::from(time_base.numerator()),
                i64::from(time_base.denominator()),
            ),
            start,
            ticks: Vec::new(),
            seek_ticks: Vec::new(),
        };
        facts.seek_ticks = (0..=last).map(|t| facts.tick_of_us(seek_us(t))).collect();
        // Tick-exact boundaries: bisect each anchor change down to one stream tick.
        for f in 1..=usize::try_from(last).unwrap() {
            if facts.anchors[f] == facts.anchors[f - 1] {
                continue;
            }
            let (lo_us, hi_us) = (
                seek_us(i64::try_from(f).unwrap() - 1),
                seek_us(i64::try_from(f).unwrap()),
            );
            let (mut lo, mut hi) = (facts.tick_of_us(lo_us), facts.tick_of_us(hi_us));
            let base = facts.anchors[f - 1];
            assert_eq!(
                shadow_anchor(&mut input, index, facts.us_of_tick(lo)),
                base,
                "{kind:?} f={f}"
            );
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if shadow_anchor(&mut input, index, facts.us_of_tick(mid)) == base {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let mut at = |tick| shadow_anchor(&mut input, index, facts.us_of_tick(tick));
            let (before, after, next) = (at(hi - 1), at(hi), at(hi + 1));
            assert!(
                before == base && after != base && after == next,
                "{kind:?} f={f} tick {hi}"
            );
            facts.ticks.push(TickBound {
                tick: hi,
                before,
                after,
            });
        }
        Self {
            kind,
            media,
            fps,
            description: asset.color_description,
            facts,
        }
    }

    /// A managed decoder as a reader opens it, with `threads` frame threads.
    pub(super) fn open(&self, threads: usize) -> VideoDecoder {
        let assumption = d65_assumption(&self.description);
        VideoDecoder::open_managed_threads(
            self.media.path(),
            self.fps,
            None,
            &self.description,
            assumption,
            threads,
            None,
        )
        .expect("the S-2 fixture opens managed")
    }

    /// The file's SHA-256, for the pin.
    pub(super) fn sha256(&self) -> String {
        sha256_file(self.media.path()).expect("hash the fixture")
    }

    /// `ffmpeg -f framehash` SHA-256 of each output frame (of the raw decode,
    /// or of `filter`'s output), in output order.
    fn framehashes(&self, filter: Option<&str>, limited: bool) -> Vec<String> {
        let mut command = Command::new(ffmpeg_executable());
        command.args(["-hide_banner", "-v", "error"]);
        if limited {
            command.args(["-cpuflags", "0"]); // no SIMD in FFmpeg's own code
        }
        command
            .arg("-i")
            .arg(self.media.path())
            .args(["-map", "0:v:0"]);
        if let Some(filter) = filter {
            command.args(["-vf", filter]);
        }
        let hashes = command
            .args([
                "-fps_mode",
                "passthrough",
                "-f",
                "framehash",
                "-hash",
                "sha256",
                "-",
            ])
            .output()
            .expect("run ffmpeg");
        assert!(
            hashes.status.success(),
            "framehash: {}",
            String::from_utf8_lossy(&hashes.stderr)
        );
        let text = String::from_utf8(hashes.stdout).unwrap();
        (text.lines().filter(|l| !l.starts_with('#')))
            .filter_map(|l| l.rsplit(',').next().map(|s| s.trim().to_owned()))
            .collect()
    }

    /// The framehashes of the file's converted frames with `filter`, with
    /// `FFmpeg`'s assembly on or (`limited`) off.
    pub(super) fn converted(&self, filter: &str, limited: bool) -> Vec<String> {
        self.framehashes(Some(filter), limited)
    }

    /// The CLI's own linear decode of the file, in display order: timestamps
    /// from `ffprobe -show_frames`, frame hashes from `ffmpeg -f framehash`.
    pub(super) fn reference(&self) -> Vec<RefFrame> {
        let path = self.media.path();
        let probe = Command::new(ffprobe_executable())
            .args(["-v", "error", "-select_streams", "v:0", "-show_frames"])
            .args([
                "-show_entries",
                "frame=best_effort_timestamp",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .expect("run ffprobe");
        assert!(
            probe.status.success(),
            "ffprobe: {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&probe.stdout).expect("ffprobe json");
        let shas = self.framehashes(None, false);
        let outs = self.framehashes(Some(CONVERSION), false);
        let frames = json["frames"].as_array().expect("ffprobe frames");
        assert!(
            frames.len() == shas.len() && shas.len() == outs.len(),
            "{:?}: ffprobe, framehash and converted framehash frame counts",
            self.kind
        );
        let stream_grid = |ts: i64| {
            let (n, d) = self.facts.time_base;
            let time_base =
                ffmpeg::Rational::new(i32::try_from(n).unwrap(), i32::try_from(d).unwrap());
            grid(ts - self.facts.start, time_base, self.fps)
        };
        let mut reference: Vec<_> = (frames.iter().zip(shas).zip(outs))
            .map(|((frame, sha), out)| {
                let ts = frame["best_effort_timestamp"].as_i64();
                RefFrame {
                    ts,
                    grid: ts.map(stream_grid),
                    sha,
                    out,
                }
            })
            .collect();
        reference.sort_by_key(|f| f.ts.unwrap_or(i64::MAX)); // display order
        reference
    }

    /// Assert the file has the properties its name promises, read from the
    /// container structure (box walk, packet order, the CLI's decode), so a
    /// silent encoder or muxer change fails loudly.
    pub(super) fn assert_shape(&self) {
        let k = self.kind;
        let mut input = media_input(self.media.path()).expect("open");
        let format = input.format().name().to_owned();
        let packets: Vec<_> = (input.packets())
            .map(|(_, p)| (p.pts(), p.dts(), p.is_key()))
            .collect();
        assert_eq!(
            format.contains("mov"),
            k != Kind::AviDtsGuess,
            "{k:?} demuxer {format}"
        );
        assert!(
            self.facts.keys.len() >= 3 && self.facts.last >= 60,
            "{k:?} needs >= 3 GOPs"
        );
        // Leading frames: a packet after a key packet, before the next, that
        // presents earlier than the key (open-GOP cross-GOP dependency).
        let leading = packets.windows(2).enumerate().any(|(i, _)| {
            let (Some(key), true) = (packets[i].0, packets[i].2) else {
                return false;
            };
            (packets[i + 1..].iter().take_while(|p| !p.2)).any(|p| p.0.is_some_and(|pts| pts < key))
        });
        assert_eq!(leading, k == Kind::OpenGop, "{k:?} open-GOP leading frames");
        if k == Kind::AviDtsGuess {
            // No container PTS at all; the decoder's timestamps are guesses.
            assert!(
                packets.iter().all(|p| p.0.is_none() && p.1.is_some()),
                "{k:?} PTS/DTS"
            );
            let reference = self.reference();
            assert!(
                reference.iter().any(|f| f.ts.is_some()),
                "{k:?} guessed timestamps"
            );
            assert!(
                reference.iter().any(|f| f.ts.is_none()),
                "{k:?} frames with no timestamp"
            );
            let (mut decoder, mut cache) = (self.open(1), FrameCache::<WorkingFrame>::new(1));
            decoder
                .decode_window(TimeCode(0), TimeCode(0), &mut cache)
                .unwrap();
            let end = TimeCode(self.facts.last + 100);
            decoder
                .decode_window_sequential(TimeCode(1), end, &mut cache)
                .unwrap();
            let frames = &decoder.probe().frames;
            assert!(
                frames.iter().any(|f| f.ts.is_none()),
                "{k:?}: the decoder saw no untimed frame"
            );
            return;
        }
        let bytes = std::fs::read(self.media.path()).expect("read fixture");
        let stbl = ["moov", "trak", "mdia", "minf", "stbl", "ctts"];
        let ctts = mp4_box(&bytes, &stbl).expect("ctts");
        let entries = |b: &[u8], size: usize, at: usize| -> Vec<i32> {
            (b[8..].chunks_exact(size))
                .map(|e| i32::from_be_bytes(e[at..at + 4].try_into().unwrap()))
                .collect()
        };
        let offsets = entries(ctts, 8, 4);
        let signed = ctts[0] == 1 && offsets.iter().any(|o| *o < 0);
        assert_eq!(
            signed,
            k == Kind::Pyramid,
            "{k:?} negative composition offsets (ctts v{})",
            ctts[0]
        );
        // The edit list's media times: -1 is an empty edit (the 0.5 s offset).
        let edits = mp4_box(&bytes, &["moov", "trak", "edts", "elst"]).map(|e| {
            assert_eq!(e[0], 0, "{k:?} elst version");
            entries(e, 12, 4)
        });
        let edits = edits.unwrap_or_default();
        match k {
            Kind::EditList => assert!(
                edits.len() == 2 && edits[0] == -1 && edits[1] >= 0,
                "{k:?} elst {edits:?}"
            ),
            Kind::Pyramid => assert!(edits.is_empty() || edits == [0], "{k:?} elst {edits:?}"),
            _ => assert!(
                edits.len() == 1 && edits[0] > 0,
                "{k:?} DTS-lead elst {edits:?}"
            ),
        }
        assert_eq!(
            self.facts.start > 0,
            k == Kind::EditList,
            "{k:?} start offset"
        );
        let mut pts: Vec<i64> = packets.iter().filter_map(|p| p.0).collect();
        pts.sort_unstable();
        let spacing: std::collections::BTreeSet<_> = pts.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(
            spacing.len() > 1,
            k == Kind::Vfr,
            "{k:?} VFR packet spacing {spacing:?}"
        );
    }
}

/// The payload of the box at `path` (each element a 4-character type), after
/// its 4-byte version/flags word's owner header: children are searched in
/// the payload; for a full box (`ctts`, `elst`) the payload starts at the
/// version byte.
fn mp4_box<'a>(mut bytes: &'a [u8], path: &[&str]) -> Option<&'a [u8]> {
    for name in path {
        let mut at = 0;
        loop {
            let size = usize::try_from(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
                .ok()?;
            let kind = bytes.get(at + 4..at + 8)?;
            if size < 8 {
                return None;
            }
            if kind == name.as_bytes() {
                bytes = bytes.get(at + 8..at + size)?;
                break;
            }
            at += size;
        }
    }
    Some(bytes)
}

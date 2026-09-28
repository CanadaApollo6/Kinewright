//! PF1 S2c C-4: the S-2 witness fixtures (design §8 S-2) and what a demux-only
//! shadow context says about each: the anchor `A(t)` of today's seek, its key
//! packets and its last frame. Generated with the pinned FFmpeg CLI into the
//! temp directory (deleted on drop), the way `perf_fixtures` does; nothing is
//! bundled. Every number here is read from the generated file.

use kinewright_core::{AssetId, ColorDescription, Rational, TimeCode};

use crate::{
    decode::{
        VideoDecoder, frame_to_global_timestamp, media_input, normalized_start, probe_path,
        stream_timestamp_to_global,
    },
    ffmpeg,
    render::d65_assumption,
    test_support::GeneratedMedia,
};

/// One of the six S-2 files. All are 320x180, 90 frames, BT.709, GOP 24;
/// 30 fps except `Pyramid` and `EditList` (30000/1001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// libx264 defaults: B-frames; the mp4 muxer writes an `elst` for the DTS lead.
    Default,
    /// `-bf 3`, b-pyramid normal, `negative_cts_offsets` (ctts v1, DTS shifted).
    Pyramid,
    /// `-bf 3`, b-pyramid normal, plus an empty edit of 0.5 s (`elst` offset).
    EditList,
    /// `open-gop=1`: key packets whose leading B-frames cross the GOP.
    OpenGop,
    /// Variable frame rate (2 frames per 3 ticks of 1/30 s, alternating 3/2).
    Vfr,
    /// AVI: packets carry a DTS and no PTS; not a `mov` demuxer (S-2 rules 3, 4).
    NoPts,
}

pub(super) const KINDS: [Kind; 6] = [
    Kind::Default,
    Kind::Pyramid,
    Kind::EditList,
    Kind::OpenGop,
    Kind::Vfr,
    Kind::NoPts,
];

/// (`pos`, DTS, key flag): the first packet of the video stream after the
/// seek, as S-2 rule 1 defines the anchor.
pub(super) type Anchor = (isize, Option<i64>, bool);

pub(super) struct Fixture {
    pub(super) kind: Kind,
    media: GeneratedMedia,
    pub(super) fps: Rational,
    pub(super) description: ColorDescription,
    /// A(t) for t in 0..=last, from today's seek on a shadow context.
    pub(super) anchors: Vec<Option<Anchor>>,
    /// (`pos`, grid frame) of each key packet (PTS, else DTS), by grid frame.
    pub(super) keys: Vec<(isize, i64)>,
    /// Grid frame of the last packet (the end of stream).
    pub(super) last: i64,
    pub(super) start_pts: i64,
    /// The stream's start offset in grid frames (the edit list's, if any).
    pub(super) offset_frames: i64,
}

/// The CLI arguments per file (the VUI colour tags let the managed decoder open).
fn arguments(kind: Kind) -> (Vec<String>, &'static str) {
    let params = |extra: &str| {
        format!(
            "colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv:keyint=24:min-keyint=24{extra}"
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
        Kind::NoPts => (params(""), &["-bf", "3"], "avi"),
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
                bt709 -color_range tv -x264-params";
    let args = (base
        .replace("RATE", rate)
        .split(' ')
        .filter(|a| !a.is_empty())
        .map(str::to_owned))
    .collect::<Vec<_>>()
    .into_iter()
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

impl Fixture {
    pub(super) fn new(kind: Kind) -> Self {
        let (args, ext) = arguments(kind);
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
        let anchors = (0..=last)
            .map(|t| {
                // The identical call `VideoDecoder::decode_window` makes.
                let ts = frame_to_global_timestamp(TimeCode(t), fps)
                    + stream_timestamp_to_global(start, time_base);
                input.seek(ts, ..ts).ok()?;
                let packet = input.packets().find(|(s, _)| s.index() == index)?.1;
                Some((packet.position(), packet.dts(), packet.is_key()))
            })
            .collect();
        let description = asset.color_description;
        let offset_frames = grid(start, time_base, fps);
        Self {
            kind,
            media,
            fps,
            description,
            anchors,
            keys,
            last,
            start_pts: start,
            offset_frames,
        }
    }

    /// A managed decoder as a reader opens it (one frame thread).
    pub(super) fn open(&self) -> VideoDecoder {
        let assumption = d65_assumption(&self.description);
        VideoDecoder::open_managed_threads(
            self.media.path(),
            self.fps,
            None,
            &self.description,
            assumption,
            1,
        )
        .expect("the S-2 fixture opens managed")
    }

    /// Assert the file has the property its name promises (read from the
    /// container, not assumed), so a silent encoder change fails loudly.
    pub(super) fn assert_shape(&self) {
        let bytes = std::fs::read(self.media.path()).expect("read fixture");
        let open_gop = bytes.windows(10).any(|w| w == b"open_gop=1");
        let mut input = media_input(self.media.path()).expect("open");
        let packets: Vec<_> = input.packets().map(|(_, p)| (p.pts(), p.dts())).collect();
        let k = self.kind;
        let lead = packets.first().and_then(|p| p.1).is_some_and(|d| d < 0);
        let deltas: std::collections::BTreeSet<_> = packets
            .windows(2)
            .filter_map(|w| Some(w[1].1? - w[0].1?))
            .collect();
        assert_eq!(open_gop, k == Kind::OpenGop, "{k:?} open-gop flag");
        assert_eq!(
            packets.iter().all(|p| p.0.is_none()),
            k == Kind::NoPts,
            "{k:?} PTS presence"
        );
        if !matches!(k, Kind::Vfr | Kind::NoPts) {
            assert_eq!(lead, k != Kind::EditList, "{k:?} DTS lead");
        }
        assert_eq!(
            self.start_pts > 0,
            k == Kind::EditList,
            "{k:?} elst start offset"
        );
        assert_eq!(
            deltas.len() > 1,
            k == Kind::Vfr,
            "{k:?} VFR packet spacing: {deltas:?}"
        );
        assert!(
            self.keys.len() >= 3 && self.last >= 60,
            "{k:?} needs >= 3 GOPs"
        );
    }
}

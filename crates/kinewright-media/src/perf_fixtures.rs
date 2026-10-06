//! PF1 W-0/W-1: the shared performance workloads. MO2 R28's builders, moved
//! here unchanged from `mo2_perf_fixtures`, plus PF1's four production
//! mirrors (design §3).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use kinewright_core::{
    AssetId, AutomationCurve, BlendMode, Clip, ClipContent, Document, Keyframe, MediaAsset,
    MediaKind, TimeCode, Title, TitlePosition, Track, TrackId, TrackKind,
};

use crate::{
    decode::probe_path,
    mo2_fixtures::{clip, effect, with_transition},
    test_support::{GeneratedMedia, tone, wav_f32},
};

pub(crate) const MIB: u64 = 1 << 20;
const HD: (u32, u32) = (1920, 1080);

/// A generated file (deleted on drop) and its probed asset.
pub(crate) type Source = (GeneratedMedia, MediaAsset);

/// One generated 30 fps BT.709 H.264 source (`performance_workloads`' args).
fn source(filter: &str, size: (u32, u32), frames: i64, id: u64) -> Source {
    encode("mo2-r28", filter, size, frames, 60, false, &[], id)
}

/// W-0: MO2's x264 arguments plus size, GOP, optional temporal noise and
/// extra inputs/outputs (the talk's audio), from the pinned tool.
#[allow(clippy::too_many_arguments)]
fn encode(
    label: &str,
    filter: &str,
    (w, h): (u32, u32),
    frames: i64,
    gop: u32,
    noise: bool,
    extra: &[&str],
    id: u64,
) -> Source {
    let noise = if noise {
        " -vf noise=alls=10:allf=t"
    } else {
        ""
    };
    let args = format!(
        "-f lavfi -i {filter}=size={w}x{h}:rate=30 -frames:v {frames}{noise} -c:v libx264 -preset \
         veryfast -pix_fmt yuv420p -color_primaries bt709 -color_trc bt709 -colorspace bt709 \
         -color_range tv -x264-params colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv -g {gop}"
    );
    let mut args: Vec<&str> = args.split(' ').collect();
    args.splice(4..4, extra.iter().copied());
    let media = GeneratedMedia::ffmpeg(label, &args, "mp4");
    let asset = probe_path(media.path(), AssetId(id)).expect("the perf source probes");
    (media, asset)
}

/// PF1 W-1 video: 1080p-class GOP-`gop` source, optionally noisy.
fn video(filter: &str, size: (u32, u32), frames: i64, gop: u32, noise: bool, id: u64) -> Source {
    encode("pf1-w1", filter, size, frames, gop, noise, &[], id)
}

/// W-0: a stereo 48 kHz music bed from `tone`/`wav_f32`, never lavfi; it
/// keeps the audio ring live for the whole timeline.
fn music(frames: i64, id: u64) -> Source {
    let samples = frames as usize * 1_600;
    let (low, high) = (
        tone(220.0, 0.2, 48_000, samples),
        tone(330.0, 0.15, 48_000, samples),
    );
    let stereo: Vec<f32> = low.iter().zip(&high).flat_map(|(l, r)| [*l, *r]).collect();
    let media = GeneratedMedia::from_bytes("pf1-music", "wav", &wav_f32(&stereo, 48_000, 2));
    let asset = probe_path(media.path(), AssetId(id)).expect("the music bed probes");
    (media, asset)
}

fn span(mut clip: Clip, asset: &MediaAsset, source: i64, len: i64, start: i64) -> Clip {
    (clip.asset, clip.timeline_start) = (asset.id, TimeCode(start));
    clip.source_range = TimeCode(source)..TimeCode(source + len);
    clip
}

fn media_clip(id: u64, asset: &MediaAsset, source: i64, len: i64, start: i64) -> Clip {
    let base = clip(id, ClipContent::Media, BlendMode::Normal, Vec::new());
    span(base, asset, source, len, start)
}

fn title(id: u64, text: &str, position: TitlePosition, start: i64, len: i64) -> Clip {
    let title = Title {
        text: text.to_owned(),
        position,
        ..Title::default()
    };
    let mut clip = clip(id, ClipContent::Title(title), BlendMode::Normal, Vec::new());
    (clip.timeline_start, clip.source_range) = (TimeCode(start), TimeCode(0)..TimeCode(len));
    clip
}

/// Tracks bottom to top; a track of audio-only media is an audio track.
fn document(resolution: (u32, u32), media: &[Source], tracks: Vec<Vec<Clip>>) -> Document {
    let audio_only = |clip: &Clip| {
        let asset = media.iter().find(|(_, asset)| asset.id == clip.asset);
        clip.content.is_media() && asset.is_some_and(|(_, a)| a.kind == MediaKind::Audio)
    };
    let track = |(index, clips): (usize, Vec<Clip>)| Track {
        id: TrackId(index as u64 + 1),
        kind: if clips.iter().all(audio_only) {
            TrackKind::Audio
        } else {
            TrackKind::Video
        },
        sync_lock: true,
        clips,
    };
    let end =
        |clip: &Clip| clip.timeline_start.0 + clip.source_range.end.0 - clip.source_range.start.0;
    let duration = tracks.iter().flatten().map(end).max().unwrap_or(0);
    let document = Document {
        resolution,
        duration: TimeCode(duration),
        tracks: tracks.into_iter().enumerate().map(track).collect(),
        media_pool: media.iter().map(|(_, asset)| asset.clone()).collect(),
        ..Document::default()
    };
    document.validate().expect("the perf workloads are valid");
    document
}

/// A workload and the generated media it plays.
pub(crate) struct Workload(pub(crate) Document, pub(crate) Vec<Source>);

/// `performance_workloads`' typical (1080p, 3 tracks, 24 clips × 15) and heavy
/// (4K, 4 tracks, 200 clips × 12) lanes: clips end to end, windows cycling.
pub(crate) fn cuts(
    resolution: (u32, u32),
    frames: i64,
    tracks: usize,
    clips: u64,
    len: i64,
) -> Workload {
    let media = [("testsrc2", 1), ("smptebars", 2)]
        .map(|(filter, id)| source(filter, resolution, frames, id));
    let mut lanes = vec![Vec::new(); tracks];
    for index in 0..clips {
        let asset = &media[index as usize % 2].1;
        let lane = &mut lanes[index as usize % tracks];
        let base = clip(index + 1, ClipContent::Media, BlendMode::Normal, Vec::new());
        let start = lane.len() as i64 * len;
        lane.push(span(
            base,
            asset,
            (index as i64 * 7) % (asset.duration.0 - len),
            len,
            start,
        ));
    }
    Workload(document(resolution, &media, lanes), media.into())
}

/// Amendment R41 (U-1): `n` assets over one generated source (distinct ids,
/// so distinct reader keys), one `len`-frame clip each, end to end.
pub(crate) fn many_sources(resolution: (u32, u32), n: u64, len: i64) -> Workload {
    let (media, asset) = source("testsrc2", resolution, len, 1);
    let assets: Vec<MediaAsset> = (1..=n)
        .map(|id| MediaAsset {
            id: AssetId(id),
            ..asset.clone()
        })
        .collect();
    let clips = (assets.iter().zip(0..))
        .map(|(asset, index)| media_clip(asset.id.0, asset, 0, len, index * len))
        .collect();
    let document = Document {
        resolution,
        duration: TimeCode(n as i64 * len),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips,
        }],
        media_pool: assets,
        ..Document::default()
    };
    document.validate().expect("many sources are valid");
    Workload(document, vec![(media, asset)])
}

/// PF1 G13: four sources, one per track, each whole and screened over the
/// last, so every frame decodes all four.
pub(crate) fn four_sources(resolution: (u32, u32), frames: i64) -> Workload {
    let filters = [
        ("testsrc2", 1),
        ("smptebars", 2),
        ("gradients", 3),
        ("testsrc", 4),
    ];
    let media = filters.map(|(filter, id)| source(filter, resolution, frames, id));
    let lanes = (media.iter().enumerate())
        .map(|(index, (_, asset))| {
            let blend = if index == 0 {
                BlendMode::Normal
            } else {
                BlendMode::Screen
            };
            let base = clip(index as u64 + 1, ClipContent::Media, blend, Vec::new());
            vec![span(base, asset, 0, frames, 0)]
        })
        .collect();
    Workload(document(resolution, &media, lanes), media.into())
}

/// K-1 (S2b-3): one source with `titles` full-length titles over it.
pub(crate) fn titled(resolution: (u32, u32), frames: i64, titles: u64) -> Workload {
    let media = [source("testsrc2", resolution, frames, 1)];
    let base = clip(1, ClipContent::Media, BlendMode::Normal, Vec::new());
    let mut lanes = vec![vec![span(base, &media[0].1, 0, frames, 0)]];
    let positions = [
        TitlePosition::Top,
        TitlePosition::Center,
        TitlePosition::LowerThird,
    ];
    for (id, position) in (0..titles).zip(positions.into_iter().cycle()) {
        lanes.push(vec![title(10 + id, "Over", position, 0, frames)]);
    }
    Workload(document(resolution, &media, lanes), media.into())
}

/// R28 `blend_heavy`: 4-track 1080p — presenter, picture-in-picture, a `Screen` leak whose
/// 30-frame clips each enter by a full-length `push_left`, and an adjustment.
pub(crate) fn blend_heavy(frames: i64) -> Workload {
    let filters = [("testsrc2", 1), ("smptebars", 2), ("gradients", 3)];
    let media = filters.map(|(filter, id)| source(filter, (1920, 1080), frames, id));
    let pip = vec![effect(
        1,
        "transform",
        &[("scale_percent", 35), ("x_percent", 30)],
    )];
    let leak = |at: i64| {
        let base = clip(
            10 + at.cast_unsigned(),
            ClipContent::Media,
            BlendMode::Screen,
            Vec::new(),
        );
        with_transition(span(base, &media[2].1, at, 30, at), "push_left", 30)
    };
    let look = vec![effect(
        2,
        "primary_correction",
        &[("saturation_percent", 40)],
    )];
    let mut adjustment = clip(3, ClipContent::Adjustment, BlendMode::Normal, look);
    adjustment.source_range = TimeCode(0)..TimeCode(frames);
    let whole = |id, effects, asset| {
        span(
            clip(id, ClipContent::Media, BlendMode::Normal, effects),
            asset,
            0,
            frames,
            0,
        )
    };
    let lanes = vec![
        vec![whole(1, Vec::new(), &media[0].1)],
        vec![whole(2, pip, &media[1].1)],
        (0..frames / 30).map(|at| leak(at * 30)).collect(),
        vec![adjustment],
    ];
    Workload(document((1920, 1080), &media, lanes), media.into())
}

/// W-1 `explainer_16x9`: a noisy GOP-250 presenter; three GOP-60 cutaways,
/// one every 6 s for 3 s (odd ones as a 35% picture-in-picture); a title
/// card, two callouts and music.
pub(crate) fn explainer_16x9() -> Workload {
    let mut media = vec![video("testsrc2", HD, 1_800, 250, true, 1)];
    for (id, filter) in [(2, "testsrc2"), (3, "smptebars"), (4, "gradients")] {
        media.push(video(filter, HD, 240, 60, false, id));
    }
    media.push(music(1_800, 5));
    let cutaway = |k: u64| {
        let (asset, at) = (&media[1 + k as usize % 3].1, k as i64);
        let mut cut = media_clip(10 + k, asset, at * 31 % 150, 90, at * 180);
        if k % 2 == 1 {
            let pip = [("scale_percent", 35), ("x_percent", 30), ("y_percent", -30)];
            cut.effects.push(effect(1, "transform", &pip));
        }
        cut
    };
    let titles = vec![
        title(30, "Title card", TitlePosition::Center, 0, 90),
        title(31, "Callout one", TitlePosition::Top, 300, 120),
        title(32, "Callout two", TitlePosition::LowerThird, 1_200, 120),
    ];
    let lanes = vec![
        vec![media_clip(1, &media[0].1, 0, 1_800, 0)],
        (0..10).map(cutaway).collect(),
        titles,
        vec![media_clip(40, &media[4].1, 0, 1_800, 0)],
    ];
    Workload(document(HD, &media, lanes), media)
}

/// W-1 `reel_9x16` (1080×1920) and `feed_4x5` (1080×1350): two noisy
/// GOP-60 sources at the document size and a 1080p source scaled to fill;
/// a cut every 1.5 s, four keyframed titles, one `Screen` blend, one
/// adjustment and music.
pub(crate) fn social((w, h): (u32, u32)) -> Workload {
    let media = vec![
        video("testsrc2", (w, h), 1_800, 60, true, 1),
        video("smptebars", (w, h), 1_800, 60, true, 2),
        video("gradients", HD, 1_800, 60, false, 3),
        music(1_800, 4),
    ];
    let fill = i64::from((1_600 * h).div_ceil(9 * w));
    let (mut cuts, mut fills) = (Vec::new(), Vec::new());
    for i in 0..40_i64 {
        let asset = &media[i as usize % 2].1;
        cuts.push(media_clip(10 + i as u64, asset, i * 97 % 1_755, 45, i * 45));
        if i % 4 == 2 {
            let mut fit = media_clip(60 + i as u64, &media[2].1, i * 45, 45, i * 45);
            fit.effects = vec![effect(1, "transform", &[("scale_percent", fill)])];
            fills.push(fit);
        }
    }
    let mut screen = media_clip(3, &media[1].1, 900, 300, 900);
    screen.blend_mode = BlendMode::Screen;
    let look = vec![effect(
        1,
        "primary_correction",
        &[("saturation_percent", 40)],
    )];
    let mut adjustment = clip(4, ClipContent::Adjustment, BlendMode::Normal, look);
    (adjustment.timeline_start, adjustment.source_range) =
        (TimeCode(600), TimeCode(0)..TimeCode(900));
    let keyframed = |j: i64| {
        let mut caption = title(
            5 + j as u64,
            "Keyframed caption",
            TitlePosition::Center,
            j * 450,
            450,
        );
        let mut motion = effect(1, "transform", &[]);
        for (name, from, to) in [
            ("scale_percent", 80, 120),
            ("x_percent", -20, 20),
            ("y_percent", 10, -10),
            ("rotation_centidegrees", -500, 500),
        ] {
            let key = |at, value| Keyframe {
                at: TimeCode(at),
                value,
                interpolation: kinewright_core::KeyframeInterpolation::Linear,
                tangent_in: 0,
                tangent_out: 0,
            };
            let keyframes = vec![key(0, from), key(449, to)];
            motion
                .keyframes
                .insert(name.to_owned(), AutomationCurve { keyframes });
        }
        caption.effects.push(motion);
        caption
    };
    let lanes = vec![
        cuts,
        fills,
        vec![screen],
        vec![adjustment],
        (0..4).map(keyframed).collect(),
        vec![media_clip(200, &media[3].1, 0, 1_800, 0)],
    ];
    Workload(document((w, h), &media, lanes), media)
}

/// W-1 `talk_recut`: 120 two-second spans of one noisy GOP-250 600 s talk
/// carrying an AAC tone; span *i* starts at Σ (2 s + jump), jumps cycling
/// 0.5 / 2 / 5 s (same-GOP jumps included); one lower third.
pub(crate) fn talk_recut() -> Workload {
    // One second of 220 Hz at 48 kHz is exactly 220 periods, so it loops cleanly.
    let wav = wav_f32(&tone(220.0, 0.3, 48_000, 48_000), 48_000, 1);
    let tone = GeneratedMedia::from_bytes("pf1-talk-tone", "wav", &wav);
    let path = tone.path().to_string_lossy().into_owned();
    let audio = [
        "-stream_loop",
        "-1",
        "-i",
        &path,
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-shortest",
    ];
    let media = vec![encode(
        "pf1-talk", "testsrc2", HD, 18_000, 250, true, &audio, 1,
    )];
    let (mut spans, mut source) = (Vec::new(), 0);
    for i in 0..120_i64 {
        spans.push(media_clip(1 + i as u64, &media[0].1, source, 60, i * 60));
        source += 60 + [15, 60, 150][i as usize % 3];
    }
    let lower_third = vec![title(200, "Speaker", TitlePosition::LowerThird, 300, 300)];
    Workload(document(HD, &media, vec![spans, lower_third]), media)
}

/// Amendment R54's witnesses: one GOP-`gop` `testsrc2` source of `size`,
/// one clip of all of it from in-point 0, on a `document`-sized timeline.
pub(crate) fn one_source(
    size: (u32, u32),
    document_size: (u32, u32),
    frames: i64,
    gop: u32,
) -> Workload {
    let media = vec![encode(
        "pf1-r54",
        "testsrc2",
        size,
        frames,
        gop,
        false,
        &[],
        1,
    )];
    let spans = vec![media_clip(1, &media[0].1, 0, frames, 0)];
    Workload(document(document_size, &media, vec![spans]), media)
}

/// P-seek's single-source GOP-60 document (L-1): noisy 1080p, 60 s.
pub(crate) fn seek_gop60() -> Workload {
    let media = vec![video("testsrc2", HD, 1_800, 60, true, 1)];
    let spans = vec![media_clip(1, &media[0].1, 0, 1_800, 0)];
    Workload(document(HD, &media, vec![spans]), media)
}

/// P-rss's title-only document (G15): 1080p, 60 s, no media.
pub(crate) fn title_only() -> Workload {
    Workload(title_card(HD, 1_800), Vec::new())
}

/// One centred title over `frames`: a document with no media at all.
pub(crate) fn title_card(resolution: (u32, u32), frames: i64) -> Document {
    let card = title(1, "Title only", TitlePosition::Center, 0, frames);
    document(resolution, &[], vec![vec![card]])
}

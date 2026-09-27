//! PF1 W-0/W-1: the shared performance workloads, starting with MO2 R28's
//! builders, moved here unchanged from `mo2_perf_fixtures`.

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]

use kinewright_core::{
    AssetId, BlendMode, Clip, ClipContent, Document, MediaAsset, TimeCode, Track, TrackId,
    TrackKind,
};

use crate::{
    decode::probe_path,
    mo2_fixtures::{clip, effect, with_transition},
    test_support::GeneratedMedia,
};

pub(crate) const MIB: u64 = 1 << 20;

/// One generated 30 fps BT.709 H.264 source (`performance_workloads`' args).
fn source(filter: &str, (w, h): (u32, u32), frames: i64, id: u64) -> (GeneratedMedia, MediaAsset) {
    let args = format!(
        "-f lavfi -i {filter}=size={w}x{h}:rate=30 -frames:v {frames} -c:v libx264 -preset \
         veryfast -pix_fmt yuv420p -color_primaries bt709 -color_trc bt709 -colorspace bt709 \
         -color_range tv -x264-params colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv -g 60"
    );
    let media = GeneratedMedia::ffmpeg("mo2-r28", &args.split(' ').collect::<Vec<_>>(), "mp4");
    let asset = probe_path(media.path(), AssetId(id)).expect("the R28 source probes");
    (media, asset)
}

fn span(mut clip: Clip, asset: &MediaAsset, source: i64, len: i64, start: i64) -> Clip {
    (clip.asset, clip.timeline_start) = (asset.id, TimeCode(start));
    clip.source_range = TimeCode(source)..TimeCode(source + len);
    clip
}

fn document(
    resolution: (u32, u32),
    media: &[(GeneratedMedia, MediaAsset)],
    tracks: Vec<Vec<Clip>>,
) -> Document {
    let track = |(index, clips): (usize, Vec<Clip>)| Track {
        id: TrackId(index as u64 + 1),
        kind: TrackKind::Video,
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
    document.validate().expect("the R28 workloads are valid");
    document
}

/// A workload and the generated media it plays.
pub(crate) struct Workload(
    pub(crate) Document,
    pub(crate) Vec<(GeneratedMedia, MediaAsset)>,
);

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

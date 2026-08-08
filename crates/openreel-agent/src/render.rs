use std::fmt::Write as _;

use openreel_core::{
    ClipId, Document, Rational, TimeCode, TrackKind, map_source_range_to_project,
};

#[must_use]
pub fn render_timeline_state(document: &Document) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "project fps={}/{} size={}x{} duration={}",
        document.fps.numerator(),
        document.fps.denominator(),
        document.resolution.0,
        document.resolution.1,
        frame_and_seconds(document.duration, document.fps),
    );
    let clip_count = document
        .tracks
        .iter()
        .map(|track| track.clips.len())
        .sum::<usize>();
    let _ = writeln!(
        output,
        "tracks={} clips={} assets={}",
        document.tracks.len(),
        clip_count,
        document.media_pool.len()
    );

    for track in &document.tracks {
        let kind = match track.kind {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        };
        let _ = writeln!(
            output,
            "track {} {kind} clips={}",
            track.id,
            track.clips.len()
        );
        for clip in &track.clips {
            let asset = document.asset(clip.asset);
            let duration = asset
                .and_then(|asset| {
                    map_source_range_to_project(
                        clip.source_range.clone(),
                        asset.fps,
                        document.fps,
                    )
                    .ok()
                })
                .unwrap_or(TimeCode::ZERO);
            let end = clip
                .timeline_start
                .checked_add(duration)
                .unwrap_or(clip.timeline_start);
            let asset_name = asset.map_or("<missing>", |asset| asset.name.as_str());
            let _ = writeln!(
                output,
                "  clip {} asset={} {:?} timeline={}..{} duration={} source={}..{}",
                clip.id,
                clip.asset,
                asset_name,
                frame_and_seconds(clip.timeline_start, document.fps),
                frame_and_seconds(end, document.fps),
                frame_and_seconds(duration, document.fps),
                source_frame_and_seconds(clip.source_range.start, asset.map(|asset| asset.fps)),
                source_frame_and_seconds(clip.source_range.end, asset.map(|asset| asset.fps)),
            );
        }
    }

    if !document.media_pool.is_empty() {
        output.push_str("assets:\n");
    }
    for asset in &document.media_pool {
        let resolution = asset
            .resolution
            .map_or_else(|| "audio-only".to_owned(), |(width, height)| {
                format!("{width}x{height}")
            });
        let _ = writeln!(
            output,
            "  asset {} {:?} kind={:?} duration={} fps={}/{} size={} path={:?}",
            asset.id,
            asset.name,
            asset.kind,
            frame_and_seconds(asset.duration, asset.fps),
            asset.fps.numerator(),
            asset.fps.denominator(),
            resolution,
            asset.path,
        );
    }
    if output.ends_with('\n') {
        output.pop();
    }
    output
}

pub fn render_clip_info(document: &Document, clip_id: ClipId) -> Result<String, String> {
    let (track, clip) = document
        .tracks
        .iter()
        .find_map(|track| {
            track
                .clips
                .iter()
                .find(|clip| clip.id == clip_id)
                .map(|clip| (track, clip))
        })
        .ok_or_else(|| format!("clip {clip_id} does not exist"))?;
    let asset = document
        .asset(clip.asset)
        .ok_or_else(|| format!("asset {} does not exist", clip.asset))?;
    let duration = map_source_range_to_project(
        clip.source_range.clone(),
        asset.fps,
        document.fps,
    )
    .map_err(|error| error.to_string())?;
    let end = clip
        .timeline_start
        .checked_add(duration)
        .ok_or_else(|| "time calculation overflowed".to_owned())?;
    Ok(format!(
        "clip {}\ntrack={} kind={:?}\nasset={} {:?}\ntimeline={}..{} duration={}\nsource={}..{} duration={}\neffects={} transition_in={}",
        clip.id,
        track.id,
        track.kind,
        asset.id,
        asset.name,
        frame_and_seconds(clip.timeline_start, document.fps),
        frame_and_seconds(end, document.fps),
        frame_and_seconds(duration, document.fps),
        frame_and_seconds(clip.source_range.start, asset.fps),
        frame_and_seconds(clip.source_range.end, asset.fps),
        frame_and_seconds(
            clip.source_range
                .end
                .checked_sub(clip.source_range.start)
                .unwrap_or(TimeCode::ZERO),
            asset.fps,
        ),
        clip.effects.len(),
        clip.transition_in
            .as_ref()
            .map_or("none".to_owned(), |transition| transition.name.clone()),
    ))
}

fn frame_and_seconds(frame: TimeCode, fps: Rational) -> String {
    let seconds = (frame.0 as f64) * f64::from(fps.denominator())
        / f64::from(fps.numerator());
    format!("{}f/{seconds:.3}s", frame.0)
}

fn source_frame_and_seconds(frame: TimeCode, fps: Option<Rational>) -> String {
    fps.map_or_else(|| format!("{}f/?s", frame.0), |fps| frame_and_seconds(frame, fps))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use openreel_core::{
        AssetId, Clip, MediaAsset, MediaKind, Track, TrackId,
    };

    use super::*;

    fn fixture() -> Document {
        Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Video,
                clips: vec![
                    Clip {
                        id: ClipId(10),
                        asset: AssetId(4),
                        source_range: TimeCode(30)..TimeCode(120),
                        timeline_start: TimeCode(0),
                        effects: Vec::new(),
                        transition_in: None,
                    },
                    Clip {
                        id: ClipId(11),
                        asset: AssetId(4),
                        source_range: TimeCode(150)..TimeCode(210),
                        timeline_start: TimeCode(120),
                        effects: Vec::new(),
                        transition_in: None,
                    },
                ],
            }],
            media_pool: vec![MediaAsset {
                id: AssetId(4),
                path: PathBuf::from("fixtures/interview.mp4"),
                name: "interview.mp4".to_owned(),
                duration: TimeCode(300),
                fps: Rational::new(30, 1).unwrap(),
                kind: MediaKind::AudioVideo,
                resolution: Some((1_920, 1_080)),
            }],
            fps: Rational::new(30, 1).unwrap(),
            resolution: (1_920, 1_080),
            duration: TimeCode(180),
        }
    }

    #[test]
    fn timeline_state_matches_the_compact_golden_rendering() {
        let expected = r#"project fps=30/1 size=1920x1080 duration=180f/6.000s
tracks=1 clips=2 assets=1
track 7 video clips=2
  clip 10 asset=4 "interview.mp4" timeline=0f/0.000s..90f/3.000s duration=90f/3.000s source=30f/1.000s..120f/4.000s
  clip 11 asset=4 "interview.mp4" timeline=120f/4.000s..180f/6.000s duration=60f/2.000s source=150f/5.000s..210f/7.000s
assets:
  asset 4 "interview.mp4" kind=AudioVideo duration=300f/10.000s fps=30/1 size=1920x1080 path="fixtures/interview.mp4""#;
        assert_eq!(render_timeline_state(&fixture()), expected);
    }

    #[test]
    fn clip_info_includes_project_and_source_time_bases() {
        let rendered = render_clip_info(&fixture(), ClipId(10)).unwrap();
        assert!(rendered.contains("timeline=0f/0.000s..90f/3.000s"));
        assert!(rendered.contains("source=30f/1.000s..120f/4.000s"));
    }
}

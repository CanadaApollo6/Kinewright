use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    sync::Arc,
};

use openreel_core::{
    AssetId, AssetTranscript, ClipContent, ClipId, Document, Effect, FrameRounding, LinkId,
    ParamValue, Rational, SceneStatus, SilenceSpan, SilenceStatus, TimeCode, TimelineSceneChange,
    TimelineSilenceSpan, TimelineTranscriptWord, Title, TrackKind, TranscriptStatus,
    map_frames_with_rounding, map_source_range_to_project,
};

use crate::shrink_silence_span_for_cutting_with_transcript;

#[must_use]
// Debug formatting keeps asset paths quoted and escaped in the stable text protocol.
#[allow(clippy::too_many_lines, clippy::unnecessary_debug_formatting)]
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
        "tracks={} clips={} assets={} markers={} link_groups={} bins={} string_outs={} sync_groups={}",
        document.tracks.len(),
        clip_count,
        document.media_pool.len(),
        document.markers.len(),
        link_groups(document).len(),
        document.catalog.bins.len(),
        document.catalog.string_outs.len(),
        document.catalog.sync_groups.len(),
    );

    for track in &document.tracks {
        let kind = match track.kind {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        };
        let _ = writeln!(
            output,
            "track {} {kind} sync_lock={} clips={}",
            track.id,
            track.sync_lock,
            track.clips.len()
        );
        let caption_clips = track
            .clips
            .iter()
            .filter(|clip| {
                matches!(
                    &clip.content,
                    ClipContent::Title(title) if title.caption_preset.is_some()
                )
            })
            .collect::<Vec<_>>();
        if !caption_clips.is_empty() {
            let clip_ids = caption_clips
                .iter()
                .map(|clip| clip.id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let presets = caption_clips
                .iter()
                .filter_map(|clip| match &clip.content {
                    ClipContent::Title(title) => title.caption_preset,
                    _ => None,
                })
                .map(openreel_core::CaptionPreset::as_str)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",");
            let motions = caption_clips
                .iter()
                .map(|clip| caption_motion_name(&clip.effects))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",");
            let start = caption_clips
                .iter()
                .map(|clip| clip.timeline_start)
                .min()
                .unwrap_or(TimeCode::ZERO);
            let end = caption_clips
                .iter()
                .filter_map(|clip| {
                    document
                        .clip_duration(clip)
                        .ok()
                        .and_then(|duration| clip.timeline_start.checked_add(duration))
                })
                .max()
                .unwrap_or(start);
            let _ = writeln!(
                output,
                "  captions cues={} clip_ids={} timeline={}..{} presets={} motions={}",
                caption_clips.len(),
                clip_ids,
                frame_and_seconds(start, document.fps),
                frame_and_seconds(end, document.fps),
                presets,
                motions,
            );
        }
        for clip in &track.clips {
            if let ClipContent::Freeze(freeze) = &clip.content {
                let duration = document.clip_duration(clip).unwrap_or(TimeCode::ZERO);
                let end = clip
                    .timeline_start
                    .checked_add(duration)
                    .unwrap_or(clip.timeline_start);
                let asset = document.asset(clip.asset);
                let asset_name = asset.map_or("<missing>", |asset| asset.name.as_str());
                let _ = writeln!(
                    output,
                    "  clip {} freeze asset={} {:?} source_frame={} timeline={}..{} duration={} effects={} transition_in={}",
                    clip.id,
                    clip.asset,
                    asset_name,
                    source_frame_and_seconds(freeze.source_frame, asset.map(|asset| asset.fps),),
                    frame_and_seconds(clip.timeline_start, document.fps),
                    frame_and_seconds(end, document.fps),
                    frame_and_seconds(duration, document.fps),
                    render_effects(&clip.effects),
                    render_transition(clip.transition_in.as_ref()),
                );
                continue;
            }
            if let ClipContent::Title(title) = &clip.content {
                if title.caption_preset.is_some() {
                    continue;
                }
                let duration = document.clip_duration(clip).unwrap_or(TimeCode::ZERO);
                let end = clip
                    .timeline_start
                    .checked_add(duration)
                    .unwrap_or(clip.timeline_start);
                let _ = writeln!(
                    output,
                    "  clip {} title={} timeline={}..{} duration={} params={} effects={} transition_in={}",
                    clip.id,
                    title.text.escape_debug(),
                    frame_and_seconds(clip.timeline_start, document.fps),
                    frame_and_seconds(end, document.fps),
                    frame_and_seconds(duration, document.fps),
                    render_title(title),
                    render_effects(&clip.effects),
                    render_transition(clip.transition_in.as_ref()),
                );
                continue;
            }
            let asset = document.asset(clip.asset);
            let duration = asset
                .and_then(|asset| {
                    let effective = openreel_core::clip_effective_fps(asset.fps, clip).ok()?;
                    map_source_range_to_project(clip.source_range.clone(), effective, document.fps)
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
                "  clip {} asset={} {:?} timeline={}..{} duration={} source={}..{} effects={} transition_in={}{}{}",
                clip.id,
                clip.asset,
                asset_name,
                frame_and_seconds(clip.timeline_start, document.fps),
                frame_and_seconds(end, document.fps),
                frame_and_seconds(duration, document.fps),
                source_frame_and_seconds(clip.source_range.start, asset.map(|asset| asset.fps)),
                source_frame_and_seconds(clip.source_range.end, asset.map(|asset| asset.fps)),
                render_effects(&clip.effects),
                render_transition(clip.transition_in.as_ref()),
                render_clip_audio(clip),
                render_clip_speed(clip),
            );
        }
    }

    render_links_and_markers(&mut output, document);

    if !document.media_pool.is_empty() {
        output.push_str("assets:\n");
    }
    for asset in &document.media_pool {
        let resolution = asset.resolution.map_or_else(
            || "audio-only".to_owned(),
            |(width, height)| format!("{width}x{height}"),
        );
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
    render_catalog(&mut output, document);
    render_audio_mix(&mut output, document);
    if output.ends_with('\n') {
        output.pop();
    }
    output
}

fn caption_motion_name(effects: &[Effect]) -> &'static str {
    let has_opacity = effects.iter().any(|effect| effect.name == "opacity");
    for effect in effects.iter().filter(|effect| effect.name == "transform") {
        if effect.keyframes.contains_key("scale_percent") {
            return "pop";
        }
        if effect.keyframes.contains_key("y_percent") {
            return "slide_up";
        }
    }
    if has_opacity { "fade" } else { "none" }
}

fn render_links_and_markers(output: &mut String, document: &Document) {
    let links = link_groups(document);
    if !links.is_empty() {
        output.push_str("links:\n");
        for (link, clips) in links {
            let clips = clips
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let _ = writeln!(output, "  link {link} clips={clips}");
        }
    }
    if !document.markers.is_empty() {
        output.push_str("markers:\n");
        for marker in &document.markers {
            let _ = writeln!(
                output,
                "  marker {} at={} color={} label={:?}",
                marker.id,
                frame_and_seconds(marker.position, document.fps),
                marker.color_token,
                marker.label,
            );
        }
    }
}

/// Render detailed state for one clip.
///
/// # Errors
///
/// Returns an error string when the clip or its referenced asset is missing.
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
    if let ClipContent::Title(title) = &clip.content {
        let duration = document
            .clip_duration(clip)
            .map_err(|error| error.to_string())?;
        let end = clip
            .timeline_start
            .checked_add(duration)
            .ok_or_else(|| "time calculation overflowed".to_owned())?;
        return Ok(format!(
            "clip {}\ntrack={} kind={:?}\ncontent=title\nlink={}\ntimeline={}..{} duration={}\ntitle={}\neffects={}\ntransition_in={}",
            clip.id,
            track.id,
            track.kind,
            clip.link
                .map_or_else(|| "none".to_owned(), |link| link.to_string()),
            frame_and_seconds(clip.timeline_start, document.fps),
            frame_and_seconds(end, document.fps),
            frame_and_seconds(duration, document.fps),
            render_title(title),
            render_effects(&clip.effects),
            render_transition(clip.transition_in.as_ref()),
        ));
    }
    if let ClipContent::Freeze(freeze) = &clip.content {
        let asset = document
            .asset(clip.asset)
            .ok_or_else(|| format!("asset {} does not exist", clip.asset))?;
        let duration = document
            .clip_duration(clip)
            .map_err(|error| error.to_string())?;
        let end = clip
            .timeline_start
            .checked_add(duration)
            .ok_or_else(|| "time calculation overflowed".to_owned())?;
        return Ok(format!(
            "clip {}\ntrack={} kind={:?}\ncontent=freeze\nasset={} {:?}\nlink={}\ntimeline={}..{} duration={}\nsource_frame={}\neffects={}\ntransition_in={}",
            clip.id,
            track.id,
            track.kind,
            asset.id,
            asset.name,
            clip.link
                .map_or_else(|| "none".to_owned(), |link| link.to_string()),
            frame_and_seconds(clip.timeline_start, document.fps),
            frame_and_seconds(end, document.fps),
            frame_and_seconds(duration, document.fps),
            frame_and_seconds(freeze.source_frame, asset.fps),
            render_effects(&clip.effects),
            render_transition(clip.transition_in.as_ref()),
        ));
    }
    let asset = document
        .asset(clip.asset)
        .ok_or_else(|| format!("asset {} does not exist", clip.asset))?;
    let effective_fps =
        openreel_core::clip_effective_fps(asset.fps, clip).map_err(|error| error.to_string())?;
    let duration =
        map_source_range_to_project(clip.source_range.clone(), effective_fps, document.fps)
            .map_err(|error| error.to_string())?;
    let end = clip
        .timeline_start
        .checked_add(duration)
        .ok_or_else(|| "time calculation overflowed".to_owned())?;
    Ok(format!(
        "clip {}\ntrack={} kind={:?}\nasset={} {:?}\nlink={}\ntimeline={}..{} duration={}\nsource={}..{} duration={}\neffects={}\ntransition_in={}{}",
        clip.id,
        track.id,
        track.kind,
        asset.id,
        asset.name,
        clip.link
            .map_or_else(|| "none".to_owned(), |link| link.to_string()),
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
        render_effects(&clip.effects),
        render_transition(clip.transition_in.as_ref()),
        render_clip_speed(clip),
    ))
}

fn render_title(title: &Title) -> String {
    format!(
        "text={:?} font_size_token={} color_token={} position={} scrim={} fade_in_frames={} fade_out_frames={}",
        title.text,
        title.font_size_token,
        title.color_token,
        title.position.as_str(),
        title.background_scrim,
        title.fade_in_frames,
        title.fade_out_frames,
    )
}

fn render_clip_audio(clip: &openreel_core::Clip) -> String {
    if clip.audio_gain_tenth_db == 0
        && clip.audio_fade_in_frames == TimeCode::ZERO
        && clip.audio_fade_out_frames == TimeCode::ZERO
    {
        String::new()
    } else {
        format!(
            " audio=gain:{},fade_in:{}f,fade_out:{}f",
            clip.audio_gain_tenth_db, clip.audio_fade_in_frames.0, clip.audio_fade_out_frames.0
        )
    }
}

fn render_clip_speed(clip: &openreel_core::Clip) -> String {
    if clip.speed_percent == 100 {
        String::new()
    } else {
        format!(" speed={}% (audio muted)", clip.speed_percent)
    }
}

fn link_groups(document: &Document) -> BTreeMap<LinkId, Vec<ClipId>> {
    let mut groups = BTreeMap::<LinkId, Vec<ClipId>>::new();
    for clip in document.tracks.iter().flat_map(|track| &track.clips) {
        if let Some(link) = clip.link {
            groups.entry(link).or_default().push(clip.id);
        }
    }
    groups
}

#[must_use]
pub fn render_asset_transcript(asset: AssetId, status: &TranscriptStatus) -> String {
    match status {
        TranscriptStatus::NotRequested => {
            format!("asset {asset} transcript status=not-requested")
        }
        TranscriptStatus::Queued => format!("asset {asset} transcript status=queued"),
        TranscriptStatus::Hashing => format!("asset {asset} transcript status=hashing"),
        TranscriptStatus::DownloadingModel {
            downloaded_bytes,
            total_bytes,
        } => total_bytes.map_or_else(
            || {
                format!(
                    "asset {asset} transcript status=downloading-model bytes={downloaded_bytes}"
                )
            },
            |total| {
                format!(
                    "asset {asset} transcript status=downloading-model bytes={downloaded_bytes}/{total}"
                )
            },
        ),
        TranscriptStatus::Transcribing { progress_percent } => {
            format!("asset {asset} transcript status=transcribing progress={progress_percent}%")
        }
        TranscriptStatus::NoSpeech => format!("asset {asset} transcript: no speech found"),
        TranscriptStatus::Cancelled => format!("asset {asset} transcript status=cancelled"),
        TranscriptStatus::Failed(error) => {
            format!("asset {asset} transcript status=failed error={error:?}")
        }
        TranscriptStatus::Ready(transcript) => {
            let mut output = format!(
                "asset {asset} transcript fps={}/{} words={}\n",
                transcript.source_fps.numerator(),
                transcript.source_fps.denominator(),
                transcript.words.len()
            );
            for word in &transcript.words {
                let _ = writeln!(
                    output,
                    "{}..{}{} {:?}",
                    frame_and_seconds(word.source_start, transcript.source_fps),
                    frame_and_seconds(word.source_end, transcript.source_fps),
                    render_speaker(word.speaker.as_deref()),
                    word.text
                );
            }
            output.pop();
            output
        }
    }
}

#[must_use]
pub fn render_timeline_transcript(
    document: &Document,
    range: std::ops::Range<TimeCode>,
    words: &[TimelineTranscriptWord],
) -> String {
    let mut output = format!(
        "timeline transcript range={}..{} words={}\n",
        frame_and_seconds(range.start, document.fps),
        frame_and_seconds(range.end, document.fps),
        words.len()
    );
    for word in words {
        let source_fps = document
            .asset(word.asset)
            .map_or(word_project_fallback_fps(document), |asset| asset.fps);
        let _ = writeln!(
            output,
            "clip={} asset={} project={}..{} source={}..{}{} {:?}",
            word.clip,
            word.asset,
            frame_and_seconds(word.project_start, document.fps),
            frame_and_seconds(word.project_end, document.fps),
            frame_and_seconds(word.source_start, source_fps),
            frame_and_seconds(word.source_end, source_fps),
            render_speaker(word.speaker.as_deref()),
            word.text
        );
    }
    output.pop();
    output
}

fn render_catalog(output: &mut String, document: &Document) {
    if !document.catalog.bins.is_empty() {
        output.push_str("bins:\n");
        for bin in &document.catalog.bins {
            let assets = bin
                .assets
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let parent = bin
                .parent
                .map_or_else(|| "root".to_owned(), |parent| parent.to_string());
            let _ = writeln!(
                output,
                "  bin {} {:?} parent={} assets={}",
                bin.id, bin.name, parent, assets
            );
        }
    }
    if !document.catalog.string_outs.is_empty() {
        output.push_str("string_outs:\n");
        for string_out in &document.catalog.string_outs {
            let _ = writeln!(
                output,
                "  string_out {} {:?} selects={}",
                string_out.id,
                string_out.name,
                string_out.selects.len()
            );
            for select in &string_out.selects {
                let _ = writeln!(
                    output,
                    "    asset={} source={}..{} label={:?}",
                    select.asset, select.source.start, select.source.end, select.label
                );
            }
        }
    }
    if !document.catalog.sync_groups.is_empty() {
        output.push_str("sync_groups:\n");
        for group in &document.catalog.sync_groups {
            let _ = writeln!(
                output,
                "  sync_group {} {:?} members={}",
                group.id,
                group.name,
                group.members.len()
            );
            for member in &group.members {
                let _ = writeln!(
                    output,
                    "    asset={} offset={} angle={:?}",
                    member.asset, member.offset, member.angle_name
                );
            }
        }
    }
}

fn render_audio_mix(output: &mut String, document: &Document) {
    if document.audio_mix.buses.is_empty() {
        return;
    }
    output.push_str("audio_buses:\n");
    for bus in &document.audio_mix.buses {
        let tracks = bus
            .tracks
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let sidechain = bus
            .ducking_sidechain_tracks
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(
            output,
            "  audio_bus {} {:?} tracks={} sidechain={} effects={}",
            bus.id,
            bus.name,
            tracks,
            if sidechain.is_empty() {
                "none"
            } else {
                &sidechain
            },
            render_effects(&bus.effects),
        );
    }
}

fn render_speaker(speaker: Option<&str>) -> String {
    speaker.map_or_else(String::new, |speaker| format!(" speaker={speaker:?}"))
}

#[must_use]
pub fn render_asset_silences(
    asset: AssetId,
    status: &SilenceStatus,
    minimum_duration: TimeCode,
    transcript: Option<&AssetTranscript>,
) -> String {
    match status {
        SilenceStatus::NotRequested => format!("asset {asset} silences status=not-requested"),
        SilenceStatus::Queued => format!("asset {asset} silences status=queued"),
        SilenceStatus::Hashing => format!("asset {asset} silences status=hashing"),
        SilenceStatus::Analyzing => format!("asset {asset} silences status=analyzing"),
        SilenceStatus::NoAudio => format!("asset {asset} silences: no audio stream"),
        SilenceStatus::Cancelled => format!("asset {asset} silences status=cancelled"),
        SilenceStatus::Failed(error) => {
            format!("asset {asset} silences status=failed error={error:?}")
        }
        SilenceStatus::Ready(silences) => {
            let spans = silences
                .spans
                .iter()
                .filter(|span| {
                    span.source_end.0.saturating_sub(span.source_start.0) >= minimum_duration.0
                })
                .flat_map(|span| {
                    shrink_silence_span_for_cutting_with_transcript(
                        *span,
                        silences.source_fps,
                        transcript.map(|transcript| transcript.words.as_slice()),
                    )
                })
                .collect::<Vec<_>>();
            let mut output = format!(
                "asset {asset} silences fps={}/{} threshold={:.2}dBFS min_duration={} spans={}\n",
                silences.source_fps.numerator(),
                silences.source_fps.denominator(),
                f64::from(silences.threshold_dbfs_hundredths) / 100.0,
                frame_and_seconds(minimum_duration, silences.source_fps),
                spans.len()
            );
            for span in spans {
                let duration = TimeCode(span.source_end.0.saturating_sub(span.source_start.0));
                let _ = writeln!(
                    output,
                    "{}..{} duration={}",
                    frame_and_seconds(span.source_start, silences.source_fps),
                    frame_and_seconds(span.source_end, silences.source_fps),
                    frame_and_seconds(duration, silences.source_fps),
                );
            }
            output.pop();
            output
        }
    }
}

#[must_use]
pub fn render_timeline_silences(
    document: &Document,
    range: std::ops::Range<TimeCode>,
    spans: &[TimelineSilenceSpan],
    transcripts: &BTreeMap<AssetId, Arc<AssetTranscript>>,
    minimum_source_frames: TimeCode,
) -> String {
    let spans = cuttable_timeline_silences(document, spans, transcripts, minimum_source_frames)
        .into_iter()
        .filter(|span| span.project_end > range.start && span.project_start < range.end)
        .collect::<Vec<_>>();
    let mut output = format!(
        "timeline silences range={}..{} min_duration={} spans={}\n",
        frame_and_seconds(range.start, document.fps),
        frame_and_seconds(range.end, document.fps),
        minimum_source_frames.0,
        spans.len()
    );
    for span in &spans {
        let source_fps = document
            .asset(span.asset)
            .map_or(document.fps, |asset| asset.fps);
        let _ = writeln!(
            output,
            "clip={} asset={} project={}..{} source={}..{}",
            span.clip,
            span.asset,
            frame_and_seconds(span.project_start, document.fps),
            frame_and_seconds(span.project_end, document.fps),
            frame_and_seconds(span.source_start, source_fps),
            frame_and_seconds(span.source_end, source_fps),
        );
    }
    output.pop();
    output
}

#[must_use]
pub fn cuttable_timeline_silences(
    document: &Document,
    spans: &[TimelineSilenceSpan],
    transcripts: &BTreeMap<AssetId, Arc<AssetTranscript>>,
    minimum_source_frames: TimeCode,
) -> Vec<TimelineSilenceSpan> {
    let minimum = minimum_source_frames.0.max(1);
    let mut cuttable = spans
        .iter()
        .flat_map(|span| {
            clamped_timeline_silences(
                document,
                *span,
                transcripts.get(&span.asset).map(Arc::as_ref),
            )
        })
        .filter(|span| span.source_end.0.saturating_sub(span.source_start.0) >= minimum)
        .collect::<Vec<_>>();
    cuttable.sort_by_key(|span| (span.project_start, span.track, span.clip, span.source_start));
    cuttable
}

fn clamped_timeline_silences(
    document: &Document,
    span: TimelineSilenceSpan,
    transcript: Option<&AssetTranscript>,
) -> Vec<TimelineSilenceSpan> {
    let Some(asset) = document.asset(span.asset) else {
        return Vec::new();
    };
    let Some(clip) = document.clip(span.clip) else {
        return Vec::new();
    };
    shrink_silence_span_for_cutting_with_transcript(
        SilenceSpan {
            source_start: span.source_start,
            source_end: span.source_end,
        },
        asset.fps,
        transcript.map(|transcript| transcript.words.as_slice()),
    )
    .into_iter()
    .filter_map(|clamped| {
        let start_offset = clamped.source_start.checked_sub(clip.source_range.start)?;
        let end_offset = clamped.source_end.checked_sub(clip.source_range.start)?;
        let project_start = clip.timeline_start.checked_add(
            map_frames_with_rounding(start_offset, asset.fps, document.fps, FrameRounding::Floor)
                .ok()?,
        )?;
        let project_end = clip.timeline_start.checked_add(
            map_frames_with_rounding(end_offset, asset.fps, document.fps, FrameRounding::Ceil)
                .ok()?,
        )?;
        (project_end > project_start).then_some(TimelineSilenceSpan {
            source_start: clamped.source_start,
            source_end: clamped.source_end,
            project_start,
            project_end,
            ..span
        })
    })
    .collect()
}

#[must_use]
pub fn render_asset_scene_changes(
    asset: AssetId,
    status: &SceneStatus,
    minimum_confidence_basis_points: u16,
) -> String {
    match status {
        SceneStatus::NotRequested => format!("asset {asset} scene changes status=not-requested"),
        SceneStatus::Queued => format!("asset {asset} scene changes status=queued"),
        SceneStatus::Hashing => format!("asset {asset} scene changes status=hashing"),
        SceneStatus::Analyzing => format!("asset {asset} scene changes status=analyzing"),
        SceneStatus::NoVideo => format!("asset {asset} scene changes: no video stream"),
        SceneStatus::Cancelled => format!("asset {asset} scene changes status=cancelled"),
        SceneStatus::Failed(error) => {
            format!("asset {asset} scene changes status=failed error={error:?}")
        }
        SceneStatus::Ready(scenes) => {
            let changes = scenes
                .changes
                .iter()
                .filter(|change| change.confidence_basis_points >= minimum_confidence_basis_points)
                .collect::<Vec<_>>();
            let mut output = format!(
                "asset {asset} scene changes fps={}/{} min_confidence={:.2}% boundaries={}\n",
                scenes.source_fps.numerator(),
                scenes.source_fps.denominator(),
                f64::from(minimum_confidence_basis_points) / 100.0,
                changes.len()
            );
            for change in changes {
                let _ = writeln!(
                    output,
                    "{} confidence={:.2}%",
                    frame_and_seconds(change.source_frame, scenes.source_fps),
                    f64::from(change.confidence_basis_points) / 100.0,
                );
            }
            output.pop();
            output
        }
    }
}

#[must_use]
pub fn render_timeline_scene_changes(
    document: &Document,
    range: std::ops::Range<TimeCode>,
    changes: &[TimelineSceneChange],
) -> String {
    let mut output = format!(
        "timeline scene changes range={}..{} boundaries={}\n",
        frame_and_seconds(range.start, document.fps),
        frame_and_seconds(range.end, document.fps),
        changes.len()
    );
    for change in changes {
        let source_fps = document
            .asset(change.asset)
            .map_or(document.fps, |asset| asset.fps);
        let _ = writeln!(
            output,
            "clip={} asset={} project={} source={} confidence={:.2}%",
            change.clip,
            change.asset,
            frame_and_seconds(change.project_frame, document.fps),
            frame_and_seconds(change.source_frame, source_fps),
            f64::from(change.confidence_basis_points) / 100.0,
        );
    }
    output.pop();
    output
}

fn word_project_fallback_fps(document: &Document) -> Rational {
    document.fps
}

fn render_effects(effects: &[Effect]) -> String {
    if effects.is_empty() {
        return "none".to_owned();
    }
    let mut rendered = String::from("[");
    for (index, effect) in effects.iter().enumerate() {
        if index != 0 {
            rendered.push_str(", ");
        }
        let _ = write!(rendered, "{}:{}(", effect.id, effect.name);
        for (parameter_index, (name, value)) in effect.parameters.iter().enumerate() {
            if parameter_index != 0 {
                rendered.push(',');
            }
            let _ = write!(rendered, "{name}={}", render_param(value));
        }
        if !effect.keyframes.is_empty() {
            rendered.push_str("; keyframes=");
            for (curve_index, (name, curve)) in effect.keyframes.iter().enumerate() {
                if curve_index != 0 {
                    rendered.push('|');
                }
                let _ = write!(rendered, "{name}[");
                for (keyframe_index, keyframe) in curve.keyframes.iter().enumerate() {
                    if keyframe_index != 0 {
                        rendered.push(',');
                    }
                    let _ = write!(
                        rendered,
                        "{}:{}:{:?}",
                        keyframe.at.0, keyframe.value, keyframe.interpolation
                    );
                }
                rendered.push(']');
            }
        }
        rendered.push(')');
    }
    rendered.push(']');
    rendered
}

fn render_param(value: &ParamValue) -> String {
    match value {
        ParamValue::Integer(value) => value.to_string(),
        ParamValue::Boolean(value) => value.to_string(),
        ParamValue::Text(value) => format!("{value:?}"),
    }
}

fn render_transition(transition: Option<&openreel_core::Transition>) -> String {
    transition.map_or_else(
        || "none".to_owned(),
        |transition| format!("{}:{}f", transition.name, transition.duration.0),
    )
}

// Human-readable seconds are intentionally approximate while frame counts remain exact.
#[allow(clippy::cast_precision_loss)]
fn frame_and_seconds(frame: TimeCode, fps: Rational) -> String {
    let seconds = (frame.0 as f64) * f64::from(fps.denominator()) / f64::from(fps.numerator());
    format!("{}f/{seconds:.3}s", frame.0)
}

fn source_frame_and_seconds(frame: TimeCode, fps: Option<Rational>) -> String {
    fps.map_or_else(
        || format!("{}f/?s", frame.0),
        |fps| frame_and_seconds(frame, fps),
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

    use openreel_core::{
        AssetId, AssetSilences, AssetTranscript, CaptionCue, CaptionMotion, CaptionPreset, Clip,
        Effect, EffectId, LinkId, Marker, MarkerId, MediaAsset, MediaKind, ParamValue, SilenceSpan,
        TimelineTranscriptWord, Track, TrackId, TranscriptStatus, Transition,
        animated_caption_operations, apply_batch,
    };

    use super::*;

    fn fixture() -> Document {
        Document {
            catalog: openreel_core::MediaCatalog::default(),
            audio_mix: openreel_core::AudioMix::default(),
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![
                    Clip {
                        id: ClipId(10),
                        asset: AssetId(4),
                        source_range: TimeCode(30)..TimeCode(120),
                        content: ClipContent::Media,
                        timeline_start: TimeCode(0),
                        effects: vec![Effect {
                            id: EffectId(3),
                            name: "brightness".to_owned(),
                            parameters: BTreeMap::from([(
                                "percent".to_owned(),
                                ParamValue::Integer(25),
                            )]),
                            keyframes: BTreeMap::new(),
                        }],
                        transition_in: Some(Transition {
                            name: "crossfade".to_owned(),
                            duration: TimeCode(15),
                        }),
                        link: Some(LinkId(2)),
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
                    },
                    Clip {
                        id: ClipId(11),
                        asset: AssetId(4),
                        source_range: TimeCode(150)..TimeCode(210),
                        content: ClipContent::Media,
                        timeline_start: TimeCode(120),
                        effects: Vec::new(),
                        transition_in: None,
                        link: Some(LinkId(2)),
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
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
            markers: vec![Marker {
                id: MarkerId(3),
                position: TimeCode(45),
                label: "Check reaction".to_owned(),
                color_token: 0,
            }],
            fps: Rational::new(30, 1).unwrap(),
            resolution: (1_920, 1_080),
            duration: TimeCode(180),
        }
    }

    #[test]
    fn timeline_state_matches_the_compact_golden_rendering() {
        let expected = r#"project fps=30/1 size=1920x1080 duration=180f/6.000s
tracks=1 clips=2 assets=1 markers=1 link_groups=1 bins=0 string_outs=0 sync_groups=0
track 7 video sync_lock=true clips=2
  clip 10 asset=4 "interview.mp4" timeline=0f/0.000s..90f/3.000s duration=90f/3.000s source=30f/1.000s..120f/4.000s effects=[3:brightness(percent=25)] transition_in=crossfade:15f
  clip 11 asset=4 "interview.mp4" timeline=120f/4.000s..180f/6.000s duration=60f/2.000s source=150f/5.000s..210f/7.000s effects=none transition_in=none
links:
  link 2 clips=10,11
markers:
  marker 3 at=45f/1.500s color=0 label="Check reaction"
assets:
  asset 4 "interview.mp4" kind=AudioVideo duration=300f/10.000s fps=30/1 size=1920x1080 path="fixtures/interview.mp4""#;
        assert_eq!(render_timeline_state(&fixture()), expected);
    }

    #[test]
    fn timeline_state_collapses_generated_caption_tracks() {
        let mut document = Document::default();
        let cues = (0..12)
            .map(|index| CaptionCue {
                start: TimeCode(index * 30),
                end: TimeCode(index * 30 + 30),
                text: format!("Caption number {index} should not repeat in state"),
            })
            .collect::<Vec<_>>();
        let operations = animated_caption_operations(
            &document,
            &cues,
            CaptionPreset::Social,
            CaptionMotion::Pop,
        )
        .unwrap();
        apply_batch(&mut document, &operations).unwrap();

        let rendered = render_timeline_state(&document);
        assert!(rendered.contains("captions cues=12 clip_ids=1,2,3,4,5,6,7,8,9,10,11,12"));
        assert!(rendered.contains("presets=social motions=pop"));
        assert!(!rendered.contains("Caption number"));
        assert!(
            rendered.len() < 600,
            "caption state grew to {} bytes",
            rendered.len()
        );
    }

    #[test]
    fn clip_info_includes_project_and_source_time_bases() {
        let rendered = render_clip_info(&fixture(), ClipId(10)).unwrap();
        assert!(rendered.contains("timeline=0f/0.000s..90f/3.000s"));
        assert!(rendered.contains("source=30f/1.000s..120f/4.000s"));
        assert!(rendered.contains("link=2"));
        assert!(rendered.contains("effects=[3:brightness(percent=25)]"));
        assert!(rendered.contains("transition_in=crossfade:15f"));
    }

    #[test]
    fn timeline_state_appends_non_default_clip_audio_values() {
        let mut document = fixture();
        let clip = document
            .tracks
            .iter_mut()
            .flat_map(|track| &mut track.clips)
            .find(|clip| clip.id == ClipId(11))
            .unwrap();
        clip.audio_gain_tenth_db = -60;
        clip.audio_fade_in_frames = TimeCode(12);
        clip.audio_fade_out_frames = TimeCode::ZERO;
        let rendered = render_timeline_state(&document);
        assert!(rendered.contains(
            "clip 11 asset=4 \"interview.mp4\" timeline=120f/4.000s..180f/6.000s duration=60f/2.000s source=150f/5.000s..210f/7.000s effects=none transition_in=none audio=gain:-60,fade_in:12f,fade_out:0f"
        ));
    }

    #[test]
    fn timeline_state_appends_speed_and_scales_duration_for_speeded_clips() {
        let mut document = fixture();
        let clip = document
            .tracks
            .iter_mut()
            .flat_map(|track| &mut track.clips)
            .find(|clip| clip.id == ClipId(11))
            .unwrap();
        clip.speed_percent = 200;
        let rendered = render_timeline_state(&document);
        assert!(
            rendered.contains("clip 11") && rendered.contains("speed=200% (audio muted)"),
            "speeded clip must carry the speed suffix: {rendered}"
        );
        // Source 150..210 at doubled effective rate covers half the frames.
        assert!(
            rendered.contains("clip 11 asset=4 \"interview.mp4\" timeline=120f/4.000s..150f/5.000s duration=30f/1.000s"),
            "speeded clip duration must reflect the effective rate: {rendered}"
        );
        let info = render_clip_info(&document, ClipId(11)).unwrap();
        assert!(info.contains("speed=200% (audio muted)"));
    }

    #[test]
    fn timeline_state_and_clip_info_include_declarative_title_parameters() {
        let mut document = fixture();
        document.tracks.push(Track {
            id: TrackId(8),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![Clip {
                id: ClipId(12),
                asset: AssetId::default(),
                source_range: TimeCode(0)..TimeCode(60),
                content: ClipContent::Title(Title {
                    text: "Lower third".to_owned(),
                    font_size_token: 2,
                    color_token: 2,
                    position: openreel_core::TitlePosition::LowerThird,
                    background_scrim: false,
                    fade_in_frames: TimeCode(6),
                    fade_out_frames: TimeCode(9),
                    caption_preset: None,
                }),
                timeline_start: TimeCode(30),
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        });
        let timeline = render_timeline_state(&document);
        assert!(timeline.contains("track 8 video sync_lock=false clips=1"));
        assert!(timeline.contains("clip 12 title=Lower third"));
        assert!(timeline.contains(
            "text=\"Lower third\" font_size_token=2 color_token=2 position=lower_third scrim=false fade_in_frames=6 fade_out_frames=9"
        ));

        let info = render_clip_info(&document, ClipId(12)).unwrap();
        assert!(info.contains("content=title"));
        assert!(info.contains("timeline=30f/1.000s..90f/3.000s duration=60f/2.000s"));
        assert!(info.contains("position=lower_third"));
    }

    #[test]
    fn timeline_state_and_clip_info_have_freeze_specific_golden_rendering() {
        let mut document = fixture();
        document.tracks.push(Track {
            id: TrackId(8),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![Clip {
                id: ClipId(12),
                asset: AssetId(4),
                source_range: TimeCode(0)..TimeCode(60),
                content: ClipContent::Freeze(openreel_core::FreezeFrame {
                    source_frame: TimeCode(45),
                }),
                timeline_start: TimeCode(30),
                effects: vec![Effect {
                    id: EffectId(4),
                    name: "crop".to_owned(),
                    parameters: BTreeMap::from([(
                        "top_percent".to_owned(),
                        ParamValue::Integer(10),
                    )]),
                    keyframes: BTreeMap::new(),
                }],
                transition_in: Some(Transition {
                    name: "crossfade".to_owned(),
                    duration: TimeCode(6),
                }),
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        });

        let timeline = render_timeline_state(&document);
        let timeline_line = timeline
            .lines()
            .find(|line| line.contains("clip 12 freeze"))
            .unwrap();
        assert_eq!(
            timeline_line,
            "  clip 12 freeze asset=4 \"interview.mp4\" source_frame=45f/1.500s timeline=30f/1.000s..90f/3.000s duration=60f/2.000s effects=[4:crop(top_percent=10)] transition_in=crossfade:6f"
        );

        assert_eq!(
            render_clip_info(&document, ClipId(12)).unwrap(),
            "clip 12\ntrack=8 kind=Video\ncontent=freeze\nasset=4 \"interview.mp4\"\nlink=none\ntimeline=30f/1.000s..90f/3.000s duration=60f/2.000s\nsource_frame=45f/1.500s\neffects=[4:crop(top_percent=10)]\ntransition_in=crossfade:6f"
        );
    }

    #[test]
    fn asset_transcript_matches_the_fixture_golden_rendering() {
        let transcript: AssetTranscript =
            serde_json::from_str(include_str!("../tests/fixtures/transcript.json")).unwrap();
        let rendered = render_asset_transcript(
            transcript.asset,
            &TranscriptStatus::Ready(std::sync::Arc::new(transcript)),
        );
        let expected = r#"asset 4 transcript fps=30/1 words=3
30f/1.000s..36f/1.200s "Hello"
39f/1.300s..45f/1.500s "um"
48f/1.600s..60f/2.000s "world.""#;
        assert_eq!(rendered, expected);
    }

    #[test]
    fn timeline_transcript_matches_the_fixture_golden_rendering() {
        let words = vec![
            TimelineTranscriptWord {
                text: "Hello".to_owned(),
                speaker: None,
                asset: AssetId(4),
                track: TrackId(7),
                clip: ClipId(10),
                source_start: TimeCode(30),
                source_end: TimeCode(36),
                project_start: TimeCode(0),
                project_end: TimeCode(6),
            },
            TimelineTranscriptWord {
                text: "um".to_owned(),
                speaker: None,
                asset: AssetId(4),
                track: TrackId(7),
                clip: ClipId(10),
                source_start: TimeCode(39),
                source_end: TimeCode(45),
                project_start: TimeCode(9),
                project_end: TimeCode(15),
            },
        ];
        let rendered = render_timeline_transcript(&fixture(), TimeCode(0)..TimeCode(30), &words);
        let expected = r#"timeline transcript range=0f/0.000s..30f/1.000s words=2
clip=10 asset=4 project=0f/0.000s..6f/0.200s source=30f/1.000s..36f/1.200s "Hello"
clip=10 asset=4 project=9f/0.300s..15f/0.500s source=39f/1.300s..45f/1.500s "um""#;
        assert_eq!(rendered, expected);
    }

    #[test]
    fn asset_silence_rendering_pads_cut_spans_and_omits_vanishing_spans() {
        let status = SilenceStatus::Ready(Arc::new(AssetSilences {
            asset: AssetId(4),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(300),
            threshold_dbfs_hundredths: -4_000,
            window_milliseconds: 20,
            spans: vec![
                SilenceSpan {
                    source_start: TimeCode(33),
                    source_end: TimeCode(63),
                },
                SilenceSpan {
                    source_start: TimeCode(90),
                    source_end: TimeCode(96),
                },
            ],
        }));

        let rendered = render_asset_silences(AssetId(4), &status, TimeCode(6), None);
        let expected = r"asset 4 silences fps=30/1 threshold=-40.00dBFS min_duration=6f/0.200s spans=1
36f/1.200s..60f/2.000s duration=24f/0.800s";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn timeline_silence_rendering_pads_in_source_space_before_project_mapping() {
        let spans = vec![
            TimelineSilenceSpan {
                asset: AssetId(4),
                track: TrackId(7),
                clip: ClipId(10),
                source_start: TimeCode(33),
                source_end: TimeCode(63),
                project_start: TimeCode(3),
                project_end: TimeCode(33),
            },
            TimelineSilenceSpan {
                asset: AssetId(4),
                track: TrackId(7),
                clip: ClipId(10),
                source_start: TimeCode(90),
                source_end: TimeCode(96),
                project_start: TimeCode(60),
                project_end: TimeCode(66),
            },
        ];

        let rendered = render_timeline_silences(
            &fixture(),
            TimeCode(0)..TimeCode(90),
            &spans,
            &BTreeMap::new(),
            TimeCode(6),
        );
        let expected = r"timeline silences range=0f/0.000s..90f/3.000s min_duration=6 spans=1
clip=10 asset=4 project=6f/0.200s..30f/1.000s source=36f/1.200s..60f/2.000s";
        assert_eq!(rendered, expected);

        let filtered = render_timeline_silences(
            &fixture(),
            TimeCode(0)..TimeCode(90),
            &spans,
            &BTreeMap::new(),
            TimeCode(25),
        );
        assert!(filtered.ends_with("min_duration=25 spans=0"));
    }
}

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    sync::Arc,
};

use kinewright_core::{
    AssetId, AssetTranscript, AutomationCurve, ClipContent, ClipId, Document, Effect,
    FrameRounding, LinkId, NOISE_PROFILE_BAND_COUNT, NOISE_PROFILE_PARAMETER_NAMES,
    PROFILE_BAND_NEUTRAL_TENTH_DB, PanLaw, ParamValue, Rational, SceneStatus, SilenceSpan,
    SilenceStatus, TRACK_AUTOMATION_PARAMETERS, TimeCode, TimelineSceneChange, TimelineSilenceSpan,
    TimelineTranscriptWord, Title, TrackKind, TranscriptStatus, is_noise_profile_parameter,
    map_frames_with_rounding, map_source_range_to_project,
};

use crate::shrink_silence_span_for_cutting_with_transcript;

#[must_use]
// Debug formatting keeps asset paths quoted and escaped in the stable text protocol.
#[allow(clippy::too_many_lines, clippy::unnecessary_debug_formatting)]
pub fn render_timeline_state(document: &Document) -> String {
    let mut output = String::new();
    let visible_marker_count = document
        .markers
        .iter()
        .filter(|marker| !is_internal_marker_label(&marker.label))
        .count();
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
        visible_marker_count,
        link_groups(document).len(),
        document.catalog.bins.len(),
        document.catalog.string_outs.len(),
        document.catalog.sync_groups.len(),
    );

    for track in &document.tracks {
        let kind = track_kind_name(track.kind);
        let _ = writeln!(
            output,
            "track {} {kind} sync_lock={} clips={}{}",
            track.id,
            track.sync_lock,
            track.clips.len(),
            render_track_mix(document, track),
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
                .map(kinewright_core::CaptionPreset::as_str)
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
                    let effective = kinewright_core::clip_effective_fps(asset.fps, clip).ok()?;
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
    let visible_markers = document
        .markers
        .iter()
        .filter(|marker| !is_internal_marker_label(&marker.label))
        .collect::<Vec<_>>();
    if !visible_markers.is_empty() {
        output.push_str("markers:\n");
        for marker in visible_markers {
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

fn is_internal_marker_label(label: &str) -> bool {
    label.starts_with("__kinewright_")
}

/// Render detailed state for one clip.
///
/// # Errors
///
/// Returns an error string when the clip or its referenced asset is missing.
#[allow(clippy::too_many_lines)]
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
            "clip {}\ntrack={} kind={:?}\ncontent=title\nlink={}\ntimeline={}..{} duration={}\ntitle={}\neffects={}\ntransition_in={}{}",
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
            render_clip_info_track_mix(document, track),
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
            "clip {}\ntrack={} kind={:?}\ncontent=freeze\nasset={} {:?}\nlink={}\ntimeline={}..{} duration={}\nsource_frame={}\neffects={}\ntransition_in={}{}",
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
            render_clip_info_track_mix(document, track),
        ));
    }
    let asset = document
        .asset(clip.asset)
        .ok_or_else(|| format!("asset {} does not exist", clip.asset))?;
    let effective_fps =
        kinewright_core::clip_effective_fps(asset.fps, clip).map_err(|error| error.to_string())?;
    let duration =
        map_source_range_to_project(clip.source_range.clone(), effective_fps, document.fps)
            .map_err(|error| error.to_string())?;
    let end = clip
        .timeline_start
        .checked_add(duration)
        .ok_or_else(|| "time calculation overflowed".to_owned())?;
    Ok(format!(
        "clip {}\ntrack={} kind={:?}\nasset={} {:?}\nlink={}\ntimeline={}..{} duration={}\nsource={}..{} duration={}\neffects={}\ntransition_in={}{}{}",
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
        render_clip_info_track_mix(document, track),
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

/// AU4 §4.1 rule 78: the envelope joins the existing audio suffix, and its
/// presence alone is enough to print the whole suffix. A curve-free clip
/// renders exactly the bytes it did before AU4.
fn render_clip_audio(clip: &kinewright_core::Clip) -> String {
    if clip.audio_gain_tenth_db == 0
        && clip.audio_fade_in_frames == TimeCode::ZERO
        && clip.audio_fade_out_frames == TimeCode::ZERO
        && clip.audio_gain_curve.is_none()
    {
        String::new()
    } else {
        format!(
            " audio=gain:{},fade_in:{}f,fade_out:{}f{}",
            clip.audio_gain_tenth_db,
            clip.audio_fade_in_frames.0,
            clip.audio_fade_out_frames.0,
            render_named_curve(",envelope:", clip.audio_gain_curve.as_ref()),
        )
    }
}

/// AU4 §4.1: the two `mix=` keys, in the same order as
/// [`TRACK_AUTOMATION_PARAMETERS`], which is what rule 81's pin uses to keep
/// the wire tokens and the rendered keys from drifting apart.
const TRACK_AUTOMATION_RENDER_KEYS: [&str; TRACK_AUTOMATION_PARAMETERS.len()] =
    ["gain_curve", "pan_curve"];

/// AU4 §4.1 rule 81: the one mapping from a `set_track_automation` parameter
/// token to the key `get_timeline_state` spells it with.
///
/// Private until Part B needs it; every caller is in this file.
#[must_use]
fn track_automation_render_key(parameter: &str) -> Option<&'static str> {
    TRACK_AUTOMATION_PARAMETERS
        .iter()
        .position(|token| *token == parameter)
        .map(|index| TRACK_AUTOMATION_RENDER_KEYS[index])
}

/// AU4 §4.1: `{prefix}[at:value:Interp,…]` when the curve exists, and nothing
/// at all when it does not. Every AU4 render field is default-omitted through
/// this one helper, so no golden moves until a curve does.
fn render_named_curve(prefix: &str, curve: Option<&AutomationCurve>) -> String {
    curve.map_or_else(String::new, |curve| {
        format!("{prefix}{}", render_curve(curve))
    })
}

/// AU4 §4.1: `render_effects`' own keyframe spelling, hoisted so the clip,
/// track, bus and master renderings cannot drift from it.
fn render_curve(curve: &AutomationCurve) -> String {
    let mut rendered = String::from("[");
    for (index, keyframe) in curve.keyframes.iter().enumerate() {
        if index != 0 {
            rendered.push(',');
        }
        let _ = write!(
            rendered,
            "{}:{}:{:?}",
            keyframe.at.0, keyframe.value, keyframe.interpolation
        );
    }
    rendered.push(']');
    rendered
}

/// The one spelling of a track kind in every agent rendering. Shared with
/// `get_audio_levels` so the two tools cannot drift (AU1 §6.2).
pub(crate) const fn track_kind_name(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
    }
}

/// AU1 §6.2: the four mix fields, or `None` for a neutral track so every
/// pre-AU1 rendering is byte-identical. The single source of the literal that
/// both the timeline track line and the `render_clip_info` line embed.
fn track_mix_fields(document: &Document, track: &kinewright_core::Track) -> Option<String> {
    let mix = document.track_mix(track.id);
    (!mix.is_neutral()).then(|| {
        let curve_field = |parameter: &str, curve: Option<&AutomationCurve>| {
            let key = track_automation_render_key(parameter)
                .expect("every call site passes a TRACK_AUTOMATION_PARAMETERS token (rule 81)");
            render_named_curve(&format!(",{key}:"), curve)
        };
        format!(
            "gain:{},pan:{},mute:{},solo:{}{}{}",
            mix.gain_tenth_db,
            mix.pan_percent,
            mix.mute,
            mix.solo,
            curve_field(TRACK_AUTOMATION_PARAMETERS[0], mix.gain_curve.as_ref()),
            curve_field(TRACK_AUTOMATION_PARAMETERS[1], mix.pan_curve.as_ref()),
        )
    })
}

/// The track-line suffix. Shaped like [`render_clip_audio`].
fn render_track_mix(document: &Document, track: &kinewright_core::Track) -> String {
    track_mix_fields(document, track).map_or_else(String::new, |fields| format!(" mix={fields}"))
}

/// The [`render_clip_info`] form of [`render_track_mix`]: a whole final line,
/// absent when the owning track is neutral.
fn render_clip_info_track_mix(document: &Document, track: &kinewright_core::Track) -> String {
    track_mix_fields(document, track)
        .map_or_else(String::new, |fields| format!("\ntrack_mix={fields}"))
}

fn render_clip_speed(clip: &kinewright_core::Clip) -> String {
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

/// AU2 §6.3: the pan law, the buses, and the master chain, each rendered only
/// when it is not the default. A document with no buses, a neutral master and
/// the balance law renders nothing at all, so every pre-AU2 compact state is
/// byte-identical.
fn render_audio_mix(output: &mut String, document: &Document) {
    let mix = &document.audio_mix;
    if mix.buses.is_empty() && mix.master.is_neutral() && mix.pan_law.is_balance() {
        return;
    }
    if !mix.pan_law.is_balance() {
        let _ = writeln!(output, "audio_pan_law={}", pan_law_name(mix.pan_law));
    }
    if !mix.buses.is_empty() {
        output.push_str("audio_buses:\n");
    }
    for bus in &mix.buses {
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
            "  audio_bus {} {:?} tracks={}{}{} sidechain={} effects={}",
            bus.id,
            bus.name,
            tracks,
            render_audio_gain(bus.gain_tenth_db),
            render_named_curve(" gain_curve=", bus.gain_curve.as_ref()),
            if sidechain.is_empty() {
                "none"
            } else {
                &sidechain
            },
            render_effects(&bus.effects),
        );
    }
    if !mix.master.is_neutral() {
        let _ = writeln!(
            output,
            "audio_master gain={}{} effects={}",
            mix.master.gain_tenth_db,
            render_named_curve(" gain_curve=", mix.master.gain_curve.as_ref()),
            render_effects(&mix.master.effects),
        );
    }
}

/// AU2 §6.3: ` gain={tenth_db}`, or nothing at all at unity.
fn render_audio_gain(gain_tenth_db: i32) -> String {
    if gain_tenth_db == 0 {
        String::new()
    } else {
        format!(" gain={gain_tenth_db}")
    }
}

/// The one spelling of a pan law in the compact state, matching the serde
/// `snake_case` name the `set_pan_law` tool takes (AU2 §5.3).
const fn pan_law_name(law: PanLaw) -> &'static str {
    match law {
        PanLaw::Balance => "balance",
        PanLaw::ConstantPower => "constant_power",
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

/// The one descriptor that carries AU5 §2.1's 31 profile rows, named here as
/// `schema.rs` names it for the same reason: the compact special case stays
/// greppable from the module that implements it.
const DENOISE_EFFECT_NAME: &str = "audio_denoise";

/// Whether one parameter name is a row [`render_noise_profile`] will collect.
///
/// Core's [`is_noise_profile_parameter`] is the predicate AU5 §2.1 rule 7
/// names as the single definition every reader outside `effect.rs` uses, and
/// it matches an **unbounded** index; the block below collects exactly the 31
/// names in [`NOISE_PROFILE_PARAMETER_NAMES`]. Requiring both makes the
/// dropped set and the collected set the same set by construction, so a
/// `profile_band99_tenth_db` — unreachable today, because core's
/// `denoise_parameters()` builds the descriptor from that same table and the
/// domain check refuses anything else — would render as an ordinary row
/// rather than be dropped by one and missed by the other.
fn is_rendered_profile_band(name: &str) -> bool {
    is_noise_profile_parameter(name) && NOISE_PROFILE_PARAMETER_NAMES.contains(&name)
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
        let hatched = effect.name == DENOISE_EFFECT_NAME;
        let mut parameter_index = 0usize;
        for (name, value) in &effect.parameters {
            if hatched && is_rendered_profile_band(name) {
                continue;
            }
            if parameter_index != 0 {
                rendered.push(',');
            }
            parameter_index += 1;
            let _ = write!(rendered, "{name}={}", render_param(value));
        }
        if let Some(profile) = render_noise_profile(effect) {
            if parameter_index != 0 {
                rendered.push(',');
            }
            rendered.push_str(&profile);
        }
        if !effect.keyframes.is_empty() {
            rendered.push_str("; keyframes=");
            for (curve_index, (name, curve)) in effect.keyframes.iter().enumerate() {
                if curve_index != 0 {
                    rendered.push('|');
                }
                let _ = write!(rendered, "{name}{}", render_curve(curve));
            }
        }
        rendered.push(')');
    }
    rendered.push(']');
    rendered
}

/// AU5 §4.2 rule 79: `audio_denoise`'s 31 learned bands as one
/// `noise_profile=[…]` block, low band to high, or `None` when there is
/// nothing to say.
///
/// **Omitted entirely when every band is at the neutral or absent**, so an
/// unlearned node — which is every node the app inserts, because
/// `insert_audio_effect` skips the profile rows — renders exactly the bytes it
/// did before AU5 and no pre-AU5 golden moves.
///
/// It is its own arm rather than a `render_param` call on a joined string
/// because `render_param` renders `ParamValue::Text` through `{value:?}`, i.e.
/// quoted: the block would arrive at the reader wrapped in escapes. The values
/// inside it still go through `render_param`, which renders an integer bare,
/// so a band that somehow holds a non-integer is still published rather than
/// silently dropped by the loop above.
///
/// An absent band inside a learned profile renders as
/// [`PROFILE_BAND_NEUTRAL_TENTH_DB`] — a value the document does not hold —
/// which is safe only because AU5 §2.1's profile rows are a **write-all-31-or-
/// none** block: core validates that, `insert_audio_effect` skips all 31 at
/// once, and `mix_noise_profile` writes all 31 at once, so a document holding
/// some-but-not-all is already invalid. The fill is that rule made visible,
/// not an arbitrary choice.
fn render_noise_profile(effect: &Effect) -> Option<String> {
    if effect.name != DENOISE_EFFECT_NAME {
        return None;
    }
    let neutral = ParamValue::Integer(PROFILE_BAND_NEUTRAL_TENTH_DB);
    let mut learned = false;
    let mut bands = Vec::with_capacity(NOISE_PROFILE_BAND_COUNT);
    for name in NOISE_PROFILE_PARAMETER_NAMES {
        match effect.parameters.get(name) {
            None => bands.push(render_param(&neutral)),
            Some(value) => {
                if *value != neutral {
                    learned = true;
                }
                bands.push(render_param(value));
            }
        }
    }
    if !learned {
        return None;
    }
    Some(format!("noise_profile=[{}]", bands.join(",")))
}

fn render_param(value: &ParamValue) -> String {
    match value {
        ParamValue::Integer(value) => value.to_string(),
        ParamValue::Boolean(value) => value.to_string(),
        ParamValue::Text(value) => format!("{value:?}"),
    }
}

fn render_transition(transition: Option<&kinewright_core::Transition>) -> String {
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

    use kinewright_core::{
        AssetId, AssetSilences, AssetTranscript, CaptionCue, CaptionMotion, CaptionPreset, Clip,
        Effect, EffectId, KeyframeInterpolation, LinkId, Marker, MarkerId, MediaAsset, MediaKind,
        ParamValue, SilenceSpan, TimelineTranscriptWord, Track, TrackId, TranscriptStatus,
        Transition, animated_caption_operations, apply_batch,
    };

    use super::*;

    fn fixture() -> Document {
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            lut_assets: Vec::new(),
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
                        audio_gain_curve: None,
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
                        audio_gain_curve: None,
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
                source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
                color_description: kinewright_core::ColorDescription::default(),
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
            color_context: kinewright_core::ColorContext::default(),
        }
    }

    /// The checked-in compact rendering of [`fixture`], byte for byte.
    ///
    /// AU2 §6.3 hoisted this literal out of the golden test below so the
    /// Part B omission test can assert against the *stored* bytes rather than
    /// against another call to the renderer.
    const COMPACT_GOLDEN: &str = r#"project fps=30/1 size=1920x1080 duration=180f/6.000s
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

    #[test]
    fn timeline_state_matches_the_compact_golden_rendering() {
        assert_eq!(render_timeline_state(&fixture()), COMPACT_GOLDEN);
    }

    #[test]
    fn timeline_state_omits_internal_marker_payloads() {
        let mut document = fixture();
        document.markers.push(Marker {
            id: MarkerId(4),
            position: TimeCode(60),
            label: "__kinewright_reframe_subject_v1:large-private-sidecar".to_owned(),
            color_token: 3,
        });

        let rendered = render_timeline_state(&document);
        assert!(rendered.contains("markers=1"));
        assert!(!rendered.contains("large-private-sidecar"));
        assert!(!rendered.contains("marker 4"));
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

    /// AU1 §7 item 20: a non-neutral track carries the exact mix suffix, and
    /// `render_clip_info` grows the matching final line. The neutral fixture
    /// renders neither, which is why every pre-AU1 golden is unchanged.
    #[test]
    fn timeline_state_appends_non_neutral_track_mix() {
        let mut document = fixture();
        document.audio_mix.tracks = vec![kinewright_core::TrackMix {
            track: TrackId(7),
            gain_tenth_db: -60,
            pan_percent: 25,
            mute: false,
            solo: true,
            gain_curve: None,
            pan_curve: None,
        }];
        let rendered = render_timeline_state(&document);
        assert!(
            rendered.contains(
                "track 7 video sync_lock=true clips=2 mix=gain:-60,pan:25,mute:false,solo:true"
            ),
            "missing track mix suffix: {rendered}"
        );
    }

    #[test]
    fn timeline_state_omits_the_track_mix_suffix_when_neutral() {
        let mut document = fixture();
        assert!(!render_timeline_state(&document).contains(" mix=gain:"));
        // A stored but neutral entry is still no suffix.
        document.audio_mix.tracks = vec![kinewright_core::TrackMix::neutral(TrackId(7))];
        let rendered = render_timeline_state(&document);
        assert!(!rendered.contains(" mix=gain:"), "{rendered}");
        assert!(rendered.contains("track 7 video sync_lock=true clips=2\n"));
    }

    /// AU2 §7 item B16: the bus fader, the master chain, and the pan law each
    /// render, in §6.3's order, when they are not the default.
    #[test]
    fn au2_timeline_state_renders_the_pan_law_bus_gain_and_master() {
        let mut document = fixture();
        document.audio_mix.pan_law = PanLaw::ConstantPower;
        document.audio_mix.buses = vec![kinewright_core::AudioBus {
            id: kinewright_core::AudioBusId(1),
            name: "Dialogue".to_owned(),
            tracks: vec![TrackId(7)],
            gain_tenth_db: -35,
            effects: vec![Effect {
                id: kinewright_core::EffectId(21),
                name: "audio_gain".to_owned(),
                parameters: [("gain_tenth_db".to_owned(), ParamValue::Integer(-20))]
                    .into_iter()
                    .collect(),
                keyframes: std::collections::BTreeMap::new(),
            }],
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
        }];
        document.audio_mix.master = kinewright_core::AudioMaster {
            gain_tenth_db: 15,
            effects: vec![Effect {
                id: kinewright_core::EffectId(22),
                name: "audio_true_peak_limiter".to_owned(),
                parameters: [("ceiling_tenth_db".to_owned(), ParamValue::Integer(-10))]
                    .into_iter()
                    .collect(),
                keyframes: std::collections::BTreeMap::new(),
            }],
            gain_curve: None,
        };

        let rendered = render_timeline_state(&document);
        assert!(
            rendered.contains(
                "audio_pan_law=constant_power\naudio_buses:\n  audio_bus 1 \"Dialogue\" tracks=7 gain=-35 sidechain=none effects=[21:audio_gain(gain_tenth_db=-20)]\naudio_master gain=15 effects=[22:audio_true_peak_limiter(ceiling_tenth_db=-10)]"
            ),
            "AU2 §6.3 renderings missing or out of order: {rendered}"
        );
    }

    /// AU2 §7 item B16: every AU2 Part B field is omitted at its default, so
    /// the compact golden is byte-unchanged.
    #[test]
    fn au2_timeline_state_omits_the_neutral_master_law_and_bus_gain() {
        // The golden fixture, with the two new fields spelled out explicitly.
        let mut document = fixture();
        document.audio_mix.pan_law = PanLaw::Balance;
        document.audio_mix.master = kinewright_core::AudioMaster::default();
        let rendered = render_timeline_state(&document);
        assert_eq!(
            rendered, COMPACT_GOLDEN,
            "a neutral master and the balance law must not move a byte of the stored golden"
        );
        for absent in ["audio_pan_law=", "audio_master ", "audio_buses:"] {
            assert!(!rendered.contains(absent), "{absent} in {rendered}");
        }

        // A bus at unity carries no `gain=` field at all.
        document.audio_mix.buses = vec![kinewright_core::AudioBus {
            id: kinewright_core::AudioBusId(1),
            name: "Dialogue".to_owned(),
            tracks: vec![TrackId(7)],
            gain_tenth_db: 0,
            effects: Vec::new(),
            ducking_sidechain_tracks: vec![TrackId(7)],
            gain_curve: None,
        }];
        let rendered = render_timeline_state(&document);
        assert!(
            rendered.contains(
                "audio_buses:\n  audio_bus 1 \"Dialogue\" tracks=7 sidechain=7 effects=none"
            ),
            "a unity bus must render exactly as it did before AU2: {rendered}"
        );
        assert!(!rendered.contains("gain="), "{rendered}");
        assert!(!rendered.contains("audio_master"), "{rendered}");
        assert!(!rendered.contains("audio_pan_law"), "{rendered}");

        document.audio_mix.master = kinewright_core::AudioMaster {
            gain_tenth_db: -60,
            effects: Vec::new(),
            gain_curve: None,
        };
        assert!(
            render_timeline_state(&document).contains("audio_master gain=-60 effects=none"),
            "a gain-only master must render"
        );
        document.audio_mix.master = kinewright_core::AudioMaster {
            gain_tenth_db: 0,
            effects: vec![Effect {
                id: kinewright_core::EffectId(22),
                name: "audio_gate".to_owned(),
                parameters: std::collections::BTreeMap::new(),
                keyframes: std::collections::BTreeMap::new(),
            }],
            gain_curve: None,
        };
        assert!(
            render_timeline_state(&document)
                .contains("audio_master gain=0 effects=[22:audio_gate()]"),
            "an effects-only master must render"
        );
    }

    /// AU4 §4.1 rule 82, the present half of the golden pair: one document
    /// carrying all five owners' curves renders every one of them, in
    /// `render_effects`' own `at:value:Interp` spelling.
    ///
    /// The track entry's five scalars are all neutral, so this golden is also
    /// the agent-side witness for rule 11: `is_neutral` is false for a
    /// curve-bearing entry, which is the only reason the `mix=` suffix exists
    /// on this line at all.
    #[test]
    fn au4_timeline_state_renders_every_owner_curve() {
        let mut document = fixture();
        let clip = document
            .tracks
            .iter_mut()
            .flat_map(|track| &mut track.clips)
            .find(|clip| clip.id == ClipId(11))
            .unwrap();
        clip.audio_gain_curve = Some(curve(&[
            (0, 0, KeyframeInterpolation::Linear),
            (30, -60, KeyframeInterpolation::Hold),
            (59, -120, KeyframeInterpolation::EaseInOut),
        ]));
        document.audio_mix.tracks = vec![kinewright_core::TrackMix {
            track: TrackId(7),
            gain_tenth_db: 0,
            pan_percent: 0,
            mute: false,
            solo: false,
            gain_curve: Some(curve(&[
                (0, 0, KeyframeInterpolation::Linear),
                (90, -45, KeyframeInterpolation::Hold),
            ])),
            pan_curve: Some(curve(&[
                (0, -100, KeyframeInterpolation::EaseIn),
                (179, 100, KeyframeInterpolation::Linear),
            ])),
        }];
        document.audio_mix.buses = vec![kinewright_core::AudioBus {
            id: kinewright_core::AudioBusId(1),
            name: "Dialogue".to_owned(),
            tracks: vec![TrackId(7)],
            gain_tenth_db: -35,
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: Some(curve(&[
                (0, -35, KeyframeInterpolation::Linear),
                (120, 0, KeyframeInterpolation::EaseOut),
            ])),
        }];
        document.audio_mix.master = kinewright_core::AudioMaster {
            gain_tenth_db: 15,
            effects: Vec::new(),
            gain_curve: Some(curve(&[(0, 15, KeyframeInterpolation::Hold)])),
        };

        let rendered = render_timeline_state(&document);
        assert!(
            rendered.contains(
                " effects=none transition_in=none audio=gain:0,fade_in:0f,fade_out:0f,envelope:[0:0:Linear,30:-60:Hold,59:-120:EaseInOut]"
            ),
            "missing clip envelope: {rendered}"
        );
        assert!(
            rendered.contains(
                "track 7 video sync_lock=true clips=2 mix=gain:0,pan:0,mute:false,solo:false,gain_curve:[0:0:Linear,90:-45:Hold],pan_curve:[0:-100:EaseIn,179:100:Linear]\n"
            ),
            "missing track curves: {rendered}"
        );
        assert!(
            rendered.contains(
                "  audio_bus 1 \"Dialogue\" tracks=7 gain=-35 gain_curve=[0:-35:Linear,120:0:EaseOut] sidechain=none effects=none\naudio_master gain=15 gain_curve=[0:15:Hold] effects=none"
            ),
            "missing bus or master fader curve, or out of order: {rendered}"
        );

        let effect_spelling = render_effects(&[Effect {
            id: kinewright_core::EffectId(9),
            name: "audio_gain".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: [(
                "gain_tenth_db".to_owned(),
                curve(&[(0, 15, KeyframeInterpolation::Hold)]),
            )]
            .into_iter()
            .collect(),
        }]);
        assert!(
            effect_spelling.contains("gain_tenth_db[0:15:Hold]"),
            "{effect_spelling}"
        );
    }

    /// AU4 §4.1 rule 82, the absent half of the golden pair: every AU4 field
    /// is default-omitted, so the stored pre-AU4 golden does not move a byte.
    #[test]
    fn au4_timeline_state_omits_every_absent_curve() {
        let mut document = fixture();
        // The five new fields, spelled out explicitly at their defaults.
        for clip in document
            .tracks
            .iter_mut()
            .flat_map(|track| &mut track.clips)
        {
            clip.audio_gain_curve = None;
        }
        document.audio_mix.master = kinewright_core::AudioMaster {
            gain_tenth_db: 0,
            effects: Vec::new(),
            gain_curve: None,
        };
        let rendered = render_timeline_state(&document);
        assert_eq!(
            rendered, COMPACT_GOLDEN,
            "an absent curve on any of the five owners must not move a byte of the stored golden"
        );
        for absent in ["envelope:", "gain_curve", "pan_curve"] {
            assert!(!rendered.contains(absent), "{absent} in {rendered}");
        }

        document.audio_mix.tracks = vec![kinewright_core::TrackMix {
            track: TrackId(7),
            gain_tenth_db: -60,
            pan_percent: 25,
            mute: false,
            solo: true,
            gain_curve: None,
            pan_curve: None,
        }];
        document.audio_mix.buses = vec![kinewright_core::AudioBus {
            id: kinewright_core::AudioBusId(1),
            name: "Dialogue".to_owned(),
            tracks: vec![TrackId(7)],
            gain_tenth_db: -35,
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
        }];
        document.audio_mix.master = kinewright_core::AudioMaster {
            gain_tenth_db: 15,
            effects: Vec::new(),
            gain_curve: None,
        };
        let rendered = render_timeline_state(&document);
        assert!(
            rendered.contains(
                "track 7 video sync_lock=true clips=2 mix=gain:-60,pan:25,mute:false,solo:true\n"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "  audio_bus 1 \"Dialogue\" tracks=7 gain=-35 sidechain=none effects=none\naudio_master gain=15 effects=none"
            ),
            "{rendered}"
        );
        for absent in ["envelope:", "gain_curve", "pan_curve"] {
            assert!(!rendered.contains(absent), "{absent} in {rendered}");
        }
    }

    /// AU5 §4.2 rule 79 / A16, the carrying half of the golden pair: a
    /// learned `audio_denoise` spells its 31 bands once, low band to high, as
    /// one `noise_profile=[…]` block after the node's ordinary rows, and the
    /// individual `profile_band{nn}_tenth_db` names never appear.
    #[test]
    fn au5_timeline_state_renders_a_learned_noise_profile() {
        let mut document = fixture();
        let mut parameters = BTreeMap::from([
            ("bypass".to_owned(), ParamValue::Integer(0)),
            ("reduction_tenth_db".to_owned(), ParamValue::Integer(120)),
        ]);
        for (index, name) in NOISE_PROFILE_PARAMETER_NAMES.iter().enumerate() {
            let band = -720 - i64::try_from(index).unwrap();
            parameters.insert((*name).to_owned(), ParamValue::Integer(band));
        }
        document.audio_mix.buses = vec![kinewright_core::AudioBus {
            id: kinewright_core::AudioBusId(1),
            name: "Dialogue".to_owned(),
            tracks: vec![TrackId(7)],
            gain_tenth_db: 0,
            effects: vec![Effect {
                id: kinewright_core::EffectId(31),
                name: "audio_denoise".to_owned(),
                parameters,
                keyframes: BTreeMap::new(),
            }],
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
        }];
        let rendered = render_timeline_state(&document);
        let expected = format!(
            "effects=[31:audio_denoise(bypass=0,reduction_tenth_db=120,noise_profile=[{}])]",
            (0..NOISE_PROFILE_BAND_COUNT)
                .map(|index| (-720 - i64::try_from(index).unwrap()).to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        assert!(rendered.contains(&expected), "{rendered}");
        assert!(
            !rendered.contains("profile_band"),
            "the 31 rows must never be enumerated: {rendered}"
        );
        let enumerated = NOISE_PROFILE_PARAMETER_NAMES
            .iter()
            .enumerate()
            .map(|(index, name)| format!("{name}={}", -720 - i64::try_from(index).unwrap()))
            .collect::<Vec<_>>()
            .join(",");
        let compact = expected
            .split_once("noise_profile=")
            .map(|(_, rest)| rest.trim_end_matches(")]").len() + "noise_profile=".len())
            .unwrap();
        assert!(
            compact * 4 < enumerated.len(),
            "compact {compact} B against enumerated {} B",
            enumerated.len()
        );
    }

    /// AU5 §4.2 rule 79 / A16, the omitting half of the golden pair: an
    /// unlearned denoiser — every band absent, or every band explicitly at the
    /// neutral — renders exactly the bytes it would have rendered before AU5,
    /// and the stored pre-AU5 golden does not move.
    #[test]
    fn au5_timeline_state_omits_an_unlearned_noise_profile() {
        let mut document = fixture();
        let absent = BTreeMap::from([
            ("bypass".to_owned(), ParamValue::Integer(0)),
            ("reduction_tenth_db".to_owned(), ParamValue::Integer(200)),
        ]);
        let mut neutral = absent.clone();
        for name in NOISE_PROFILE_PARAMETER_NAMES {
            neutral.insert(
                name.to_owned(),
                ParamValue::Integer(PROFILE_BAND_NEUTRAL_TENTH_DB),
            );
        }
        for parameters in [absent, neutral] {
            let rendered = render_effects(&[Effect {
                id: kinewright_core::EffectId(31),
                name: "audio_denoise".to_owned(),
                parameters,
                keyframes: BTreeMap::new(),
            }]);
            assert_eq!(
                rendered, "[31:audio_denoise(bypass=0,reduction_tenth_db=200)]",
                "an unlearned profile must cost nothing"
            );
        }

        for (name, parameter) in [
            ("primary_correction", "profile_band01_tenth_db"),
            ("audio_denoise", "profile_band99_tenth_db"),
        ] {
            let rendered = render_effects(&[Effect {
                id: kinewright_core::EffectId(32),
                name: name.to_owned(),
                parameters: BTreeMap::from([(parameter.to_owned(), ParamValue::Integer(-720))]),
                keyframes: BTreeMap::new(),
            }]);
            assert_eq!(
                rendered,
                format!("[32:{name}({parameter}=-720)]"),
                "a row no block collects must still be published"
            );
        }

        for clip in document
            .tracks
            .iter_mut()
            .flat_map(|track| &mut track.clips)
        {
            clip.audio_gain_curve = None;
        }
        assert_eq!(render_timeline_state(&document), COMPACT_GOLDEN);
    }

    /// AU4 §4.1 rule 81: the two `set_track_automation` parameter tokens and
    /// the two `mix=` render keys are pinned to each other, both derived from
    /// Core's `TRACK_AUTOMATION_PARAMETERS`, so a rename cannot drift them
    /// apart and leave the agent unable to read back what it wrote.
    #[test]
    fn au4_track_automation_parameters_pin_the_render_keys() {
        assert_eq!(
            TRACK_AUTOMATION_PARAMETERS.len(),
            TRACK_AUTOMATION_RENDER_KEYS.len()
        );
        assert_eq!(
            track_automation_render_key(TRACK_AUTOMATION_PARAMETERS[0]),
            Some("gain_curve")
        );
        assert_eq!(
            track_automation_render_key(TRACK_AUTOMATION_PARAMETERS[1]),
            Some("pan_curve")
        );
        assert_eq!(track_automation_render_key("gain_curve"), None);
        assert_eq!(track_automation_render_key(""), None);

        assert_eq!(
            track_automation_render_key("gain_tenth_db"),
            Some("gain_curve")
        );
        assert_eq!(
            track_automation_render_key("pan_percent"),
            Some("pan_curve")
        );

        let mut document = fixture();
        document.audio_mix.tracks = vec![kinewright_core::TrackMix {
            track: TrackId(7),
            gain_tenth_db: 0,
            pan_percent: 0,
            mute: false,
            solo: false,
            gain_curve: Some(curve(&[(0, -10, KeyframeInterpolation::Linear)])),
            pan_curve: Some(curve(&[(0, -10, KeyframeInterpolation::Linear)])),
        }];
        let rendered = render_timeline_state(&document);
        let gain = rendered
            .find(&format!(",{}:", TRACK_AUTOMATION_RENDER_KEYS[0]))
            .expect("the gain render key must be printed");
        let pan = rendered
            .find(&format!(",{}:", TRACK_AUTOMATION_RENDER_KEYS[1]))
            .expect("the pan render key must be printed");
        assert!(gain < pan, "{rendered}");
    }

    fn curve(keyframes: &[(i64, i64, KeyframeInterpolation)]) -> AutomationCurve {
        AutomationCurve {
            keyframes: keyframes
                .iter()
                .map(|(at, value, interpolation)| kinewright_core::Keyframe {
                    at: TimeCode(*at),
                    value: *value,
                    interpolation: *interpolation,
                })
                .collect(),
        }
    }

    #[test]
    fn clip_info_shows_track_mix_only_when_non_neutral() {
        let mut document = fixture();
        let neutral = render_clip_info(&document, ClipId(10)).unwrap();
        assert!(!neutral.contains("track_mix="), "{neutral}");

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
                    position: kinewright_core::TitlePosition::LowerThird,
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
                audio_gain_curve: None,
            }],
        });
        document.tracks.push(Track {
            id: TrackId(9),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![Clip {
                id: ClipId(13),
                asset: AssetId(4),
                source_range: TimeCode(0)..TimeCode(60),
                content: ClipContent::Freeze(kinewright_core::FreezeFrame {
                    source_frame: TimeCode(45),
                }),
                timeline_start: TimeCode(30),
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
            }],
        });
        let non_neutral = |track| kinewright_core::TrackMix {
            track,
            gain_tenth_db: -60,
            pan_percent: 25,
            mute: false,
            solo: true,
            gain_curve: None,
            pan_curve: None,
        };
        document.audio_mix.tracks = vec![
            non_neutral(TrackId(7)),
            non_neutral(TrackId(8)),
            non_neutral(TrackId(9)),
        ];

        // media, title and freeze branches all end with the same final line.
        for clip in [ClipId(10), ClipId(12), ClipId(13)] {
            let rendered = render_clip_info(&document, clip).unwrap();
            assert!(
                rendered.ends_with("\ntrack_mix=gain:-60,pan:25,mute:false,solo:true"),
                "{clip}: {rendered}"
            );
        }

        let clip = document
            .tracks
            .iter_mut()
            .flat_map(|track| &mut track.clips)
            .find(|clip| clip.id == ClipId(10))
            .unwrap();
        clip.speed_percent = 50;
        let rendered = render_clip_info(&document, ClipId(10)).unwrap();
        assert!(
            rendered.ends_with(
                "\ntransition_in=crossfade:15f speed=50% (audio muted)\ntrack_mix=gain:-60,pan:25,mute:false,solo:true"
            ),
            "{rendered}"
        );
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
                    position: kinewright_core::TitlePosition::LowerThird,
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
                audio_gain_curve: None,
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
                content: ClipContent::Freeze(kinewright_core::FreezeFrame {
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
                audio_gain_curve: None,
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

use std::{collections::BTreeMap, ops::Range};

use kinewright_core::{InterchangeFormat, LossCode, MediaKind, Rational, TrackKind};
use serde::{Deserialize, de::IgnoredAny};

use crate::{
    MAX_MEDIA, MAX_NEST_DEPTH, MAX_RATE, MAX_TRACKS_PER_KIND, Refusal, RefusalCode, Time,
    check_rate,
    ir::{
        Clip, Decorations, Discarded, Item, Loss, Marker, MarkerKind, Media, MediaId, NamedEffect,
        ParseOptions, Placed, Playback, Source, Stack, Timeline, Track,
    },
    otio_syntax,
    parse::{self, Budget, malformed, text},
};

#[derive(Deserialize)]
struct Node {
    #[serde(rename = "OTIO_SCHEMA")]
    schema: String,
    name: Option<String>,
    kind: Option<String>,
    color: Option<String>,
    enabled: Option<bool>,
    metadata: Option<IgnoredAny>,
    #[serde(default)]
    children: Vec<Node>,
    tracks: Option<Box<Node>>,
    source_range: Option<RawRange>,
    available_range: Option<RawRange>,
    marked_range: Option<RawRange>,
    global_start_time: Option<RawTime>,
    in_offset: Option<RawTime>,
    out_offset: Option<RawTime>,
    transition_type: Option<String>,
    target_url: Option<String>,
    effect_name: Option<String>,
    time_scalar: Option<f64>,
    media_reference: Option<Box<Node>>,
    #[serde(default)]
    media_references: BTreeMap<String, Node>,
    active_media_reference_key: Option<String>,
    #[serde(default)]
    markers: Vec<Node>,
    #[serde(default)]
    effects: Vec<Node>,
}

#[derive(Clone, Copy, Deserialize)]
struct RawTime {
    value: f64,
    rate: f64,
}

#[derive(Clone, Copy, Deserialize)]
struct RawRange {
    start_time: RawTime,
    duration: RawTime,
}

impl Node {
    fn kind(&self) -> &str {
        self.schema.split('.').next().unwrap_or_default()
    }

    fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("")
    }

    fn layout_version(&self, site: &str) -> Result<(), Refusal> {
        let max = match self.kind() {
            "Clip" => 2,
            "Timeline"
            | "Stack"
            | "Track"
            | "Sequence"
            | "Gap"
            | "Filler"
            | "Transition"
            | "SerializableCollection" => 1,
            _ => return Ok(()),
        };
        if self
            .schema
            .split_once('.')
            .and_then(|(_, version)| version.parse::<u8>().ok())
            .is_some_and(|version| (1..=max).contains(&version))
        {
            Ok(())
        } else {
            Err(Refusal::new(
                RefusalCode::UnsupportedFormatVersion,
                "The OTIO layout schema version is unsupported.",
            )
            .at(site))
        }
    }
}

/// Parse one OTIO timeline from the main file bytes.
///
/// # Errors
/// Refuses malformed input, unsupported layout/rates, unresolved selection and cap breaches.
pub fn parse(
    bytes: &[u8],
    source_name: &str,
    options: ParseOptions<'_>,
) -> Result<Timeline, Refusal> {
    parse::check_bytes(bytes)?;
    otio_syntax::preflight(bytes)?;
    let node: Node = serde_json::from_slice(bytes).map_err(|error| {
        if error.to_string().starts_with("recursion limit exceeded") {
            parse::too_deep()
        } else {
            malformed(
                otio_syntax::pointer(bytes, error.line(), error.column()),
                "The OTIO JSON is malformed.",
            )
        }
    })?;
    node.layout_version("/")?;
    let (node, path) = match node.kind() {
        "Timeline" => (&node, String::new()),
        "SerializableCollection" => {
            let timelines: Vec<_> = node
                .children
                .iter()
                .enumerate()
                .filter(|(_, node)| node.kind() == "Timeline")
                .collect();
            let index =
                parse::select_timeline(&timelines, options.timeline, |(_, node)| node.label())?;
            let (index, timeline) = timelines[index];
            (timeline, format!("/children/{index}"))
        }
        _ => {
            return Err(Refusal::new(
                RefusalCode::UnsupportedContainer,
                "The OTIO file has no supported timeline container.",
            ));
        }
    };
    if node.kind() == "Timeline" && path.is_empty() {
        parse::select_timeline(&[node], options.timeline, |node| node.label())?;
    }
    let mut parser = Parser::default();
    let start = node
        .global_start_time
        .map_or(Ok(Time::ZERO), |time| parser.read_time(time, &path))?;
    let hint = node
        .global_start_time
        .map(|time| snap_rate(time.rate))
        .transpose()?;
    let root = parser.timeline(node, &path, 0)?;
    let fps = if let Some(fps) = options.project_fps.or(hint).map(check_rate).transpose()? {
        fps
    } else {
        infer_rate(&root, &parser.media)?
    };
    Ok(Timeline {
        source: Source::new(InterchangeFormat::Otio, &node.schema, source_name, bytes),
        name: text(if node.label().is_empty() {
            source_name
        } else {
            node.label()
        }),
        start,
        rate_hint: fps,
        resolution_hint: options.resolution,
        root,
        media: parser.media,
        notes: parser.notes,
    })
}

#[derive(Default)]
struct Parser<'a> {
    budget: Budget,
    media: Vec<Media>,
    media_ids: BTreeMap<(&'a str, &'a str), MediaId>,
    notes: Vec<Loss>,
}

impl<'a> Parser<'a> {
    fn timeline(&mut self, node: &'a Node, path: &str, depth: usize) -> Result<Stack, Refusal> {
        node.layout_version(path)?;
        let tracks = node
            .tracks
            .as_deref()
            .ok_or_else(|| malformed(format!("{path}/tracks"), "The timeline has no tracks."))?;
        let mut root = self.stack(tracks, &format!("{path}/tracks"), depth)?;
        self.metadata(node);
        if let Some(window) = node.source_range {
            parse::apply_window(&mut root, self.range(window, path)?)?;
        }
        Ok(root)
    }

    fn stack(&mut self, node: &'a Node, path: &str, depth: usize) -> Result<Stack, Refusal> {
        node.layout_version(path)?;
        if depth > MAX_NEST_DEPTH {
            return Err(parse::too_deep());
        }
        if node.kind() != "Stack" {
            return Err(malformed(
                path,
                "A timeline stack must use the Stack schema.",
            ));
        }
        self.metadata(node);
        let mut root = Stack {
            decorations: self.decorations(node, path)?,
            ..Stack::default()
        };
        let mut counts = [0; 2];
        for (index, child) in node.children.iter().enumerate() {
            if matches!(child.kind(), "Track" | "Sequence") {
                let kind = track_kind(child, path)?;
                let kind_index = usize::from(kind == TrackKind::Audio);
                counts[kind_index] += 1;
                if counts[kind_index] > MAX_TRACKS_PER_KIND {
                    return Err(Refusal::new(
                        RefusalCode::TooManyTracks,
                        "The timeline exceeds 128 tracks of one kind.",
                    )
                    .at(path));
                }
                root.tracks.push(self.track(
                    child,
                    kind,
                    &format!("{path}/children/{index}"),
                    depth,
                )?);
            } else {
                counts[0] += 1;
                if counts[0] > MAX_TRACKS_PER_KIND {
                    return Err(Refusal::new(
                        RefusalCode::TooManyTracks,
                        "The timeline exceeds 128 video tracks.",
                    )
                    .at(path));
                }
                let mut track = Track::new(TrackKind::Video);
                track.items.push(self.placed(
                    child,
                    Time::ZERO,
                    TrackKind::Video,
                    &format!("{path}/children/{index}"),
                    depth,
                )?);
                root.tracks.push(track);
            }
        }
        if let Some(window) = node.source_range {
            parse::apply_window(&mut root, self.range(window, path)?)?;
        }
        Ok(root)
    }

    fn track(
        &mut self,
        node: &'a Node,
        kind: TrackKind,
        path: &str,
        depth: usize,
    ) -> Result<Track, Refusal> {
        node.layout_version(path)?;
        self.metadata(node);
        let mut track = Track::new(kind);
        track.name = node
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(text);
        track.decorations = self.decorations(node, path)?;
        let mut at = Time::ZERO;
        for (index, child) in node.children.iter().enumerate() {
            let site = format!("{path}/children/{index}");
            let placed = self.placed(child, at, kind, &site, depth)?;
            at = at.checked_add(placed.item.span())?;
            track.items.push(placed);
        }
        if let Some(window) = node.source_range {
            parse::window_track(&mut track, &self.range(window, path)?)?;
        }
        Ok(track)
    }

    fn placed(
        &mut self,
        node: &'a Node,
        at: Time,
        kind: TrackKind,
        site: &str,
        depth: usize,
    ) -> Result<Placed, Refusal> {
        self.budget.item()?;
        node.layout_version(site)?;
        self.metadata(node);
        let mut notes = Vec::new();
        let range = node
            .source_range
            .map(|range| self.range(range, site))
            .transpose()?;
        let span = range
            .as_ref()
            .map_or(Ok(Time::ZERO), |range| range.end.checked_sub(range.start))?;
        let item = match node.kind() {
            "Clip" => self.clip(node, kind, range, site, &mut notes)?,
            "Gap" | "Filler" => Item::Gap { duration: span },
            "Transition" => Item::Transition {
                kind: text(
                    node.transition_type
                        .as_deref()
                        .or(node.name.as_deref())
                        .unwrap_or("SMPTE_Dissolve"),
                ),
                before: self.duration(node.in_offset, site)?,
                after: self.duration(node.out_offset, site)?,
            },
            "Stack" | "Timeline" => {
                let root = if node.kind() == "Stack" {
                    self.stack(node, site, depth + 1)?
                } else {
                    self.timeline(node, site, depth + 1)?
                };
                let duration = root.window.as_ref().map_or_else(
                    || stack_duration(&root),
                    |window| window.end.checked_sub(window.start),
                )?;
                Item::Nested {
                    stack: root,
                    duration,
                }
            }
            "Track" | "Sequence" => {
                if depth >= MAX_NEST_DEPTH {
                    return Err(parse::too_deep());
                }
                let track = self.track(node, track_kind(node, site)?, site, depth + 1)?;
                let stack = Stack {
                    tracks: vec![track],
                    ..Stack::default()
                };
                let duration = if range.is_some() {
                    span
                } else {
                    stack_duration(&stack)?
                };
                Item::Nested { stack, duration }
            }
            _ => Item::Unsupported {
                what: text(&node.schema),
                duration: span,
            },
        };
        let mut placed = Placed::new(at, item, site);
        placed.notes = notes;
        Ok(placed)
    }

    fn clip(
        &mut self,
        node: &'a Node,
        kind: TrackKind,
        range: Option<Range<Time>>,
        site: &str,
        notes: &mut Vec<Loss>,
    ) -> Result<Item, Refusal> {
        if let Some(name) = node.name.as_deref().filter(|name| !name.is_empty()) {
            notes.push(Loss::dropped(LossCode::ClipNamesDropped).named(name));
        }
        let reference = if node.schema == "Clip.2" {
            node.active_media_reference_key
                .as_deref()
                .and_then(|key| node.media_references.get(key))
        } else {
            node.media_reference.as_deref()
        };
        let available = reference
            .and_then(|r| r.available_range)
            .map(|r| self.range(r, site))
            .transpose()?;
        let source = range
            .or_else(|| available.as_ref().map(|range| range.start..range.end))
            .ok_or_else(|| malformed(site, "A clip has no source or available range."))?;
        let span = source.end.checked_sub(source.start)?;
        if let Some(reference) = reference {
            let loss = match reference.schema.as_str() {
                "ExternalReference.1" | "MissingReference.1" => None,
                "GeneratorReference.1" => Some(LossCode::GeneratorDropped),
                "ImageSequenceReference.1" => Some(LossCode::ImageSequenceDropped),
                _ => Some(LossCode::UnknownItem),
            };
            if let Some(code) = loss {
                notes.push(
                    Loss::dropped(code)
                        .named(reference.name.as_deref().unwrap_or(&reference.schema)),
                );
                return Ok(Item::Gap { duration: span });
            }
            self.metadata(reference);
        }
        let locator = reference
            .and_then(|r| r.target_url.as_deref())
            .unwrap_or_default();
        let fps = reference
            .and_then(|r| r.available_range)
            .map(|r| snap_rate(r.start_time.rate))
            .transpose()?
            .or(node
                .source_range
                .map(|range| snap_rate(range.start_time.rate))
                .transpose()?);
        let media_kind = match (kind, is_still(locator)) {
            (TrackKind::Audio, _) => MediaKind::Audio,
            (TrackKind::Video, true) => MediaKind::Image,
            (TrackKind::Video, false) => MediaKind::Video,
        };
        let media = self.media(MediaInput {
            locator,
            name: node.label(),
            media_kind,
            fps,
            available,
        })?;
        let mut clip = Clip::new(media, source.start, span);
        clip.enabled = node.enabled.unwrap_or(true);
        clip.name = node.name.as_deref().map(text);
        clip.decorations = self.decorations(node, site)?;
        for marker in &mut clip.decorations.markers {
            marker.at = marker.at.checked_sub(source.start)?;
        }
        if let Some(effect) = node
            .effects
            .iter()
            .find(|effect| matches!(effect.schema.as_str(), "LinearTimeWarp.1" | "FreezeFrame.1"))
        {
            clip.time = if effect.schema == "FreezeFrame.1" {
                Playback::Freeze(source.start)
            } else {
                notes.push(Loss::approximation(LossCode::TimeWarpReading));
                let ratio = Time::from_float(effect.time_scalar.unwrap_or(1.0))?;
                match ratio.cmp(&Time::ZERO) {
                    std::cmp::Ordering::Equal => Playback::Freeze(source.start),
                    std::cmp::Ordering::Less => Playback::Reverse(ratio.abs()?),
                    std::cmp::Ordering::Greater => Playback::Speed(ratio),
                }
            };
        }
        Ok(Item::Clip(clip))
    }

    fn media(&mut self, input: MediaInput<'a>) -> Result<MediaId, Refusal> {
        let key = (
            input.locator,
            if input.locator.is_empty() {
                input.name
            } else {
                ""
            },
        );
        if let Some(id) = self.media_ids.get(&key) {
            let media = &mut self.media[id.0];
            if media.kind != input.media_kind
                && media.kind != MediaKind::Image
                && input.media_kind != MediaKind::Image
            {
                media.kind = MediaKind::AudioVideo;
            }
            return Ok(*id);
        }
        if self.media.len() >= MAX_MEDIA {
            return Err(parse::too_many_items());
        }
        let id = MediaId(self.media.len());
        self.media_ids.insert(key, id);
        self.media.push(Media {
            locator: input.locator.into(),
            name: text(if input.name.is_empty() {
                input.locator.rsplit(['/', '\\']).next().unwrap_or_default()
            } else {
                input.name
            }),
            kind: input.media_kind,
            fps: input.fps,
            start: input.available.as_ref().map(|range| range.start),
            duration: input
                .available
                .map(|range| range.end.checked_sub(range.start))
                .transpose()?,
            resolution: None,
        });
        Ok(id)
    }

    fn decorations(&mut self, node: &'a Node, path: &str) -> Result<Decorations, Refusal> {
        let mut result = Decorations::default();
        for (index, marker) in node.markers.iter().enumerate() {
            self.budget.item()?;
            self.metadata(marker);
            let site = format!("{path}/markers/{index}");
            if !matches!(marker.schema.as_str(), "Marker.1" | "Marker.2" | "Marker.3") {
                result.removed.push(Discarded {
                    count: 1,
                    loss: Loss::dropped(LossCode::UnknownItem).named(&marker.schema),
                    site,
                });
                continue;
            }
            let range = marker
                .marked_range
                .map(|range| self.range(range, &site))
                .transpose()?
                .unwrap_or(Time::ZERO..Time::ZERO);
            result.markers.push(Marker {
                at: range.start,
                duration: range.end.checked_sub(range.start)?,
                label: text(marker.label()),
                color: marker.color.as_deref().map(text),
                site,
                kind: MarkerKind::Plain,
            });
        }
        let mut time_effect = false;
        for (index, effect) in node.effects.iter().enumerate() {
            self.budget.item()?;
            self.metadata(effect);
            if node.kind() == "Clip"
                && !time_effect
                && matches!(effect.schema.as_str(), "LinearTimeWarp.1" | "FreezeFrame.1")
            {
                time_effect = true;
                continue;
            }
            let site = format!("{path}/effects/{index}");
            result.effects.push(NamedEffect {
                name: text(
                    effect
                        .effect_name
                        .as_deref()
                        .or(effect.name.as_deref())
                        .unwrap_or(&effect.schema),
                ),
                site,
                code: LossCode::EffectDropped,
            });
        }
        Ok(result)
    }

    fn metadata(&mut self, node: &'a Node) {
        if node.metadata.is_some() {
            self.notes.push(Loss::dropped(LossCode::MetadataDropped));
        }
        if node.color.is_some() && node.kind() != "Marker" {
            self.notes.push(Loss::dropped(LossCode::MetadataDropped));
        }
    }

    fn read_time(&mut self, raw: RawTime, site: &str) -> Result<Time, Refusal> {
        let rate = snap_rate(raw.rate).map_err(|error| error.at(site))?;
        let snapped = f64::from(rate.numerator()) / f64::from(rate.denominator());
        if (raw.rate - snapped).abs() > f64::EPSILON {
            self.notes.push(Loss::approximation(LossCode::RateSnapped));
        }
        let round = raw.value.round();
        let value = if (raw.value - round).abs() <= 1e-6 {
            round
        } else {
            self.notes
                .push(Loss::approximation(LossCode::SubframeRounded));
            raw.value
        };
        Time::from_float(value)?.checked_div(Time::rate(rate))
    }

    fn range(&mut self, raw: RawRange, site: &str) -> Result<Range<Time>, Refusal> {
        let start = self.read_time(raw.start_time, site)?;
        let end = start.checked_add(self.duration(Some(raw.duration), site)?)?;
        Ok(start..end)
    }

    fn duration(&mut self, raw: Option<RawTime>, site: &str) -> Result<Time, Refusal> {
        let time = raw.map_or(Ok(Time::ZERO), |time| self.read_time(time, site))?;
        if time < Time::ZERO {
            return Err(malformed(site, "A duration is negative."));
        }
        Ok(time)
    }
}

struct MediaInput<'a> {
    locator: &'a str,
    name: &'a str,
    media_kind: MediaKind,
    fps: Option<Rational>,
    available: Option<Range<Time>>,
}

fn stack_duration(stack: &Stack) -> Result<Time, Refusal> {
    stack
        .tracks
        .iter()
        .flat_map(|track| &track.items)
        .try_fold(Time::ZERO, |end, placed| {
            Ok(end.max(placed.at.checked_add(placed.item.span())?))
        })
}

fn track_kind(node: &Node, site: &str) -> Result<TrackKind, Refusal> {
    match node.kind.as_deref().unwrap_or("Video") {
        "Video" => Ok(TrackKind::Video),
        "Audio" => Ok(TrackKind::Audio),
        _ => Err(malformed(site, "The OTIO track kind is unknown.")),
    }
}

pub(crate) fn is_still(locator: &str) -> bool {
    locator.rsplit_once('.').is_some_and(|(_, ext)| {
        ["jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp", "exr"]
            .iter()
            .any(|candidate| ext.eq_ignore_ascii_case(candidate))
    })
}

fn snap_rate(rate: f64) -> Result<Rational, Refusal> {
    let invalid = || {
        Refusal::new(
            RefusalCode::UnsupportedRate,
            "The OTIO frame rate cannot be snapped to a supported rate.",
        )
    };
    if !rate.is_finite() || rate <= 0.0 || rate > f64::from(MAX_RATE) {
        return Err(invalid());
    }
    let integral = rate.round();
    if (integral - rate).abs() <= 1e-6 {
        return Time::from_float(integral)?.as_rate();
    }
    let k = (rate * 1.001).round();
    if (rate - k * 1000.0 / 1001.0).abs() <= 0.005 {
        return Time::from_float(k)?
            .checked_mul(Time::new(1000, 1001)?)?
            .as_rate();
    }
    Err(invalid())
}

fn infer_rate(root: &Stack, media: &[Media]) -> Result<Rational, Refusal> {
    let mut weighted = BTreeMap::new();
    let mut stacks = vec![root];
    while let Some(stack) = stacks.pop() {
        for track in &stack.tracks {
            for placed in &track.items {
                match &placed.item {
                    Item::Nested { stack, .. } => stacks.push(stack),
                    Item::Clip(clip) if track.kind == TrackKind::Video => {
                        if let Some(rate) = media[clip.media.0].fps {
                            let weight = weighted.entry(Time::rate(rate)).or_insert(Time::ZERO);
                            *weight = weight.checked_add(clip.span)?;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    weighted
        .into_iter()
        .max_by_key(|(rate, weight)| (*weight, *rate))
        .map(|(rate, _)| rate.as_rate())
        .transpose()?
        .ok_or_else(|| {
            Refusal::new(
                RefusalCode::ProjectRateRequired,
                "Supply a project frame rate for this OTIO timeline.",
            )
        })
}

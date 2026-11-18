use std::collections::{BTreeMap, BTreeSet};

use kinewright_core::{InterchangeFormat, LossBound, LossCode, MediaKind, Rational, TrackKind};

use crate::{
    MAX_MEDIA, MAX_NEST_DEPTH, MAX_TEXT_BYTES, MAX_TRACKS_PER_KIND, Refusal, RefusalCode, Time,
    check_rate, fcpxml_syntax,
    fcpxml_time::{duration, optional, site, source_map},
    ir::{
        Anchor, Clip, ContainerTime, Decorations, GainKey, Item, Loss, Marker, MarkerKind, Media,
        MediaId, NamedEffect, ParseOptions, Placed, Playback, Source, Stack, Timeline, Track,
    },
    otio::is_still,
    parse::{self, Budget, malformed, text},
};

type Xml<'a, 'input> = roxmltree::Node<'a, 'input>;
type ResourceMap<'a, 'input> = BTreeMap<&'a str, Xml<'a, 'input>>;

/// Parse one selected timeline from an FCPXML document.
///
/// # Errors
/// Refuses malformed XML, unsafe declarations, invalid rates and unsupported or oversized trees.
pub fn parse(
    bytes: &[u8],
    source_name: &str,
    options: ParseOptions<'_>,
) -> Result<Timeline, Refusal> {
    let document = fcpxml_syntax::document(bytes)?;
    let root = document.root_element();
    if !root.has_tag_name("fcpxml") {
        return Err(Refusal::new(
            RefusalCode::UnsupportedContainer,
            "The XML root is not FCPXML.",
        ));
    }
    let version = root.attribute("version").unwrap_or_default();
    let minor = version
        .strip_prefix("1.")
        .and_then(|minor| minor.parse::<u8>().ok());
    if !minor.is_some_and(|minor| (8..=14).contains(&minor)) {
        return Err(Refusal::new(
            RefusalCode::UnsupportedFormatVersion,
            "FCPXML versions 1.8 through 1.14 are supported.",
        )
        .at(site(root)));
    }
    if let Some(fps) = options.project_fps {
        check_rate(fps)?;
    }

    let resources = index_resources(root)?;
    check_reference_cycles(&resources)?;
    let projects: Vec<_> = root
        .descendants()
        .filter(|node| node.has_tag_name("project") && child(*node, "sequence").is_some())
        .collect();
    let selected = parse::select_timeline(&projects, options.timeline, |project| {
        project.attribute("name").unwrap_or_default()
    })?;
    let project = projects[selected];
    let sequence = child(project, "sequence")
        .ok_or_else(|| malformed(site(project), "The project has no sequence."))?;
    let format = sequence
        .attribute("format")
        .and_then(|id| resources.get(id))
        .copied()
        .ok_or_else(|| malformed(site(sequence), "The sequence has no format resource."))?;
    let rate_hint = format_rate(format)?
        .ok_or_else(|| malformed(site(format), "The sequence format has no frame duration."))?;
    let start = optional(sequence, "tcStart")?.unwrap_or(Time::ZERO);
    let mut parser = Parser {
        resources,
        media_ids: BTreeMap::new(),
        media: Vec::new(),
        notes: Vec::new(),
        budget: Budget::default(),
        project_rate: rate_hint,
        minor: minor.unwrap_or_default(),
        next_group: 0,
        lines: LineIndex::new(bytes),
    };
    if sequence.attribute("tcFormat") == Some("DF") {
        parser
            .notes
            .push(Loss::approximation(LossCode::DropFrameLabels));
    }
    let spine = child(sequence, "spine")
        .ok_or_else(|| malformed(site(sequence), "The sequence has no spine."))?;
    let mut root = parser.spine(spine, start, 0)?;
    root.time.in_point = start;
    root.decorations = parser.decorations(sequence, Time::ZERO)?;
    let resolution_hint = resolution(format)?.or(options.resolution);
    Ok(Timeline {
        source: Source::new(InterchangeFormat::Fcpxml, version, source_name, bytes),
        name: text(project.attribute("name").unwrap_or(source_name)),
        start,
        rate_hint,
        resolution_hint,
        root,
        media: parser.media,
        notes: parser.notes,
    })
}

const LINE_INDEX_CHUNK_BYTES: usize = 4 * 1024;

struct LineIndex<'input> {
    bytes: &'input [u8],
    // Each checkpoint holds the line number and its starting byte offset.
    prefixes: Vec<(usize, usize)>,
}

impl<'input> LineIndex<'input> {
    fn new(bytes: &'input [u8]) -> Self {
        let (chunks, _) = bytes.as_chunks::<LINE_INDEX_CHUNK_BYTES>();
        let mut prefixes = Vec::with_capacity(chunks.len() + 1);
        let mut prefix = (1, 0);
        prefixes.push(prefix);
        for (index, chunk) in chunks.iter().enumerate() {
            prefix = advance_lines(chunk, index * LINE_INDEX_CHUNK_BYTES, prefix);
            prefixes.push(prefix);
        }
        Self { bytes, prefixes }
    }

    fn position(&self, offset: usize) -> (usize, usize) {
        let chunk = offset / LINE_INDEX_CHUNK_BYTES;
        let base = chunk * LINE_INDEX_CHUNK_BYTES;
        let (line, start) = advance_lines(&self.bytes[base..offset], base, self.prefixes[chunk]);
        (line, offset - start + 1)
    }
}

fn advance_lines(
    bytes: &[u8],
    base: usize,
    (mut line, mut start): (usize, usize),
) -> (usize, usize) {
    for (offset, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            line += 1;
            start = base + offset + 1;
        }
    }
    (line, start)
}

fn child<'a, 'input>(node: Xml<'a, 'input>, name: &str) -> Option<Xml<'a, 'input>> {
    node.children().find(|child| child.has_tag_name(name))
}

fn element_name<'input>(node: Xml<'_, 'input>) -> &'input str {
    node.tag_name().name()
}

fn index_resources<'a, 'input>(root: Xml<'a, 'input>) -> Result<ResourceMap<'a, 'input>, Refusal> {
    let mut ids = BTreeMap::new();
    let mut assets = 0;
    for resource in root
        .children()
        .filter(|node| node.has_tag_name("resources"))
        .flat_map(|node| node.children())
    {
        if let Some(id) = resource.attribute("id")
            && ids.insert(id, resource).is_some()
        {
            return Err(malformed(
                site(resource),
                "A resource ID is declared more than once.",
            ));
        }
        if resource.has_tag_name("asset") {
            assets += 1;
            if assets > MAX_MEDIA {
                return Err(parse::too_many_items());
            }
        }
        if resource.has_tag_name("format") {
            format_rate(resource)?;
        }
    }
    Ok(ids)
}

fn check_reference_cycles(resources: &ResourceMap<'_, '_>) -> Result<(), Refusal> {
    let mut completed = BTreeSet::new();
    let mut active = BTreeSet::new();
    for (&id, &resource) in resources {
        if completed.contains(id) {
            continue;
        }
        let edges = |resource: Xml<'_, '_>| {
            resource
                .descendants()
                .filter_map(|node| node.attribute("ref"))
                .filter_map(|id| {
                    resources
                        .get_key_value(id)
                        .map(|(&id, &resource)| (id, resource))
                })
                .collect::<Vec<_>>()
                .into_iter()
        };
        active.insert(id);
        let mut pending = vec![(id, edges(resource))];
        while let Some((id, children)) = pending.last_mut() {
            if let Some((child_id, child)) = children.next() {
                if active.contains(child_id) {
                    return Err(malformed(
                        site(child),
                        "The resource reference graph has a cycle.",
                    ));
                }
                if !completed.contains(child_id) {
                    active.insert(child_id);
                    pending.push((child_id, edges(child)));
                }
            } else {
                active.remove(*id);
                completed.insert(*id);
                pending.pop();
            }
        }
    }
    Ok(())
}

fn format_rate(format: Xml<'_, '_>) -> Result<Option<Rational>, Refusal> {
    optional(format, "frameDuration")?
        .map(|duration| {
            if duration <= Time::ZERO {
                return Err(Refusal::new(
                    RefusalCode::UnsupportedRate,
                    "The format frame duration must be positive.",
                )
                .at(site(format)));
            }
            Time::ONE
                .checked_div(duration)?
                .as_rate()
                .map_err(|error| error.at(site(format)))
        })
        .transpose()
}

fn resolution(format: Xml<'_, '_>) -> Result<Option<(u32, u32)>, Refusal> {
    let (Some(width), Some(height)) = (format.attribute("width"), format.attribute("height"))
    else {
        return Ok(None);
    };
    let parse_dimension = |value: &str| {
        value
            .parse::<u32>()
            .ok()
            .filter(|&value| value > 0)
            .ok_or_else(|| malformed(site(format), "The format resolution is invalid."))
    };
    Ok(Some((parse_dimension(width)?, parse_dimension(height)?)))
}

struct Parser<'a, 'input> {
    resources: ResourceMap<'a, 'input>,
    media_ids: BTreeMap<&'a str, MediaId>,
    media: Vec<Media>,
    notes: Vec<Loss>,
    budget: Budget,
    project_rate: Rational,
    minor: u8,
    next_group: u32,
    lines: LineIndex<'input>,
}

type Parts = Vec<(TrackKind, Placed)>;

impl<'a, 'input> Parser<'a, 'input> {
    fn spine(
        &mut self,
        node: Xml<'a, 'input>,
        origin: Time,
        depth: usize,
    ) -> Result<Stack, Refusal> {
        if depth > MAX_NEST_DEPTH {
            return Err(parse::too_deep());
        }
        let mut cursor = origin;
        let mut tracks = BTreeMap::<(bool, bool, i64), Track>::new();
        let mut zero_duration = Vec::new();
        for element in node
            .children()
            .filter(|child| child.is_element() && is_story(*child))
        {
            let lane = lane(element)?;
            let at = optional(element, "offset")?.unwrap_or(cursor);
            let parts = self.parts(element, at, cursor, depth)?;
            for (kind, placed) in parts {
                if placed.item.zero_duration_count().is_some() {
                    zero_duration.push(placed);
                    continue;
                }
                if lane == 0 {
                    cursor = cursor.max(placed.at.checked_add(placed.item.span())?);
                }
                let audio = kind == TrackKind::Audio;
                let lane_key = (audio, audio && lane != 0, lane);
                if !tracks.contains_key(&lane_key)
                    && tracks
                        .keys()
                        .filter(|(is_audio, _, _)| *is_audio == audio)
                        .count()
                        >= MAX_TRACKS_PER_KIND
                {
                    return Err(Refusal::new(
                        RefusalCode::TooManyTracks,
                        "The timeline exceeds 128 lanes of one kind.",
                    )
                    .at(site(element)));
                }
                tracks
                    .entry(lane_key)
                    .or_insert_with(|| Track::new(kind))
                    .items
                    .push(placed);
            }
        }
        mirror_audio_transitions(&mut tracks);
        let mut stack = Stack {
            tracks: tracks.into_values().collect(),
            ..Stack::default()
        };
        for track in &mut stack.tracks {
            self.track_roles(track);
        }
        if !zero_duration.is_empty() {
            if stack.tracks.is_empty() {
                stack.tracks.push(Track::new(TrackKind::Video));
            }
            stack.tracks[0].items.extend(zero_duration);
        }
        Ok(stack)
    }

    fn track_roles(&mut self, track: &mut Track) {
        let mut roles = track.items.iter().filter_map(|placed| match &placed.item {
            Item::Clip(clip) => Some(clip.role.as_deref()),
            _ => None,
        });
        if let Some(first) = roles.next() {
            if roles.all(|role| role == first) {
                track.name = first.map(text);
            } else {
                self.notes.push(Loss::dropped(LossCode::TrackNamesDropped));
            }
        }
    }

    fn parts(
        &mut self,
        node: Xml<'a, 'input>,
        offset: Time,
        cut: Time,
        depth: usize,
    ) -> Result<Parts, Refusal> {
        self.budget.item()?;
        let span = duration(node, Time::ZERO)?;
        if !node.has_tag_name("transition")
            && node.attribute("duration").is_some()
            && span <= Time::new(1, 1_000_000)?.checked_div(Time::rate(self.project_rate))?
        {
            let placed = Placed::discard_zero_duration(offset, &self.place(node));
            return Ok(vec![(TrackKind::Video, placed)]);
        }
        self.timed_parts(node, offset, cut, depth, span)
    }

    fn timed_parts(
        &mut self,
        node: Xml<'a, 'input>,
        offset: Time,
        cut: Time,
        depth: usize,
        span: Time,
    ) -> Result<Parts, Refusal> {
        let name = element_name(node);
        let source = self.place(node);
        let start = optional(node, "start")?.unwrap_or(Time::ZERO);
        let mut notes = Vec::new();
        let mut frame_origin = start;
        let mut speed = Time::ONE;
        let generator = self.video_generator(node);
        let mut parts = match name {
            "video" if generator.is_some() => {
                notes.push(
                    Loss::dropped(LossCode::GeneratorDropped).named(generator.unwrap_or("video")),
                );
                vec![(
                    TrackKind::Video,
                    Placed::new(offset, Item::Gap { duration: span }, source),
                )]
            }
            "asset-clip" | "clip" | "video" | "audio" if node.attribute("ref").is_some() => {
                let media_ref = node.attribute("ref").unwrap_or_default();
                let id = self.asset(media_ref, node)?;
                let source_media = &self.media[id.0];
                let start = optional(node, "start")?
                    .or(source_media.start)
                    .unwrap_or(Time::ZERO);
                let span = duration(node, source_media.duration.unwrap_or(Time::ZERO))?;
                let (in_point, playback) =
                    source_map(node, start, span, source_media.fps, self.project_rate)?;
                frame_origin = in_point;
                speed = playback.average()?;
                if matches!(playback, Playback::Freeze(_)) {
                    notes.push(Loss::approximation(LossCode::HoldReading));
                }
                self.media_parts(node, id, in_point, span, playback, offset)?
            }
            "ref-clip" | "sync-clip" | "mc-clip" | "audition" | "spine" | "clip" => {
                let (in_point, playback) = source_map(node, start, span, None, self.project_rate)?;
                frame_origin = in_point;
                speed = playback.average()?;
                let item = self.nested(node, span, in_point, &playback, depth, &mut notes)?;
                vec![(TrackKind::Video, Placed::new(offset, item, source))]
            }
            "transition" => vec![(
                transition_track_kind(node),
                self.transition(node, offset, span, cut, &source)?,
            )],
            name => {
                let item = match name {
                    "gap" => Item::Gap { duration: span },
                    "title" => Item::Title {
                        text: title_text(node),
                        duration: span,
                        enabled: true,
                        decorations: Decorations::empty(),
                    },
                    _ => Item::Unsupported {
                        what: text(name),
                        duration: span,
                    },
                };
                vec![(TrackKind::Video, Placed::new(offset, item, source))]
            }
        };
        if let Some((_, first)) = parts.first_mut() {
            first.notes.append(&mut notes);
            first.decorations = self.decorations(node, frame_origin)?;
            if speed > Time::ZERO && speed != Time::ONE {
                for marker in &mut first.decorations.markers {
                    marker.at = marker.at.checked_div(speed)?;
                    marker.duration = marker.duration.checked_div(speed)?;
                }
            }
            if name == "video" && node.attribute("ref").is_none() {
                first.notes.push(
                    Loss::dropped(LossCode::GeneratorDropped)
                        .named(node.attribute("name").unwrap_or("video")),
                );
            }
            let parent = ContainerTime {
                offset,
                in_point: frame_origin,
                speed: if speed > Time::ZERO { speed } else { Time::ONE },
            };
            let owns_inline_children = matches!(name, "sync-clip" | "spine" | "clip")
                && matches!(first.item, Item::Nested { .. })
                && child(node, "spine").is_none();
            if !owns_inline_children {
                first.anchors = self.anchors(node, parent, depth, speed != Time::ONE)?;
            }
        }
        if node
            .ancestors()
            .any(|ancestor| ancestor.attribute("enabled") == Some("0"))
        {
            for (_, placed) in &mut parts {
                placed.disable();
            }
        }
        Ok(parts)
    }

    fn video_generator(&self, node: Xml<'a, 'input>) -> Option<&'a str> {
        if !node.has_tag_name("video") {
            return None;
        }
        let id = node.attribute("ref")?;
        let resource = self.resources.get(id)?;
        resource.has_tag_name("effect").then(|| {
            resource
                .attribute("name")
                .or_else(|| node.attribute("name"))
                .unwrap_or(id)
        })
    }

    fn transition(
        &self,
        node: Xml<'_, '_>,
        offset: Time,
        span: Time,
        previous_end: Time,
        source: &str,
    ) -> Result<Placed, Refusal> {
        let end = offset.checked_add(span)?;
        let center = offset.checked_add(span.checked_div(Time::integer(2))?)?;
        let cut = if previous_end >= offset && previous_end <= end {
            previous_end
        } else {
            center
        };
        let kind = node
            .attribute("name")
            .or_else(|| {
                child(node, "filter-video")
                    .or_else(|| child(node, "filter-audio"))
                    .and_then(|filter| filter.attribute("ref"))
                    .and_then(|id| self.resources.get(id))
                    .and_then(|effect| effect.attribute("name"))
            })
            .unwrap_or("Cross Dissolve");
        Ok(Placed::new(
            cut,
            Item::Transition {
                kind: text(kind),
                before: cut.checked_sub(offset)?,
                after: end.checked_sub(cut)?,
            },
            source,
        ))
    }

    fn asset(&mut self, id: &'a str, node: Xml<'a, 'input>) -> Result<MediaId, Refusal> {
        if let Some(existing) = self.media_ids.get(id) {
            return Ok(*existing);
        }
        if self.media.len() >= MAX_MEDIA {
            return Err(parse::too_many_items());
        }
        let resource = self
            .resources
            .get(id)
            .copied()
            .filter(|asset| asset.has_tag_name("asset"));
        let resource =
            resource.ok_or_else(|| malformed(site(node), "A clip references an unknown asset."))?;
        let locator = if self.minor == 8 {
            resource.attribute("src")
        } else {
            resource
                .children()
                .find(|rep| {
                    rep.has_tag_name("media-rep") && rep.attribute("kind") == Some("original-media")
                })
                .and_then(|rep| rep.attribute("src"))
        }
        .unwrap_or_default();
        let format = resource
            .attribute("format")
            .and_then(|format| self.resources.get(format))
            .copied();
        let fps = format.map(format_rate).transpose()?.flatten();
        let resolution = format.map(resolution).transpose()?.flatten();
        let has_video = resource.attribute("hasVideo").map_or(
            resolution.is_some() || !node.has_tag_name("audio"),
            |kind| kind == "1",
        );
        let has_audio = resource
            .attribute("hasAudio")
            .is_some_and(|flag| flag == "1")
            || node.has_tag_name("audio");
        let kind = match (has_video, has_audio, is_still(locator)) {
            (false, _, _) => MediaKind::Audio,
            (_, _, true) => MediaKind::Image,
            (true, true, _) => MediaKind::AudioVideo,
            (true, false, _) => MediaKind::Video,
        };
        let asset_id = MediaId(self.media.len());
        self.media_ids.insert(id, asset_id);
        self.media.push(Media {
            locator: locator.into(),
            name: text(resource.attribute("name").unwrap_or(id)),
            kind,
            fps,
            resolution,
            start: optional(resource, "start")?,
            duration: optional(resource, "duration")?,
        });
        Ok(asset_id)
    }

    fn media_parts(
        &mut self,
        node: Xml<'a, 'input>,
        id: MediaId,
        in_point: Time,
        span: Time,
        playback: Playback,
        offset: Time,
    ) -> Result<Parts, Refusal> {
        let kind = self.media[id.0].kind;
        let enable = node.attribute("srcEnable");
        let video = kind.supports(TrackKind::Video)
            && !node.has_tag_name("audio")
            && enable != Some("audio");
        let has_audio = kind.supports(TrackKind::Audio)
            && !node.has_tag_name("video")
            && enable != Some("video");
        let group = if video && has_audio {
            self.next_group += 1;
            Some(self.next_group)
        } else {
            None
        };
        let mut parts = Vec::new();
        let mut playback = Some(playback);
        for (kind, enabled) in [(TrackKind::Video, video), (TrackKind::Audio, has_audio)] {
            if !enabled {
                continue;
            }
            let audio = kind == TrackKind::Audio;
            let at = if audio {
                let reference = optional(node, "start")?.unwrap_or(in_point);
                let source = optional(node, "audioStart")?.unwrap_or(reference);
                offset.checked_add(source.checked_sub(reference)?)?
            } else {
                offset
            };
            let source_start = if audio {
                optional(node, "audioStart")?.unwrap_or(in_point)
            } else {
                in_point
            };
            let clip_span = if audio {
                optional(node, "audioDuration")?.unwrap_or(span)
            } else {
                span
            };
            let mut clip = Clip::new(id, source_start, clip_span);
            // A split edit has two independent IR parts and both retain the time map.
            clip.time = if video && has_audio && !audio {
                playback.clone().unwrap_or_default()
            } else {
                playback.take().unwrap_or_default()
            };
            clip.av_group = group;
            clip.role = node
                .attribute(if audio { "audioRole" } else { "videoRole" })
                .or(node.attribute("role"))
                .map(text);
            clip.name = node.attribute("name").map(text);
            let mut placed = Placed::new(at, Item::Clip(clip), self.place(node));
            placed.counts_item = parts.is_empty();
            if let Some(name) = node.attribute("name").filter(|_| placed.counts_item) {
                placed
                    .notes
                    .push(Loss::dropped(LossCode::ClipNamesDropped).named(name));
            }
            if audio && let Item::Clip(clip) = &mut placed.item {
                self.audio(node, clip, &mut placed.notes)?;
            }
            parts.push((kind, placed));
        }
        if parts.is_empty() {
            parts.push((
                TrackKind::Video,
                Placed::new(
                    offset,
                    Item::Unsupported {
                        what: "disabled source components".into(),
                        duration: span,
                    },
                    self.place(node),
                ),
            ));
        }
        Ok(parts)
    }

    fn nested(
        &mut self,
        node: Xml<'a, 'input>,
        span: Time,
        in_point: Time,
        playback: &Playback,
        depth: usize,
        notes: &mut Vec<Loss>,
    ) -> Result<Item, Refusal> {
        if depth >= MAX_NEST_DEPTH {
            return Err(parse::too_deep());
        }
        let mut stack = self.nested_stack(node, depth + 1, notes)?;
        let speed = playback.average()?;
        let monotonic = !matches!(playback, Playback::Variable(points) if points.windows(2).any(|pair| pair[1].value < pair[0].value));
        let percent = speed
            .checked_mul(Time::integer(100))?
            .frame(Rational::new(1, 1).map_err(|_| Refusal::arithmetic())?)?
            .0;
        if monotonic && (10..=1_000).contains(&percent) {
            let end = in_point.checked_add(span.checked_mul(speed)?)?;
            parse::apply_window(&mut stack, in_point..end)?;
            stack.time.speed = speed;
            if let Playback::Variable(points) = playback {
                let mut maximum = 0;
                if let Some(first) = points.first() {
                    for point in points {
                        let averaged = point
                            .at
                            .checked_sub(first.at)?
                            .checked_mul(speed)?
                            .checked_add(first.value)?;
                        let error = point
                            .value
                            .checked_sub(averaged)?
                            .abs()?
                            .checked_div(speed)?;
                        maximum =
                            maximum.max(error.ceil_frame(self.project_rate)?.0.unsigned_abs());
                    }
                }
                notes.push(
                    Loss::approximation(LossCode::VariableSpeedAveraged)
                        .bounded(LossBound::Frames(maximum)),
                );
            }
        } else {
            stack.time.speed = Time::ZERO;
        }
        Ok(Item::Nested {
            stack,
            duration: span,
        })
    }

    fn nested_stack(
        &mut self,
        node: Xml<'a, 'input>,
        depth: usize,
        notes: &mut Vec<Loss>,
    ) -> Result<Stack, Refusal> {
        match element_name(node) {
            "ref-clip" => {
                let resource = self.resource(node)?;
                let sequence = child(resource, "sequence")
                    .ok_or_else(|| malformed(site(node), "A compound has no sequence."))?;
                let spine = child(sequence, "spine")
                    .ok_or_else(|| malformed(site(sequence), "A compound has no spine."))?;
                let origin = optional(sequence, "tcStart")?.unwrap_or(Time::ZERO);
                let mut stack = self.spine(spine, origin, depth)?;
                stack.decorations = self.decorations(sequence, Time::ZERO)?;
                prefix_stack_sites(&self.place(node), &mut stack);
                Ok(stack)
            }
            "mc-clip" => {
                let resource = self.resource(node)?;
                let multicam = child(resource, "multicam").ok_or_else(|| {
                    malformed(site(node), "A multicam clip has no multicam resource.")
                })?;
                let choices: Vec<_> = node
                    .children()
                    .filter(|source| source.has_tag_name("mc-source"))
                    .collect();
                let angle = |kind| {
                    if choices.is_empty() {
                        return multicam
                            .children()
                            .find(|angle| angle.has_tag_name("mc-angle"));
                    }
                    let chosen = choices
                        .iter()
                        .find(|source| {
                            source
                                .attribute("srcEnable")
                                .is_none_or(|enabled| enabled == "all" || enabled == kind)
                        })?
                        .attribute("angleID")?;
                    multicam.children().find(|angle| {
                        angle.has_tag_name("mc-angle") && angle.attribute("angleID") == Some(chosen)
                    })
                };
                let video = angle("video");
                let audio = angle("audio");
                if video.is_none() && audio.is_none() {
                    return Err(malformed(
                        site(node),
                        "The active multicam angle is missing.",
                    ));
                }
                let mut stack = Stack::default();
                for (kind, angle) in [(TrackKind::Video, video), (TrackKind::Audio, audio)] {
                    let Some(angle) = angle else {
                        continue;
                    };
                    if kind == TrackKind::Audio && audio == video {
                        continue;
                    }
                    notes.push(
                        Loss::approximation(LossCode::MulticamActiveAngle)
                            .named(angle.attribute("name").unwrap_or("angle")),
                    );
                    let mut take =
                        self.spine(child(angle, "spine").unwrap_or(angle), Time::ZERO, depth)?;
                    if video != audio {
                        retain_angle_kind(&mut take, kind);
                    }
                    stack.tracks.append(&mut take.tracks);
                }
                prefix_stack_sites(&self.place(node), &mut stack);
                Ok(stack)
            }
            "audition" => {
                let mut stack = Stack::default();
                let pick = node
                    .children()
                    .find(|child| is_story(*child))
                    .ok_or_else(|| malformed(site(node), "An audition has no active pick."))?;
                notes.push(
                    Loss::approximation(LossCode::AuditionActivePick)
                        .named(pick.attribute("name").unwrap_or("pick")),
                );
                for (kind, placed) in self.parts(pick, Time::ZERO, Time::ZERO, depth)? {
                    let mut track = Track::new(kind);
                    track.items.push(placed);
                    stack.tracks.push(track);
                }
                Ok(stack)
            }
            _ => self.spine(child(node, "spine").unwrap_or(node), Time::ZERO, depth),
        }
    }

    fn resource(&self, node: Xml<'a, 'input>) -> Result<Xml<'a, 'input>, Refusal> {
        node.attribute("ref")
            .and_then(|id| self.resources.get(id))
            .copied()
            .ok_or_else(|| malformed(site(node), "A container references an unknown resource."))
    }

    fn anchors(
        &mut self,
        node: Xml<'a, 'input>,
        parent: ContainerTime,
        depth: usize,
        retimed: bool,
    ) -> Result<Vec<Anchor>, Refusal> {
        let mut anchors = Vec::new();
        for element in node
            .children()
            .filter(|child| is_story(*child) && child.attribute("lane").is_some())
        {
            if depth >= MAX_NEST_DEPTH {
                return Err(parse::too_deep());
            }
            let lane = lane(element)?;
            let local = optional(element, "offset")?.unwrap_or(parent.in_point);
            for (kind, mut placed) in self.parts(element, local, local, depth + 1)? {
                if retimed {
                    placed
                        .notes
                        .push(Loss::approximation(LossCode::AnchorRetimeReading));
                }
                let mut track = Track::new(kind);
                track.items.push(placed);
                self.track_roles(&mut track);
                anchors.push(Anchor {
                    lane,
                    time: parent,
                    track,
                });
            }
        }
        Ok(anchors)
    }

    fn decorations(&mut self, node: Xml<'_, '_>, origin: Time) -> Result<Decorations, Refusal> {
        let mut result = Decorations::default();
        for element in node.children().filter(roxmltree::Node::is_element) {
            let name = element_name(element);
            let marker = match name {
                "marker" if element.attribute("completed").is_some() => Some(MarkerKind::Todo),
                "marker" => Some(MarkerKind::Plain),
                "chapter-marker" => Some(MarkerKind::Chapter),
                "keyword" | "rating" => Some(MarkerKind::KeywordOrRating),
                _ => None,
            };
            if let Some(kind) = marker {
                self.budget.item()?;
                let at = optional(element, "start")?
                    .unwrap_or(origin)
                    .checked_sub(origin)?;
                result.markers.push(Marker {
                    at,
                    duration: duration(element, Time::ZERO)?,
                    label: text(element.attribute("value").unwrap_or_default()),
                    color: None,
                    site: self.place(element),
                    kind,
                });
            } else if matches!(name, "metadata" | "note") {
                self.notes.push(Loss::dropped(LossCode::MetadataDropped));
            } else if name.starts_with("filter-")
                || (name.starts_with("adjust-")
                    && !matches!(name, "adjust-volume" | "adjust-panner"))
            {
                if node.has_tag_name("transition") {
                    continue;
                }
                self.budget.item()?;
                let label = element
                    .attribute("name")
                    .or_else(|| {
                        element
                            .attribute("ref")
                            .and_then(|id| self.resources.get(id))
                            .and_then(|resource| resource.attribute("name"))
                    })
                    .unwrap_or(name);
                result.effects.push(NamedEffect {
                    name: text(label),
                    site: self.place(element),
                    code: LossCode::EffectDropped,
                });
            }
        }
        Ok(result)
    }

    fn audio(
        &mut self,
        node: Xml<'_, '_>,
        clip: &mut Clip,
        notes: &mut Vec<Loss>,
    ) -> Result<(), Refusal> {
        if let Some(volume) = child(node, "adjust-volume") {
            clip.gain_tenth_db = tenth_db(volume.attribute("amount").unwrap_or("0dB"))?;
            let origin = optional(node, "start")?.unwrap_or(Time::ZERO);
            for key in volume
                .descendants()
                .filter(|key| key.has_tag_name("keyframe"))
            {
                self.budget.item()?;
                let at = optional(key, "time")?
                    .unwrap_or(origin)
                    .checked_sub(origin)?;
                let tenth_db = tenth_db(key.attribute("value").unwrap_or("0dB"))?;
                clip.gain_keys.push(GainKey { at, tenth_db });
            }
            for fade in volume
                .descendants()
                .filter(|fade| matches!(element_name(*fade), "fadeIn" | "fadeOut"))
            {
                let length = duration(fade, Time::ZERO)?;
                if fade.has_tag_name("fadeIn") {
                    clip.fade_in = length;
                } else {
                    clip.fade_out = length;
                }
                notes.push(
                    Loss::approximation(LossCode::FadeShapeLinear)
                        .named(fade.attribute("type").unwrap_or("linear")),
                );
            }
        }
        if child(node, "audio-channel-source").is_some() {
            notes.push(Loss::approximation(LossCode::ChannelMapDropped));
        }
        if child(node, "adjust-panner").is_some() {
            notes.push(Loss::dropped(LossCode::PanDropped));
        }
        Ok(())
    }

    fn place(&self, node: Xml<'_, '_>) -> String {
        let (line, column) = self.lines.position(node.range().start);
        format!("{line}:{column}")
    }
}

fn retain_angle_kind(stack: &mut Stack, kind: TrackKind) {
    for track in &mut stack.tracks {
        track.items.retain_mut(|placed| {
            placed.anchors.retain(|anchor| anchor.track.kind == kind);
            if let Item::Nested { stack, .. } = &mut placed.item {
                retain_angle_kind(stack, kind);
                !stack.tracks.is_empty()
            } else {
                if track.kind == kind {
                    placed.counts_item = true;
                }
                track.kind == kind
            }
        });
    }
    stack.tracks.retain(|track| !track.items.is_empty());
}

fn prefix_stack_sites(prefix: &str, stack: &mut Stack) {
    prefix_decoration_sites(prefix, &mut stack.decorations);
    for track in &mut stack.tracks {
        prefix_track_sites(prefix, track);
    }
}

fn prefix_track_sites(prefix: &str, track: &mut Track) {
    prefix_decoration_sites(prefix, &mut track.decorations);
    for placed in &mut track.items {
        placed.site = format!("{prefix}/{}", placed.site);
        prefix_decoration_sites(prefix, &mut placed.decorations);
        match &mut placed.item {
            Item::Nested { stack, .. } => prefix_stack_sites(prefix, stack),
            Item::Clip(clip) => prefix_decoration_sites(prefix, &mut clip.decorations),
            Item::Title { decorations, .. } => prefix_decoration_sites(prefix, decorations),
            _ => {}
        }
        for anchor in &mut placed.anchors {
            prefix_track_sites(prefix, &mut anchor.track);
        }
    }
}

fn prefix_decoration_sites(prefix: &str, decorations: &mut Decorations) {
    for site in decorations
        .markers
        .iter_mut()
        .map(|marker| &mut marker.site)
        .chain(
            decorations
                .effects
                .iter_mut()
                .map(|effect| &mut effect.site),
        )
        .chain(
            decorations
                .removed
                .iter_mut()
                .map(|removed| &mut removed.site),
        )
        .chain(&mut decorations.retained_effects)
    {
        *site = format!("{prefix}/{site}");
    }
}

fn transition_track_kind(node: Xml<'_, '_>) -> TrackKind {
    if child(node, "filter-audio").is_some() && child(node, "filter-video").is_none() {
        TrackKind::Audio
    } else {
        TrackKind::Video
    }
}

fn mirror_audio_transitions(tracks: &mut BTreeMap<(bool, bool, i64), Track>) {
    let mut pairs = Vec::new();
    for ((audio, _, lane), track) in &*tracks {
        if *audio {
            continue;
        }
        let transitions: Vec<_> = track
            .items
            .iter()
            .filter(|placed| matches!(placed.item, Item::Transition { .. }))
            .map(|placed| {
                let mut copy = Placed::new(placed.at, placed.item.clone(), &placed.site);
                copy.counts_item = false;
                copy
            })
            .collect();
        if !transitions.is_empty() {
            pairs.push((*lane, transitions));
        }
    }
    for (lane, transitions) in pairs {
        let Some(audio) = tracks.get_mut(&(true, lane != 0, lane)) else {
            continue;
        };
        let boundaries: BTreeSet<_> = audio
            .items
            .iter()
            .flat_map(|placed| {
                [
                    Some(placed.at),
                    placed.at.checked_add(placed.item.span()).ok(),
                ]
            })
            .flatten()
            .collect();
        let explicit_cuts: BTreeSet<_> = audio
            .items
            .iter()
            .filter(|placed| matches!(placed.item, Item::Transition { .. }))
            .map(|placed| placed.at)
            .collect();
        audio.items.extend(transitions.into_iter().filter(|placed| {
            boundaries.contains(&placed.at) && !explicit_cuts.contains(&placed.at)
        }));
        audio
            .items
            .sort_by_key(|placed| (placed.at, !matches!(placed.item, Item::Transition { .. })));
    }
}

fn is_story(node: Xml<'_, '_>) -> bool {
    matches!(
        element_name(node),
        "asset-clip"
            | "clip"
            | "video"
            | "audio"
            | "ref-clip"
            | "sync-clip"
            | "mc-clip"
            | "audition"
            | "spine"
            | "gap"
            | "transition"
            | "title"
            | "caption"
    ) || node.is_element()
        && node.attribute("duration").is_some()
        && node.attribute("offset").is_some()
}

fn lane(node: Xml<'_, '_>) -> Result<i64, Refusal> {
    node.attribute("lane").map_or(Ok(0), |value| {
        value
            .parse()
            .map_err(|_| malformed(site(node), "The clip lane is invalid."))
    })
}

fn title_text(node: Xml<'_, '_>) -> String {
    let mut text = String::new();
    for fragment in node.descendants().filter(|node| {
        node.is_text()
            && node
                .parent()
                .is_some_and(|parent| parent.has_tag_name("text-style"))
    }) {
        let part = fragment.text().unwrap_or_default();
        let remaining = MAX_TEXT_BYTES - text.len();
        text.push_str(crate::shorten(part, remaining));
        if part.len() > remaining {
            break;
        }
    }
    text
}

fn tenth_db(value: &str) -> Result<i64, Refusal> {
    let value = value.trim().strip_suffix("dB").unwrap_or(value).trim();
    if value == "-inf" {
        return Ok(i64::MIN);
    }
    let rate = Rational::new(10, 1).map_err(|_| Refusal::arithmetic())?;
    Ok(Time::decimal(value)?.frame(rate)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_newline_xml_position_index_is_sparse() {
        let bytes = vec![b'\n'; 16 * 1024];
        let index = LineIndex::new(&bytes);
        let index_bytes = index.prefixes.capacity() * size_of::<(usize, usize)>();
        assert!(
            index_bytes <= bytes.len() / 4,
            "{index_bytes} index bytes for {} input bytes",
            bytes.len()
        );
        assert_eq!(index.position(0), (1, 1));
        assert_eq!(index.position(bytes.len()), (bytes.len() + 1, 1));
    }

    #[test]
    fn xml_position_index_keeps_columns_across_chunks() {
        let mut bytes = vec![b' '; 2 * LINE_INDEX_CHUNK_BYTES + 1];
        bytes[1] = b'\n';
        bytes[2 * LINE_INDEX_CHUNK_BYTES] = b'\n';
        let index = LineIndex::new(&bytes);
        assert_eq!(
            index.position(2 * LINE_INDEX_CHUNK_BYTES),
            (2, 2 * LINE_INDEX_CHUNK_BYTES - 1)
        );
        assert_eq!(index.position(bytes.len()), (3, 1));
    }
}

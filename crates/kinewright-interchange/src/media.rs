use std::{ops::Range, path::PathBuf};

use kinewright_core::{
    AssetId, ColorDescription, MediaAsset, MediaCounts, MediaKind, MediaSourceFingerprint,
    Rational, RemoteLocator, TimeCode,
};

use crate::{
    MAX_MEDIA, MAX_NEST_DEPTH, Refusal, RefusalCode, Time, check_rate,
    ir::{Item, Media, MediaId, Playback, Timeline, Track},
    locator::{self, Locator, Platform},
    parse,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedMedia {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub fps: Rational,
    pub duration: TimeCode,
    pub resolution: Option<(u32, u32)>,
    pub source_fingerprint: MediaSourceFingerprint,
    pub color_description: ColorDescription,
    pub relinked_by_name: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// Let lowering classify the written locator as a remote or an offline file.
    Missing,
    /// The project layer determined that the local or approved file is offline.
    Unavailable,
    Remote,
    TimedOut,
    /// Inspection found the file but did not probe or hash it.
    Unprobed {
        path: PathBuf,
        relinked_by_name: bool,
    },
    Probed(Box<ProbedMedia>),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MediaTable {
    /// Entries are indexed by the timeline's `MediaId`. Missing entries are offline.
    pub entries: Vec<Availability>,
}

#[derive(Clone, Copy)]
pub(crate) struct AssetInfo {
    pub id: AssetId,
    pub fps: Rational,
    pub kind: MediaKind,
    pub duration: TimeCode,
    pub origin: Time,
    pub origin_known: bool,
    pub probed: bool,
    pub ranges_merged: bool,
}

pub(crate) struct Assets {
    pub info: Vec<Option<AssetInfo>>,
    pub pool: Vec<MediaAsset>,
    pub counts: MediaCounts,
    pub estimated: Vec<Range<AssetId>>,
    pub relinked_by_name: u32,
}

impl Assets {
    pub fn build(timeline: &Timeline, table: &MediaTable) -> Result<Self, Refusal> {
        if timeline.media.len() > MAX_MEDIA || table.entries.len() > MAX_MEDIA {
            return Err(parse::too_many_items());
        }
        let mut extents = vec![None; timeline.media.len()];
        let mut tracks: Vec<_> = timeline
            .root
            .tracks
            .iter()
            .map(|track| (track, 0))
            .collect();
        while let Some((track, depth)) = tracks.pop() {
            accumulate(track, depth, &timeline.media, &mut extents, &mut tracks)?;
        }
        let mut result = Self {
            info: vec![None; timeline.media.len()],
            pool: Vec::new(),
            counts: MediaCounts::default(),
            estimated: Vec::new(),
            relinked_by_name: 0,
        };
        for online in [true, false] {
            for (index, (media, used)) in timeline.media.iter().zip(&extents).enumerate() {
                let Some(used) = used else {
                    continue;
                };
                let availability = table.entries.get(index).unwrap_or(&Availability::Missing);
                if online
                    != matches!(
                        availability,
                        Availability::Probed(_) | Availability::Unprobed { .. }
                    )
                {
                    continue;
                }
                result.push(index, media, *used, timeline.rate_hint, availability)?;
            }
        }
        if result.counts.offline > 0 {
            let first = u64::from(result.counts.online) + 1;
            let end = u64::from(result.counts.assets) + 1;
            result.estimated.push(AssetId(first)..AssetId(end));
        }
        Ok(result)
    }

    fn push(
        &mut self,
        index: usize,
        media: &Media,
        furthest: Time,
        project: Rational,
        availability: &Availability,
    ) -> Result<(), Refusal> {
        let id = AssetId(u64::from(self.counts.assets) + 1);
        let origin = media.start.unwrap_or(Time::ZERO);
        let estimate_rate = media.fps.unwrap_or(if media.kind == MediaKind::Audio {
            Rational::default()
        } else {
            project
        });
        let estimate_rate = check_rate(estimate_rate)?;
        let estimate = || -> Result<TimeCode, Refusal> {
            let duration = media.duration.unwrap_or(furthest);
            Ok(duration.ceil_frame(estimate_rate)?.max(TimeCode(1)))
        };
        let mut asset = MediaAsset {
            remote_locator: None,
            id,
            path: PathBuf::from(&media.locator),
            name: if media.name.is_empty() {
                "Unidentified media".into()
            } else {
                media.name.clone()
            },
            duration: TimeCode(1),
            fps: estimate_rate,
            kind: media.kind,
            resolution: valid_resolution(media.resolution),
            source_fingerprint: MediaSourceFingerprint::unknown(),
            color_description: ColorDescription::default(),
            assumed_from: None,
        };
        match availability {
            Availability::Probed(probe) => {
                // The document owns its media metadata and the caller may reuse the table.
                asset.path.clone_from(&probe.path);
                asset
                    .source_fingerprint
                    .clone_from(&probe.source_fingerprint);
                asset.color_description.clone_from(&probe.color_description);
                asset.kind = probe.kind;
                asset.fps = check_rate(probe.fps)?;
                asset.resolution = valid_resolution(probe.resolution);
                asset.duration = probe.duration.max(TimeCode(1));
                self.counts.online += 1;
                self.relinked_by_name += u32::from(probe.relinked_by_name);
            }
            Availability::Unprobed {
                path,
                relinked_by_name,
            } => {
                asset.path.clone_from(path);
                asset.duration = estimate()?;
                self.counts.online += 1;
                self.relinked_by_name += u32::from(*relinked_by_name);
            }
            _ => {
                asset.duration = estimate()?;
                self.counts.offline += 1;
                let remote = *availability == Availability::Remote
                    || locator::classify(&media.locator, Platform::NATIVE) == Locator::Remote;
                if remote {
                    asset.path = PathBuf::new();
                    asset.remote_locator = Some(RemoteLocator::new(media.locator.clone()));
                }
                if *availability == Availability::TimedOut {
                    self.counts.timed_out += 1;
                } else if remote
                    && matches!(availability, Availability::Missing | Availability::Remote)
                {
                    self.counts.remote += 1;
                }
            }
        }
        self.info[index] = Some(AssetInfo {
            id,
            fps: asset.fps,
            kind: asset.kind,
            duration: asset.duration,
            origin,
            origin_known: media.start.is_some(),
            probed: matches!(availability, Availability::Probed(_)),
            ranges_merged: media.ranges_merged && !matches!(availability, Availability::Probed(_)),
        });
        self.pool.push(asset);
        self.counts.assets += 1;
        Ok(())
    }
}

pub(crate) fn valid_resolution(resolution: Option<(u32, u32)>) -> Option<(u32, u32)> {
    resolution.filter(|&(width, height)| width > 0 && height > 0)
}

fn accumulate<'a>(
    track: &'a Track,
    depth: usize,
    media: &[Media],
    extents: &mut [Option<Time>],
    tracks: &mut Vec<(&'a Track, usize)>,
) -> Result<(), Refusal> {
    if depth > MAX_NEST_DEPTH {
        return Err(parse::too_deep());
    }
    for placed in &track.items {
        if placed.item.zero_duration_count().is_some() {
            continue;
        }
        for anchor in &placed.anchors {
            tracks.push((&anchor.track, depth + 1));
        }
        match &placed.item {
            Item::Clip(clip) => {
                let invalid = || {
                    Refusal::new(
                        RefusalCode::Malformed,
                        "A clip references a missing IR media entry.",
                    )
                };
                let MediaId(index) = clip.media;
                let origin = media
                    .get(index)
                    .ok_or_else(invalid)?
                    .start
                    .unwrap_or(Time::ZERO);
                let start = clip.source_start.checked_sub(origin)?;
                let used = match clip.time {
                    Playback::Freeze(hold) => hold.checked_sub(origin)?.checked_add(Time::ONE)?,
                    _ => start.checked_add(clip.span.checked_mul(clip.time.average()?.abs()?)?)?,
                };
                extents[index] = Some(extents[index].unwrap_or(Time::ZERO).max(used));
            }
            Item::Nested { stack, .. } if stack.time.speed > Time::ZERO => {
                tracks.extend(stack.tracks.iter().map(|track| (track, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

use std::{collections::BTreeMap, ops::Range};

use kinewright_core::{
    ClipId, EffectId, InterchangeFormat, LinkId, LossCode, MarkerId, TimeCode, Track, TrackId,
    TrackKind,
};

use crate::{
    MAX_CLIPS, Refusal, Time,
    ir::{Item, Loss},
    parse,
};

use super::{
    Location, State,
    clips::Candidate,
    layout::{Block, Lane, Pair},
};

impl State {
    pub(super) fn pair(&self, location: Location<'_>, candidate: &Candidate) -> Option<Pair> {
        let Item::Clip(clip) = &location.placed.item else {
            return None;
        };
        if let Some(group) = clip.av_group {
            return Some(Pair::Explicit(group));
        }
        (self.format == InterchangeFormat::Otio).then_some(Pair::Otio {
            asset: candidate.clip.asset,
            start: location.start,
            source: clip.source_start,
        })
    }

    pub(super) fn collapse_dual_mono(&mut self, tracks: &mut [Lane]) {
        let mut lowest = BTreeMap::<_, usize>::new();
        for (lane, track) in tracks.iter_mut().enumerate() {
            track.clips.retain(|&at, stored| {
                let Some(pair @ Pair::Otio { .. }) = stored.pair else {
                    return true;
                };
                let key = (pair, stored.raw_end, stored.candidate.head);
                let first = *lowest.entry(key).or_insert(lane);
                if first == lane {
                    return true;
                }
                if !stored.candidate.head {
                    self.report.loss(
                        &Loss::approximation(LossCode::ChannelMapDropped),
                        &stored.source,
                        at,
                        None,
                    );
                }
                false
            });
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Track emission assigns all output IDs from one sequence"
    )]
    pub(super) fn output_tracks(
        &mut self,
        block: Block,
    ) -> Result<(Vec<Track>, TimeCode), Refusal> {
        let mut videos = BTreeMap::new();
        let mut audios = BTreeMap::new();
        let mut video_ranges = BTreeMap::<_, Vec<(Time, Time)>>::new();
        for (kind, lanes, keys) in [
            (TrackKind::Video, &block.video, &mut videos),
            (TrackKind::Audio, &block.audio, &mut audios),
        ] {
            for stored in lanes.iter().flat_map(|lane| lane.clips.values()) {
                if let Some(pair) = stored.pair {
                    keys.entry((pair, stored.candidate.head))
                        .or_insert(stored.candidate.clip.id);
                    if kind == TrackKind::Video
                        && !stored.candidate.head
                        && let Pair::Otio { asset, start, .. } = pair
                    {
                        video_ranges
                            .entry(asset)
                            .or_default()
                            .push((start, stored.raw_end));
                    }
                }
            }
        }
        for ranges in video_ranges.values_mut() {
            ranges.sort_unstable();
            let mut end = Time::ZERO;
            for (_, edge) in ranges {
                end = end.max(*edge);
                *edge = end;
            }
        }
        let mut links = BTreeMap::new();
        let mut tracks = Vec::new();
        let mut duration = TimeCode::ZERO;
        let assets = u64::from(self.assets.counts.assets);
        let track_count = block.video.len() + block.audio.len();
        let clip_count = block
            .video
            .iter()
            .chain(&block.audio)
            .map(|lane| lane.clips.len())
            .sum::<usize>();
        if clip_count > MAX_CLIPS {
            return Err(parse::too_many_items());
        }
        let mut ids = BTreeMap::new();
        let mut next_clip =
            assets + u64::try_from(track_count).map_err(|_| Refusal::arithmetic())?;
        let mut next_link =
            next_clip + u64::try_from(clip_count).map_err(|_| Refusal::arithmetic())?;
        for (kind, lanes) in [
            (TrackKind::Video, block.video),
            (TrackKind::Audio, block.audio),
        ] {
            for lane in lanes {
                let mut clips = Vec::with_capacity(lane.clips.len());
                for (at, stored) in lane.clips {
                    let mut clip = stored.candidate.clip;
                    let head = stored.candidate.head;
                    duration = duration.max(TimeCode(stored.candidate.end));
                    if let Some(pair) = stored.pair {
                        let key = (pair, head);
                        let (own, other) = if kind == TrackKind::Video {
                            (&videos, &audios)
                        } else {
                            (&audios, &videos)
                        };
                        if own.get(&key) == Some(&clip.id) && other.contains_key(&key) {
                            clip.link = Some(*links.entry(key).or_insert_with(|| {
                                next_link += 1;
                                LinkId(next_link)
                            }));
                        } else if kind == TrackKind::Audio
                            && !head
                            && let Pair::Otio { asset, start, .. } = pair
                            && video_ranges
                                .get(&asset)
                                .is_some_and(|ranges| overlaps(ranges, start..stored.raw_end))
                        {
                            self.report.loss(
                                &Loss::approximation(LossCode::AvLinkUnpaired),
                                &stored.source,
                                at,
                                Some(clip.id),
                            );
                        }
                    }
                    next_clip += 1;
                    ids.insert(clip.id, ClipId(next_clip));
                    clip.id = ClipId(next_clip);
                    clips.push(clip);
                }
                let id = TrackId(assets + u64::try_from(tracks.len()).unwrap_or_default() + 1);
                tracks.push(Track {
                    id,
                    kind,
                    name: lane.name,
                    sync_lock: true,
                    clips,
                });
            }
        }
        self.report.remap_clips(&ids);
        self.markers
            .sort_by_key(|marker| (marker.position, marker.id));
        for marker in &mut self.markers {
            next_link += 1;
            marker.id = MarkerId(next_link);
        }
        for effect in tracks
            .iter_mut()
            .flat_map(|track| &mut track.clips)
            .flat_map(|clip| &mut clip.effects)
        {
            next_link += 1;
            effect.id = EffectId(next_link);
        }
        Ok((tracks, duration))
    }
}

fn overlaps(ranges: &[(Time, Time)], target: Range<Time>) -> bool {
    let before_end = ranges.partition_point(|(start, _)| *start < target.end);
    before_end > 0 && ranges[before_end - 1].1 > target.start
}

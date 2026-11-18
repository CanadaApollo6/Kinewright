use std::collections::BTreeMap;

use kinewright_core::{AssetId, TrackKind};

use crate::{MAX_TRACKS_PER_KIND, Refusal, RefusalCode, Time};

use super::clips::Candidate;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Pair {
    Explicit(u32),
    Otio {
        asset: AssetId,
        start: Time,
        source: Time,
    },
}

pub(super) struct Stored {
    pub candidate: Candidate,
    pub pair: Option<Pair>,
    pub source: String,
    pub raw_end: Time,
}

#[derive(Default)]
pub(super) struct Lane {
    pub name: Option<String>,
    pub clips: BTreeMap<i64, Stored>,
}

impl Lane {
    pub fn fits(&self, start: i64, end: i64) -> bool {
        self.clips
            .range(..=start)
            .next_back()
            .is_none_or(|(_, previous)| previous.candidate.end <= start)
            && self
                .clips
                .range(start..)
                .next()
                .is_none_or(|(&next, _)| next >= end)
    }
}

#[derive(Default)]
pub(super) struct Block {
    pub video: Vec<Lane>,
    pub audio: Vec<Lane>,
}

impl Block {
    pub fn lanes(&mut self, kind: TrackKind) -> &mut Vec<Lane> {
        match kind {
            TrackKind::Video => &mut self.video,
            TrackKind::Audio => &mut self.audio,
        }
    }

    pub fn append(&mut self, other: Self) -> Result<(), Refusal> {
        for (ours, theirs) in [
            (&mut self.video, other.video),
            (&mut self.audio, other.audio),
        ] {
            if ours.len() + theirs.len() > MAX_TRACKS_PER_KIND {
                return Err(too_many_tracks());
            }
            ours.extend(theirs);
        }
        Ok(())
    }

    pub fn overlay(&mut self, other: Self) -> Result<bool, Refusal> {
        let mut spill = false;
        for (ours, theirs) in [
            (&mut self.video, other.video),
            (&mut self.audio, other.audio),
        ] {
            let fits = ours.iter().zip(&theirs).all(|(lane, added)| {
                added
                    .clips
                    .iter()
                    .all(|(&start, clip)| lane.fits(start, clip.candidate.end))
            });
            if fits {
                if ours.len() < theirs.len() {
                    ours.resize_with(theirs.len(), Lane::default);
                }
                for (lane, mut added) in ours.iter_mut().zip(theirs) {
                    if lane.name.is_none() {
                        lane.name = added.name.take();
                    }
                    lane.clips.append(&mut added.clips);
                }
            } else {
                if ours.len() + theirs.len() > MAX_TRACKS_PER_KIND {
                    return Err(too_many_tracks());
                }
                ours.extend(theirs);
                spill = true;
            }
        }
        Ok(spill)
    }
}

pub(super) fn too_many_tracks() -> Refusal {
    Refusal::new(
        RefusalCode::TooManyTracks,
        "The import exceeds 128 tracks of one kind, including lanes.",
    )
}

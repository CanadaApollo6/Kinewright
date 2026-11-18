use std::ops::Range;

use kinewright_core::{LossBound, LossCode, Marker, MarkerId, TimeCode};

use crate::{
    Refusal, Time,
    ir::{self, ContainerTime, Item, Loss, MarkerKind, Timeline},
};

use super::{Location, State};

impl State {
    pub(super) fn initial_report(&mut self, timeline: &Timeline, start: i64, inferred: bool) {
        if start != 0 {
            self.timeline_loss(&Loss::approximation(LossCode::StartRebased));
        }
        if inferred {
            self.timeline_loss(&Loss::approximation(LossCode::ResolutionInferred));
        }
        self.timeline_loss(&Loss::approximation(LossCode::ColorSpaceDefault));
        for note in &timeline.notes {
            self.timeline_loss(note);
        }
        for (code, count) in [
            (LossCode::MediaOffline, self.assets.counts.offline),
            (LossCode::MediaTimeout, self.assets.counts.timed_out),
            (LossCode::RemoteUrl, self.assets.counts.remote),
            (LossCode::RelinkedByName, self.assets.relinked_by_name),
        ] {
            self.report
                .loss_many(&Loss::approximation(code), "", 0, None, count);
        }
    }

    fn timeline_loss(&mut self, loss: &Loss) {
        self.report.loss(loss, "", 0, None);
    }

    pub(super) fn item_decorations(
        &mut self,
        location: Location<'_>,
        map: ContainerTime,
        start: i64,
        end: i64,
    ) -> Result<(), Refusal> {
        if matches!(&location.placed.item, Item::Nested { stack, .. } if stack.time.speed <= Time::ZERO)
        {
            return Ok(());
        }
        let item_map = ContainerTime {
            offset: location.start,
            in_point: Time::ZERO,
            speed: map.speed,
        };
        let window = start..end;
        self.decorations(&location.placed.decorations, item_map, Some(&window))?;
        match &location.placed.item {
            Item::Clip(clip) => self.decorations(&clip.decorations, item_map, Some(&window)),
            Item::Title { decorations, .. } => {
                self.decorations(decorations, item_map, Some(&window))
            }
            _ => Ok(()),
        }
    }

    pub(super) fn decorations(
        &mut self,
        decorations: &ir::Decorations,
        map: ContainerTime,
        window: Option<&Range<i64>>,
    ) -> Result<(), Refusal> {
        for removed in &decorations.removed {
            self.report.item(&removed.site, removed.count);
            self.report
                .loss_many(&removed.loss, &removed.site, 0, None, removed.count);
        }
        for effect in &decorations.effects {
            self.report.item(&effect.site, 1);
            self.report.loss(
                &Loss::dropped(effect.code).named(&effect.name),
                &effect.site,
                0,
                None,
            );
        }
        for marker in &decorations.markers {
            let frame = map.to_parent(marker.at)?.frame(self.project)?.0;
            self.marker(marker, frame, window);
        }
        Ok(())
    }

    fn marker(&mut self, marker: &ir::Marker, frame: i64, window: Option<&Range<i64>>) {
        let site = &marker.site;
        self.report.item(site, 1);
        let mut loss = |loss| self.report.loss(&loss, site, frame, None);
        if frame < 0 || window.is_some_and(|window| !window.contains(&frame)) {
            loss(Loss::dropped(LossCode::MarkerOutsideItem));
            return;
        }
        let prefix = match marker.kind {
            MarkerKind::Plain => "",
            MarkerKind::Chapter => "[chapter] ",
            MarkerKind::Todo => "[to-do] ",
            MarkerKind::KeywordOrRating => {
                loss(Loss::dropped(LossCode::KeywordRatingDropped).named(&marker.label));
                return;
            }
            MarkerKind::Unknown => {
                loss(Loss::dropped(LossCode::UnknownItem));
                return;
            }
        };
        if window.is_some() {
            loss(Loss::approximation(LossCode::ItemMarkerPinned));
        }
        if marker.duration > Time::ZERO {
            loss(Loss::approximation(LossCode::MarkerDurationDropped));
        }
        let mut label = String::from(prefix);
        if let Some(color) = &marker.color {
            match MARKER_COLOR_POLICY {
                MarkerColorPolicy::LabelPrefix => {
                    label.push('[');
                    label.push_str(&color.to_lowercase());
                    label.push_str("] ");
                    loss(Loss::approximation(LossCode::MarkerColorAsLabel).named(color));
                }
                MarkerColorPolicy::Drop => {
                    loss(Loss::dropped(LossCode::MarkerColorDropped).named(color));
                }
            }
        }
        label.push_str(&marker.label);
        self.markers.push(Marker {
            id: MarkerId(u64::try_from(self.markers.len()).unwrap_or_default() + 1),
            position: TimeCode(frame),
            label,
            color_token: 0,
        });
    }

    pub(super) fn finish_gaps(&mut self, duration: i64) {
        let mut last = duration;
        for (site, start, end) in &self.gaps {
            if *start < duration {
                self.report.item(site, 1);
            }
            last = last.max(*end);
        }
        if last > duration {
            self.timeline_loss(
                &Loss::approximation(LossCode::TrailingGapTrimmed)
                    .bounded(LossBound::Frames(last.abs_diff(duration))),
            );
        }
    }
}

enum MarkerColorPolicy {
    LabelPrefix,
    #[allow(dead_code, reason = "Ruling 4 leaves the shipped choice open")]
    Drop,
}

const MARKER_COLOR_POLICY: MarkerColorPolicy = MarkerColorPolicy::LabelPrefix;

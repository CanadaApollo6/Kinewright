use kinewright_core::{Rational, TimeCode, map_frames, map_project_duration_to_source};

use crate::{Refusal, Time};

#[derive(Clone, Copy)]
pub(super) struct Grid {
    source: Rational,
    project: Rational,
    base: i64,
}

impl Grid {
    pub fn new(source: Rational, project: Rational, start: TimeCode) -> Result<Self, Refusal> {
        let base = map_frames(start, source, project)
            .map_err(|_| Refusal::arithmetic())?
            .0;
        Ok(Self {
            source,
            project,
            base,
        })
    }

    pub fn duration(self, end: i64) -> i64 {
        map_frames(TimeCode(end), self.source, self.project)
            .map_or(i64::MAX, |frame| frame.0 - self.base)
    }

    pub fn exact_end(self, start: i64, duration: i64) -> Option<TimeCode> {
        map_project_duration_to_source(
            TimeCode(start),
            TimeCode(duration),
            self.source,
            self.project,
        )
        .ok()
    }

    pub fn last_at_most(self, mut low: i64, mut high: i64, span: i64) -> i64 {
        while low < high {
            let difference = high - low;
            let middle = low + difference / 2 + difference % 2;
            if self.duration(middle) > span {
                high = middle - 1;
            } else {
                low = middle;
            }
        }
        low
    }

    pub fn first_at_least(self, mut low: i64, mut high: i64, span: i64) -> i64 {
        while low < high {
            let middle = low + (high - low) / 2;
            if self.duration(middle) >= span {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        low
    }

    pub fn nearest_end(
        self,
        source_start: i64,
        media_end: i64,
        start: i64,
        target: Time,
        limit: i64,
    ) -> Result<Option<TimeCode>, Refusal> {
        if source_start >= media_end || start >= limit {
            return Ok(None);
        }
        let last = self.last_at_most(source_start, media_end, limit - start);
        if self.duration(last) == 0 {
            return Ok(None);
        }
        let preferred = target.floor_frame(self.project)?.0.saturating_sub(start);
        let shorter = if preferred > 0 {
            self.duration(self.last_at_most(source_start, last, preferred))
        } else {
            0
        };
        let longer = if preferred < self.duration(last) {
            let first = self.first_at_least(source_start, last, preferred.saturating_add(1).max(1));
            Some(self.duration(first))
        } else {
            None
        };
        let decided = match (shorter, longer) {
            (0, Some(longer)) => longer,
            (shorter, Some(longer)) => {
                let midpoint = Time::from_frame(TimeCode(start + shorter), self.project)?
                    .checked_add(Time::from_frame(TimeCode(start + longer), self.project)?)?
                    .checked_div(Time::integer(2))?;
                if target > midpoint { longer } else { shorter }
            }
            (shorter, None) => shorter,
        };
        self.exact_end(source_start, decided)
            .map(Some)
            .ok_or_else(Refusal::arithmetic)
    }
}

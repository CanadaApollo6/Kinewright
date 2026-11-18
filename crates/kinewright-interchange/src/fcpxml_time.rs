use std::cmp::Ordering;

use kinewright_core::Rational;

use crate::{
    Refusal, Time,
    ir::{Playback, TimePoint},
    parse,
};

type Xml<'a, 'input> = roxmltree::Node<'a, 'input>;

pub(crate) fn time(value: &str) -> Result<Time, Refusal> {
    let value = value.strip_suffix('s').ok_or_else(Refusal::arithmetic)?;
    if let Some((numerator, denominator)) = value.split_once('/') {
        return Time::new(
            numerator.parse().map_err(|_| Refusal::arithmetic())?,
            denominator.parse().map_err(|_| Refusal::arithmetic())?,
        );
    }
    Time::decimal(value)
}

pub(crate) fn site(node: Xml<'_, '_>) -> String {
    let pos = node.document().text_pos_at(node.range().start);
    format!("{}:{}", pos.row, pos.col)
}

pub(crate) fn optional(node: Xml<'_, '_>, attribute: &str) -> Result<Option<Time>, Refusal> {
    node.attribute(attribute)
        .map(|value| time(value).map_err(|error| error.at(site(node))))
        .transpose()
}

pub(crate) fn duration(node: Xml<'_, '_>, fallback: Time) -> Result<Time, Refusal> {
    let duration = optional(node, "duration")?.unwrap_or(fallback);
    if duration < Time::ZERO {
        return Err(parse::malformed(site(node), "A duration is negative."));
    }
    Ok(duration)
}

pub(crate) fn source_map(
    node: Xml<'_, '_>,
    start: Time,
    span: Time,
    asset_fps: Option<Rational>,
    project_fps: Rational,
) -> Result<(Time, Playback), Refusal> {
    let Some(map) = node.children().find(|child| child.has_tag_name("timeMap")) else {
        let conform = node
            .children()
            .find(|child| child.has_tag_name("conform-rate"));
        let source_fps = conform
            .and_then(|conform| conform.attribute("srcFrameRate"))
            .map(conform_fps)
            .transpose()?
            .or(asset_fps);
        let speed = if conform.is_some_and(|conform| conform.attribute("scaleEnabled") == Some("1"))
            && let Some(fps) = source_fps
        {
            Playback::Speed(Time::rate(project_fps).checked_div(Time::rate(fps))?)
        } else {
            Playback::Normal
        };
        return Ok((start, speed));
    };
    let mut points = Vec::new();
    for point in map.children().filter(|child| child.has_tag_name("timept")) {
        let at = optional(point, "time")?
            .ok_or_else(|| parse::malformed(site(point), "A time map point has no time."))?;
        let value = optional(point, "value")?
            .ok_or_else(|| parse::malformed(site(point), "A time map point has no value."))?;
        if points
            .last()
            .is_some_and(|previous: &TimePoint| previous.at >= at)
        {
            return Err(parse::malformed(
                site(point),
                "Time map points must have increasing times.",
            ));
        }
        points.push(TimePoint { at, value });
    }
    match points.as_slice() {
        [] => return Ok((start, Playback::Normal)),
        [point] => return Ok((point.value, Playback::Freeze(point.value))),
        _ => {}
    }
    let first = interpolate(&points, start)?;
    let end = start.checked_add(span)?;
    let last = interpolate(&points, end)?;
    if span == Time::ZERO {
        return Ok((first, Playback::Freeze(first)));
    }
    points.retain(|point| point.at > start && point.at < end);
    points.insert(
        0,
        TimePoint {
            at: start,
            value: first,
        },
    );
    points.push(TimePoint {
        at: end,
        value: last,
    });
    let playback = if points.iter().all(|point| point.value == first) {
        Playback::Freeze(first)
    } else if points.len() == 2 {
        let speed = last.checked_sub(first)?.checked_div(span)?;
        match speed.cmp(&Time::ZERO) {
            Ordering::Less => Playback::Reverse(speed.abs()?),
            Ordering::Equal => Playback::Freeze(first),
            Ordering::Greater => Playback::Speed(speed),
        }
    } else {
        Playback::Variable(points)
    };
    Ok((first, playback))
}

fn interpolate(points: &[TimePoint], at: Time) -> Result<Time, Refusal> {
    let next = points
        .partition_point(|point| point.at <= at)
        .clamp(1, points.len() - 1);
    let from = points[next - 1];
    let to = points[next];
    if at == from.at {
        return Ok(from.value);
    }
    let fraction = at
        .checked_sub(from.at)?
        .checked_div(to.at.checked_sub(from.at)?)?;
    from.value
        .checked_add(fraction.checked_mul(to.value.checked_sub(from.value)?)?)
}

fn conform_fps(value: &str) -> Result<Rational, Refusal> {
    match value {
        "23.98" | "23.976" => Time::new(24_000, 1_001)?,
        "29.97" => Time::new(30_000, 1_001)?,
        "47.95" | "47.952" => Time::new(48_000, 1_001)?,
        "59.94" => Time::new(60_000, 1_001)?,
        value => Time::decimal(value)?,
    }
    .as_rate()
}

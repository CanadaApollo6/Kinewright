use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CaptionPreset, ClipContent, ClipId, Document, TimeCode, TrackId, TrackKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QaSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QaIssue {
    pub severity: QaSeverity,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<std::ops::Range<TimeCode>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QaReport {
    pub document_duration: TimeCode,
    pub issues: Vec<QaIssue>,
}

impl QaReport {
    #[must_use]
    pub fn count(&self, severity: QaSeverity) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == severity)
            .count()
    }

    #[must_use]
    pub fn export_ready(&self) -> bool {
        self.count(QaSeverity::Error) == 0
    }
}

/// Run deterministic structural and delivery checks against a document snapshot.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn qa_document(document: &Document) -> QaReport {
    let mut issues = Vec::new();
    if document.duration <= TimeCode::ZERO {
        issues.push(issue(
            QaSeverity::Error,
            "empty_timeline",
            "The timeline has no renderable duration.",
            None,
            None,
            None,
        ));
    }
    for asset in &document.media_pool {
        if !asset.path.exists() {
            issues.push(issue(
                QaSeverity::Error,
                "missing_media",
                format!("Media file is missing: {}", asset.path.display()),
                None,
                None,
                None,
            ));
        }
    }

    let mut has_audio_track = false;
    for track in &document.tracks {
        has_audio_track |= track.kind == TrackKind::Audio && !track.clips.is_empty();
        let mut previous_end = TimeCode::ZERO;
        let mut previous_was_media = false;
        for clip in &track.clips {
            let duration = document.clip_duration(clip).unwrap_or(TimeCode::ZERO);
            let end = TimeCode(clip.timeline_start.0.saturating_add(duration.0));
            if clip.timeline_start > previous_end {
                issues.push(issue(
                    QaSeverity::Warning,
                    "track_gap",
                    format!(
                        "Track {} has a gap from frame {} to {}.",
                        track.id, previous_end.0, clip.timeline_start.0
                    ),
                    Some(track.id),
                    Some(clip.id),
                    Some(previous_end..clip.timeline_start),
                ));
            } else if previous_was_media
                && matches!(clip.content, ClipContent::Media)
                && clip.transition_in.is_none()
            {
                issues.push(issue(
                    QaSeverity::Info,
                    "abrupt_cut",
                    format!("Clip {} starts with a hard cut.", clip.id),
                    Some(track.id),
                    Some(clip.id),
                    Some(clip.timeline_start..TimeCode(clip.timeline_start.0.saturating_add(1))),
                ));
            }
            if matches!(clip.content, ClipContent::Media) && clip.speed_percent != 100 {
                issues.push(issue(
                    QaSeverity::Warning,
                    "retimed_audio_muted",
                    format!(
                        "Clip {} is retimed to {}%; its audio is muted.",
                        clip.id, clip.speed_percent
                    ),
                    Some(track.id),
                    Some(clip.id),
                    Some(clip.timeline_start..end),
                ));
            }
            if let ClipContent::Title(title) = &clip.content
                && let Some(preset) = title.caption_preset
            {
                let maximum = match preset {
                    CaptionPreset::Social => 32,
                    CaptionPreset::Clean | CaptionPreset::Minimal => 42,
                };
                if title.text.chars().count() > maximum {
                    issues.push(issue(
                        QaSeverity::Warning,
                        "caption_line_too_long",
                        format!(
                            "Caption clip {} exceeds the {maximum}-character {:?} preset target.",
                            clip.id, preset
                        ),
                        Some(track.id),
                        Some(clip.id),
                        Some(clip.timeline_start..end),
                    ));
                }
                let nominal_half_second = i64::from(document.fps.numerator())
                    / i64::from(document.fps.denominator().max(1))
                    / 2;
                if duration.0 < nominal_half_second.max(1) {
                    issues.push(issue(
                        QaSeverity::Warning,
                        "caption_too_brief",
                        format!("Caption clip {} may be too brief to read.", clip.id),
                        Some(track.id),
                        Some(clip.id),
                        Some(clip.timeline_start..end),
                    ));
                }
            }
            previous_end = end.max(previous_end);
            previous_was_media = matches!(clip.content, ClipContent::Media);
        }
    }
    if document.duration > TimeCode::ZERO && !has_audio_track {
        issues.push(issue(
            QaSeverity::Info,
            "no_audio_track",
            "The timeline has no populated audio track.",
            None,
            None,
            None,
        ));
    }
    QaReport {
        document_duration: document.duration,
        issues,
    }
}

fn issue(
    severity: QaSeverity,
    code: &str,
    message: impl Into<String>,
    track: Option<TrackId>,
    clip: Option<ClipId>,
    range: Option<std::ops::Range<TimeCode>>,
) -> QaIssue {
    QaIssue {
        severity,
        code: code.to_owned(),
        message: message.into(),
        track,
        clip,
        range,
    }
}

#[cfg(test)]
mod tests {
    use crate::{Clip, MediaAsset, MediaKind, Rational, Title, Track};

    use super::*;

    #[test]
    fn qa_reports_missing_media_gaps_retiming_and_caption_readability() {
        let asset = MediaAsset {
            id: crate::AssetId(1),
            path: "definitely-missing-m31-fixture.mp4".into(),
            name: "fixture".to_owned(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::AudioVideo,
            resolution: Some((1920, 1080)),
        };
        let document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![
                    Clip {
                        id: ClipId(1),
                        asset: asset.id,
                        source_range: TimeCode(0)..TimeCode(30),
                        content: ClipContent::Media,
                        timeline_start: TimeCode(10),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 200,
                    },
                    Clip {
                        id: ClipId(2),
                        asset: crate::AssetId::default(),
                        source_range: TimeCode(0)..TimeCode(4),
                        content: ClipContent::Title(Title {
                            caption_preset: Some(CaptionPreset::Social),
                            text: "A caption line that is intentionally far too long".to_owned(),
                            ..Title::default()
                        }),
                        timeline_start: TimeCode(30),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
                    },
                ],
            }],
            media_pool: vec![asset],
            duration: TimeCode(34),
            ..Document::default()
        };
        let report = qa_document(&document);
        for code in [
            "missing_media",
            "track_gap",
            "retimed_audio_muted",
            "caption_line_too_long",
            "caption_too_brief",
        ] {
            assert!(report.issues.iter().any(|issue| issue.code == code));
        }
        assert!(!report.export_ready());
    }
}

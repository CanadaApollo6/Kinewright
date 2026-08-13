use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ClipContent, Document, Effect, EffectId, OpError, ParamValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryAspect {
    Widescreen,
    Vertical,
    Square,
}

impl DeliveryAspect {
    pub const ALL: [Self; 3] = [Self::Widescreen, Self::Vertical, Self::Square];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Widescreen => "16:9",
            Self::Vertical => "9:16",
            Self::Square => "1:1",
        }
    }

    #[must_use]
    pub const fn resolution(self) -> (u32, u32) {
        match self {
            Self::Widescreen => (1920, 1080),
            Self::Vertical => (1080, 1920),
            Self::Square => (1080, 1080),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryVariant {
    pub aspect: DeliveryAspect,
    /// Reviewable manual/model-selected focal point. This is deliberately not
    /// presented as learned subject tracking.
    pub focus_x_percent: u8,
    pub focus_y_percent: u8,
}

impl DeliveryVariant {
    /// Create a deterministic cover-framed delivery variant.
    ///
    /// # Errors
    ///
    /// Returns an error when either focal coordinate is outside 0..=100.
    #[allow(clippy::similar_names)]
    pub const fn new(
        aspect: DeliveryAspect,
        focus_x_percent: u8,
        focus_y_percent: u8,
    ) -> Result<Self, DeliveryVariantError> {
        if focus_x_percent > 100 || focus_y_percent > 100 {
            return Err(DeliveryVariantError::InvalidFocus {
                x: focus_x_percent,
                y: focus_y_percent,
            });
        }
        Ok(Self {
            aspect,
            focus_x_percent,
            focus_y_percent,
        })
    }

    #[must_use]
    pub const fn centered(aspect: DeliveryAspect) -> Self {
        Self {
            aspect,
            focus_x_percent: 50,
            focus_y_percent: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeliveryVariantError {
    #[error("delivery focal point ({x}, {y}) must stay inside 0..=100 percent")]
    InvalidFocus { x: u8, y: u8 },
    #[error("effect id space is exhausted")]
    EffectIdExhausted,
    #[error(transparent)]
    InvalidDocument(#[from] OpError),
}

/// Materialize a non-destructive delivery document from a master cut.
/// Existing reframe effects are replaced so repeated previews never stack crops.
///
/// # Errors
///
/// Returns an error for invalid source/output documents or exhausted effect ids.
pub fn document_for_delivery_variant(
    document: &Document,
    variant: DeliveryVariant,
) -> Result<Document, DeliveryVariantError> {
    document.validate()?;
    if variant.focus_x_percent > 100 || variant.focus_y_percent > 100 {
        return Err(DeliveryVariantError::InvalidFocus {
            x: variant.focus_x_percent,
            y: variant.focus_y_percent,
        });
    }
    let mut output = document.clone();
    output.resolution = variant.aspect.resolution();
    let mut next_effect_id = output
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| &clip.effects)
        .map(|effect| effect.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(DeliveryVariantError::EffectIdExhausted)?;
    let (width, height) = output.resolution;
    let aspect_basis_points = u64::from(width)
        .saturating_mul(10_000)
        .saturating_add(u64::from(height) / 2)
        / u64::from(height);
    let aspect_basis_points = i64::try_from(aspect_basis_points).unwrap_or(i64::MAX);
    for clip in output.tracks.iter_mut().flat_map(|track| &mut track.clips) {
        if matches!(clip.content, ClipContent::Title(_)) {
            continue;
        }
        clip.effects.retain(|effect| effect.name != "reframe");
        let mut parameters = BTreeMap::new();
        parameters.insert(
            "target_aspect_basis_points".to_owned(),
            ParamValue::Integer(aspect_basis_points),
        );
        parameters.insert(
            "focus_x_percent".to_owned(),
            ParamValue::Integer(i64::from(variant.focus_x_percent)),
        );
        parameters.insert(
            "focus_y_percent".to_owned(),
            ParamValue::Integer(i64::from(variant.focus_y_percent)),
        );
        clip.effects.push(Effect {
            id: EffectId(next_effect_id),
            name: "reframe".to_owned(),
            parameters,
            keyframes: BTreeMap::default(),
        });
        next_effect_id = next_effect_id
            .checked_add(1)
            .ok_or(DeliveryVariantError::EffectIdExhausted)?;
    }
    output.validate()?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use crate::{
        AssetId, Clip, MediaAsset, MediaKind, Rational, TimeCode, Track, TrackId, TrackKind,
    };

    use super::*;

    fn fixture() -> Document {
        let asset = MediaAsset {
            id: AssetId(1),
            path: "variant.mp4".into(),
            name: "variant".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
        };
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: crate::ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(30),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset],
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    #[test]
    fn every_delivery_aspect_materializes_one_valid_non_stacking_reframe() {
        for aspect in DeliveryAspect::ALL {
            let variant = DeliveryVariant::centered(aspect);
            let first = document_for_delivery_variant(&fixture(), variant).unwrap();
            let second = document_for_delivery_variant(&first, variant).unwrap();
            assert_eq!(second.resolution, aspect.resolution());
            let effects = &second.tracks[0].clips[0].effects;
            assert_eq!(
                effects
                    .iter()
                    .filter(|effect| effect.name == "reframe")
                    .count(),
                1
            );
            assert_eq!(
                effects[0].parameters["focus_x_percent"],
                ParamValue::Integer(50)
            );
        }
    }

    #[test]
    fn focal_points_are_bounded() {
        assert_eq!(
            DeliveryVariant::new(DeliveryAspect::Vertical, 101, 50),
            Err(DeliveryVariantError::InvalidFocus { x: 101, y: 50 })
        );
    }
}

//! CPU-side projection of an authored overlay's bounds through the compositor.
use crate::compositor::{LayerParams, params_for_at, pixel_coverage_edge};
use crate::timeline::{title_alpha, transition_render_params};
use kinewright_core::{
    Clip, ClipContent, Document, Effect, EffectUniform as U, MediaError, TimeCode,
    TitlePixelBounds, TransitionShading, effect_descriptor, transition_descriptor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayGeometry {
    Visible(TitlePixelBounds),
    Hidden,
    Unresolved,
}

/// Project the title layout or the still's full quad at an active project frame.
/// Pixel-dependent or unsupported geometry is marked unresolved.
///
/// # Errors
/// Returns an error if the clip's project duration cannot be represented.
pub fn review_overlay_geometry(
    document: &Document,
    clip: &Clip,
    at: TimeCode,
    bounds: TitlePixelBounds,
    raster: (u32, u32),
) -> Result<OverlayGeometry, MediaError> {
    let mut transition = transition_render_params(clip, at);
    if let ClipContent::Title(title) = &clip.content {
        transition.alpha *= title_alpha(document, clip, title, at)?;
    }
    let local = TimeCode(at.0 - clip.timeline_start.0);
    if clip.effects.iter().any(|effect| {
        effect.is_enabled_at(local) && effect_parameters_reshape(effect, Some(local), false)
    }) {
        return Ok(OverlayGeometry::Unresolved);
    }
    Ok(project(
        bounds,
        raster,
        params_for_at(&clip.effects, transition, Some(local)),
    ))
}

/// Whether a layer can reshape the already-composited picture during its range.
#[must_use]
pub fn review_layer_can_reshape_below(clip: &Clip) -> bool {
    reshaping_transition_frames(clip).is_some()
        || (matches!(clip.content, ClipContent::Adjustment)
            && clip.effects.iter().any(|effect| {
                (effect.enabled || effect.enabled_curve.is_some())
                    && effect_parameters_reshape(effect, None, true)
            }))
}

/// Whether this layer can reshape the backdrop at one project frame.
#[must_use]
pub fn review_layer_reshapes_below_at(clip: &Clip, at: TimeCode) -> bool {
    let local = TimeCode(at.0 - clip.timeline_start.0);
    clip.is_enabled_at(local)
        && (reshaping_transition_frames(clip).is_some_and(|end| (0..end).contains(&local.0))
            || (matches!(clip.content, ClipContent::Adjustment)
                && clip.effects.iter().any(|effect| {
                    effect.is_enabled_at(local)
                        && effect_parameters_reshape(effect, Some(local), true)
                })))
}

fn reshaping_transition_frames(clip: &Clip) -> Option<i64> {
    let transition = clip.transition_in.as_ref().filter(|t| t.duration.0 > 1)?;
    let reshapes = transition_descriptor(&transition.name).is_none_or(|desc| {
        match desc.shading {
            TransitionShading::Push { .. } => true,
            TransitionShading::CrossfadeAlpha | TransitionShading::FadeFromColor { .. } => false,
            // Adjustment layers sample the stack below them. A spatial
            // transition can move or mask those pixels as well.
            _ => matches!(clip.content, ClipContent::Adjustment),
        }
    });
    // The renderer reaches the authored, undisplaced state at d - 1.
    reshapes.then_some(transition.duration.0 - 1)
}

fn effect_parameters_reshape(
    effect: &Effect,
    local: Option<TimeCode>,
    include_transforms: bool,
) -> bool {
    effect_descriptor(&effect.name).is_none_or(|desc| {
        desc.parameters.iter().any(|parameter| {
            match parameter.uniform {
                U::Brightness
                | U::Contrast
                | U::Saturation
                | U::Opacity
                | U::PrimaryExposure
                | U::PrimaryTemperature
                | U::PrimaryTint
                | U::PrimaryContrast
                | U::PrimaryPivot
                | U::Blacks
                | U::Shadows
                | U::Highlights
                | U::Whites
                | U::PrimarySaturation
                | U::LutPreset
                | U::LutIntensity
                | U::ExternalLutIntensity
                | U::ColorNode
                | U::AnchorX
                | U::AnchorY => false,
                U::Scale
                | U::ScaleX
                | U::ScaleY
                | U::ScaleFine
                | U::OffsetX
                | U::OffsetY
                | U::OffsetXBasisPoints
                | U::OffsetYBasisPoints
                | U::Rotation => {
                    include_transforms
                        && match local {
                            Some(at) => effect
                                .integer_parameter_at(parameter.name, at)
                                .is_some_and(|value| value != parameter.neutral),
                            None => {
                                effect
                                    .static_integer_parameter(parameter.name)
                                    .is_some_and(|value| value != parameter.neutral)
                                    || effect.keyframes.get(parameter.name).is_some_and(|curve| {
                                        curve
                                            .keyframes
                                            .iter()
                                            .any(|key| key.value != parameter.neutral)
                                    })
                            }
                        }
                }
                // Crops, reframes, masks, keys and any unclassified uniform
                // require a visibility proof beyond a transformed rectangle.
                _ => true,
            }
        })
    })
}

fn project(
    bounds: TitlePixelBounds,
    raster: (u32, u32),
    mut params: LayerParams,
) -> OverlayGeometry {
    if params.opacity <= 0.0 || params.scale_x == 0.0 || params.scale_y == 0.0 {
        return OverlayGeometry::Hidden;
    }
    let width = f64::from(raster.0);
    let height = f64::from(raster.1);
    let anchor_x = f64::from(params.anchor_x) * width;
    let anchor_y = f64::from(params.anchor_y) * height;
    let offset_x = f64::from(params.offset_x) * width * 0.5;
    let offset_y = f64::from(params.offset_y) * height * 0.5;
    let (sin, cos) = f64::from(params.rotation).sin_cos();
    let mut left = f64::INFINITY;
    let mut right = f64::NEG_INFINITY;
    let mut top = f64::INFINITY;
    let mut bottom = f64::NEG_INFINITY;
    for x in [bounds.left, bounds.right] {
        for y in [bounds.top, bounds.bottom] {
            let dx = (f64::from(x) - anchor_x) * f64::from(params.scale_x);
            let dy = (f64::from(y) - anchor_y) * f64::from(params.scale_y);
            let x = anchor_x + dx * cos - dy * sin + offset_x;
            let y = anchor_y + dx * sin + dy * cos + offset_y;
            left = left.min(x);
            right = right.max(x);
            top = top.min(y);
            bottom = bottom.max(y);
        }
    }
    if ![left, top, right, bottom].into_iter().all(f64::is_finite) {
        return OverlayGeometry::Unresolved;
    }
    if params.coverage_on > 0.5 {
        pixel_coverage_edge(&mut params, raster.0, raster.1);
        let (low, high) = if params.coverage_axis > 0.5 {
            (&mut top, &mut bottom)
        } else {
            (&mut left, &mut right)
        };
        let column = f64::from(params.coverage_edge) - 0.5;
        if params.coverage_on < 1.5 {
            *high = high.min(column);
        } else {
            *low = low.max(column);
        }
    }
    if left.max(0.0) >= right.min(width) || top.max(0.0) >= bottom.min(height) {
        return OverlayGeometry::Hidden;
    }
    // The compositor rounds to f32 and samples with filtering on transformed
    // quads. Static identity bounds already include layout's pixel padding.
    #[allow(
        clippy::float_cmp,
        reason = "only the exact identity uses unexpanded bounds"
    )]
    let transformed = params.scale_x != 1.0
        || params.scale_y != 1.0
        || params.rotation != 0.0
        || offset_x != 0.0
        || offset_y != 0.0;
    let margin = f64::from(u8::from(transformed));
    #[allow(clippy::cast_possible_truncation)]
    let pixels = TitlePixelBounds {
        left: (left - margin).floor().max(0.0) as i32,
        top: (top - margin).floor().max(0.0) as i32,
        right: (right + margin).ceil().min(width) as i32,
        bottom: (bottom + margin).ceil().min(height) as i32,
    };
    OverlayGeometry::Visible(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::twin::{rasterized, transition_covers};

    fn rasterized_bounds(raster: (u16, u16), params: &LayerParams) -> Option<TitlePixelBounds> {
        let size = [f32::from(raster.0), f32::from(raster.1)];
        let mut result: Option<TitlePixelBounds> = None;
        for y in 0..raster.1 {
            for x in 0..raster.0 {
                let pixel = [usize::from(x), usize::from(y)];
                if !rasterized(params, size, pixel) || !transition_covers(params, size, pixel) {
                    continue;
                }
                let (x, y) = (i32::from(x), i32::from(y));
                let bounds = result.get_or_insert(TitlePixelBounds {
                    left: x,
                    top: y,
                    right: x + 1,
                    bottom: y + 1,
                });
                bounds.left = bounds.left.min(x);
                bounds.top = bounds.top.min(y);
                bounds.right = bounds.right.max(x + 1);
                bounds.bottom = bounds.bottom.max(y + 1);
            }
        }
        result
    }

    #[test]
    fn output_review_fix2_geometry_matches_compositor_cpu_twin() {
        for raster in [(33_u16, 21_u16), (19, 35)] {
            let full = TitlePixelBounds {
                left: 0,
                top: 0,
                right: i32::from(raster.0),
                bottom: i32::from(raster.1),
            };
            // Keep lightly clipped quads so the same tight bound holds
            // whether the rasterizer covers a rotated or axis-aligned edge.
            for [anchor_x, anchor_y] in [[0.5, 0.5], [0.38, 0.60]] {
                for [scale_x, scale_y] in [[0.75, 0.75], [-0.58, 0.77]] {
                    for rotation in [0.0, 0.41, -0.41] {
                        for [offset_x, offset_y] in [[0.0, 0.0], [0.10, -0.11]] {
                            for [on, axis, edge] in
                                [[0.0, 0.0, 0.0], [1.0, 0.0, 0.68], [2.0, 1.0, 0.27]]
                            {
                                let mut params = params_for_at(
                                    &[],
                                    crate::TransitionRenderParams::default(),
                                    None,
                                );
                                params.anchor_x = anchor_x;
                                params.anchor_y = anchor_y;
                                params.scale_x = scale_x;
                                params.scale_y = scale_y;
                                params.rotation = rotation;
                                params.offset_x = offset_x;
                                params.offset_y = offset_y;
                                params.frame_aspect = f32::from(raster.1) / f32::from(raster.0);
                                params.coverage_on = on;
                                params.coverage_axis = axis;
                                params.coverage_edge = edge;
                                let actual =
                                    rasterized_bounds(raster, &params).expect("visible test quad");
                                let OverlayGeometry::Visible(expected) =
                                    project(full, (raster.0.into(), raster.1.into()), params)
                                else {
                                    panic!("a rasterized quad must have expected bounds");
                                };
                                assert!(
                                    expected.contains(actual),
                                    "{params:?} {actual:?} outside {expected:?}"
                                );
                                let excess = [
                                    actual.left - expected.left,
                                    actual.top - expected.top,
                                    expected.right - actual.right,
                                    expected.bottom - actual.bottom,
                                ];
                                assert!(
                                    excess.into_iter().all(|edge| edge <= 2),
                                    "{params:?} {actual:?} vs {expected:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn output_review_fix1_unknown_upper_effect_is_conservative() {
        let effect = Effect {
            id: kinewright_core::EffectId(1),
            name: "future_geometry".into(),
            parameters: std::collections::BTreeMap::new(),
            keyframes: std::collections::BTreeMap::new(),
            enabled: true,
            enabled_curve: None,
        };
        assert!(effect_parameters_reshape(&effect, None, true));
    }

    #[test]
    fn output_review_fix2_unknown_upper_transition_is_unresolved_while_active() {
        let clip = Clip {
            id: kinewright_core::ClipId(1),
            asset: kinewright_core::AssetId(0),
            content: ClipContent::Adjustment,
            timeline_start: TimeCode(10),
            source_range: TimeCode(0)..TimeCode(30),
            transition_in: Some(kinewright_core::Transition {
                name: "future_spatial_transition".into(),
                duration: TimeCode(21),
            }),
            effects: Vec::new(),
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            audio_gain_curve: None,
            speed_percent: 100,
            enabled: true,
            enabled_curve: None,
            blend_mode: kinewright_core::BlendMode::Normal,
        };
        assert!(review_layer_can_reshape_below(&clip));
        assert!(review_layer_reshapes_below_at(&clip, TimeCode(10)));
        assert!(review_layer_reshapes_below_at(&clip, TimeCode(29)));
        assert!(!review_layer_reshapes_below_at(&clip, TimeCode(30)));
    }
}

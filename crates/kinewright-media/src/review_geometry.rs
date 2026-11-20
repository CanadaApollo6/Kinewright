//! CPU-side projection of an authored overlay's bounds through the compositor.
use crate::compositor::params_for;
use crate::timeline::{
    TransitionRenderParams, evaluated_effects, title_alpha, transition_render_params,
};
use kinewright_core::{
    Clip, ClipContent, Document, MediaError, TimeCode, TitlePixelBounds, TransitionAxis,
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
    let effects = evaluated_effects(clip, at);
    Ok(project(bounds, raster, &effects, transition))
}

fn project(
    bounds: TitlePixelBounds,
    raster: (u32, u32),
    effects: &[kinewright_core::Effect],
    transition: TransitionRenderParams,
) -> OverlayGeometry {
    let params = params_for(effects, transition);
    if params.opacity <= 0.0 || params.scale_x == 0.0 || params.scale_y == 0.0 {
        return OverlayGeometry::Hidden;
    }
    if effects.iter().any(|effect| {
        matches!(
            effect.name.as_str(),
            "crop" | "reframe" | "mask" | "chroma_key"
        )
    }) {
        return OverlayGeometry::Unresolved;
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
    if let Some(coverage) = transition.coverage {
        let (low, high, extent) = match coverage.axis {
            TransitionAxis::Horizontal => (&mut left, &mut right, width),
            TransitionAxis::Vertical => (&mut top, &mut bottom, height),
        };
        let column = (f64::from(coverage.edge) * extent - 0.5).ceil();
        if coverage.below_edge {
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

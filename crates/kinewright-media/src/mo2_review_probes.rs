//! MO2 B1 review probes (review-mo2-b1-1 `review1_*`, review-mo2-b1-2
//! `review2_*`), retained as committed regressions (B1 fix round 1).
//!
//! The review-2 oracle is design-derived: f64 equations, straight alpha over
//! opaque black, independent inverse-coordinate bilinear sampling and pixel
//! centre coverage. It never consults the twin's blend/sampling, the shader
//! or `params_for`; production `visual_layers_at` only drives the transition
//! under test. Review-1's §6 oracle is likewise explicit geometry.

#![allow(clippy::many_single_char_names)]

use super::*;

fn full() -> (RenderScale, DecodeStrategy) {
    (RenderScale::FullResolution, DecodeStrategy::Seek)
}

// ---------------------------------------------------------------- R10

/// Review-1 B1 / review-2 B2: a valid `Normal` adjustment whose four +5-stop
/// corrections leave the f16 range refuses typed on every path (including an
/// actual export), also when an opaque layer covers it afterwards.
fn review1_r10_normal_adjustment_storage_must_refuse_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context.clone());
    let boosts = (1..=4)
        .map(|id| primary(id, &[("exposure_milli_stops", 5_000)]))
        .collect();
    let white = solid(1, [255; 3], BlendMode::Normal, vec![]);
    let mut doc = document(vec![white, adjustment(2, BlendMode::Normal, boosts)]);
    let expected = Some(MediaError::NonFiniteRender {
        layer: 1,
        clip: Some(ClipId(2)),
        at: Some(TimeCode(3)),
    });
    let (scale, seek) = full();
    for covered in [false, true] {
        if covered {
            let cover = solid(3, GREY, BlendMode::Normal, vec![]);
            doc.tracks.push(Track {
                id: TrackId(3),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![cover],
            });
        }
        let (at, size) = (TimeCode(3), doc.resolution);
        assert_eq!(gpu(&mut r, &doc, 3).err(), expected, "covered={covered}");
        assert_eq!(r.twin_working(&doc, at, size).err(), expected);
        assert_eq!(r.render(&doc, at, size, scale, seek).err(), expected);
        let delivery = r.render_delivery(&doc, at, size, scale, seek);
        assert_eq!(delivery.err(), expected, "delivery covered={covered}");
    }
    let directory = crate::test_support::TempDirectory::new("mo2-b1-nonfinite-export");
    let output = directory.path("refused.mp4");
    let settings = kinewright_core::ExportSettings {
        fps: doc.fps,
        resolution: doc.resolution,
        delivery_color: doc.color_context.delivery.clone(),
        video_codec: "libx264".into(),
        audio_codec: "aac".into(),
        video_bitrate: 1_000_000,
        audio_bitrate: 192_000,
        loudness_normalization: None,
        cancellation: kinewright_core::ExportCancellation::default(),
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    let export = crate::export::export_document(&doc, &output, &settings, &tx, context);
    assert!(
        export.is_err(),
        "an export never encodes saturation: {export:?}"
    );
}

/// Review-1 B1 / review-2 B2: NaN and ±inf operands refuse before
/// `min`/`max` can erase them, on both lanes.
fn review1_r10_nan_extrema_must_refuse_on(context: GpuContext) {
    let c = Compositor::new(context);
    for mode in [BlendMode::Darken, BlendMode::Lighten] {
        for (s, d) in [
            (f32::NAN, 0.5),
            (0.5, f32::NAN),
            (f32::INFINITY, 0.5),
            (f32::NEG_INFINITY, 0.5),
        ] {
            let (gpu, cpu) = pair_lanes(&c, mode, grey4(d), grey4(s));
            let label = format!("{mode:?}({s}, {d})");
            refused(gpu, 1, &label);
            refused(cpu, 1, &label);
        }
    }
}

/// Review-1 B1: a valid Darken solid whose extreme wheels overflow f32 to
/// +inf before the blend refuses on every path.
fn review1_r10_valid_darken_overflow_must_refuse_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let extreme = effect(
        2,
        "color_wheels",
        &[
            ("gain_master_thousandths", 4_000),
            ("gain_red_thousandths", 4_000),
            ("gamma_master_thousandths", 4_000),
            ("gamma_red_thousandths", 4_000),
        ],
    );
    let hot = vec![primary(1, &[("exposure_milli_stops", 2_000)]), extreme];
    let top = solid(2, [255; 3], BlendMode::Darken, hot);
    let doc = document(vec![solid(1, GREY, BlendMode::Normal, vec![]), top]);
    let (at, size, (scale, seek)) = (TimeCode(0), doc.resolution, full());
    refused(gpu(&mut r, &doc, 0), 1, "GPU");
    refused(r.twin_working(&doc, at, size), 1, "twin");
    let monitor = r.render(&doc, at, size, scale, seek);
    assert!(matches!(monitor, Err(MediaError::NonFiniteRender { .. })));
    let delivery = r.render_delivery(&doc, at, size, scale, seek);
    assert!(matches!(delivery, Err(MediaError::NonFiniteRender { .. })));
}

/// Review-2 B2: the same through `pair_lanes`, plus a `Normal` pixel layer
/// carrying +inf into a `Normal` adjustment, covered or not.
fn review2_nonfinite_inputs_and_normal_adjustment_on(context: GpuContext) {
    let compositor = Compositor::new(context.clone());
    let mut accepted = Vec::new();
    for (mode, s) in [
        (BlendMode::Darken, f32::INFINITY),
        (BlendMode::Lighten, f32::NEG_INFINITY),
        (BlendMode::Darken, f32::NAN),
    ] {
        let (a, b) = pair_lanes(&compositor, mode, grey4(0.5), grey4(s));
        if a.is_ok() || b.is_ok() {
            accepted.push(format!("{mode:?}({s})"));
        }
    }
    let mut r = FrameRenderer::new(context);
    let boost = (1..=4)
        .map(|id| primary(id, &[("exposure_milli_stops", 5_000)]))
        .collect();
    let hot = adjustment(2, BlendMode::Normal, boost);
    let mut doc = document_sized(
        (3, 3),
        vec![solid(1, [255; 3], BlendMode::Normal, vec![]), hot],
    );
    let (scale, seek) = full();
    for covered in [false, true] {
        if covered {
            doc.tracks.push(Track {
                id: TrackId(3),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![solid(3, GREY, BlendMode::Normal, vec![])],
            });
        }
        let (at, size) = (TimeCode(3), doc.resolution);
        let results = [
            gpu(&mut r, &doc, 3).is_ok(),
            r.twin_working(&doc, at, size).is_ok(),
            r.render(&doc, at, size, scale, seek).is_ok(),
            r.render_delivery(&doc, at, size, scale, seek).is_ok(),
        ];
        if results.iter().any(|ok| *ok) {
            accepted.push(format!("Normal adjustment covered={covered}: {results:?}"));
        }
    }
    assert!(accepted.is_empty(), "R10 accepted {accepted:?}");
}

/// Review-2 S1: `Add(40000, 40000)` at α = 0.25 stores a finite 50,000
/// (49,984 in f16); the unstored intermediate B = 80,000 is not a boundary.
fn review2_representable_result_must_not_refuse_on(context: GpuContext) {
    let compositor = Compositor::new(context);
    let (d, s) = (grey4(40_000.0), [40_000.0, 40_000.0, 40_000.0, 0.25]);
    let (a, b) = pair_lanes(&compositor, BlendMode::Add, d, s);
    let q = store(50_000.0);
    assert_eq!(q, 49_984.0);
    assert_eq!(a.unwrap().pixels, [q, q, q, 1.0], "GPU");
    assert_eq!(b.unwrap().pixels, [q, q, q, 1.0], "twin");
}

gpu_lanes! {
    review1_r10_normal_adjustment_storage_must_refuse => review1_r10_normal_adjustment_storage_must_refuse_on,
    review1_r10_nan_extrema_must_refuse => review1_r10_nan_extrema_must_refuse_on,
    review1_r10_valid_darken_overflow_must_refuse => review1_r10_valid_darken_overflow_must_refuse_on,
    review2_nonfinite_inputs_and_normal_adjustment => review2_nonfinite_inputs_and_normal_adjustment_on,
    review2_representable_result_must_not_refuse => review2_representable_result_must_not_refuse_on,
}

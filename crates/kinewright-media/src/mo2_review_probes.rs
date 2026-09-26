//! MO2 B1 review probes (review-mo2-b1-1 `review1_*`, review-mo2-b1-2
//! `review2_*`), retained as committed regressions (B1 fix round 1).
//!
//! The review-2 oracle is design-derived: f64 equations, straight alpha over
//! opaque black, independent inverse-coordinate bilinear sampling and pixel
//! centre coverage. It never consults the twin's blend/sampling, the shader
//! or `params_for`; production `visual_layers_at` only drives the transition
//! under test. Review-1's §6 oracle is likewise explicit geometry.

#![allow(clippy::many_single_char_names, clippy::cast_possible_wrap)]

use super::*;
use crate::timeline::{TimelineVisualLayer, visual_layers_at};
use kinewright_core::{AutomationCurve, Keyframe, KeyframeInterpolation};

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

// ---------------------------------------------------------------- oracle

fn q(x: f64) -> f64 {
    f16::from_f64(x).to_f64()
}

fn reference_blend(mode: BlendMode, s: f64, d: f64) -> f64 {
    match mode {
        BlendMode::Normal => s,
        BlendMode::Multiply => s * d,
        BlendMode::Darken => s.min(d),
        BlendMode::Lighten => s.max(d),
        BlendMode::Add => s + d,
        BlendMode::Screen | BlendMode::Overlay => {
            let (a, b) = (s.clamp(0.0, 1.0), d.clamp(0.0, 1.0));
            let unit = if mode == BlendMode::Screen {
                a + b - a * b
            } else if b <= 0.5 {
                2.0 * a * b
            } else {
                2.0 * a + 2.0 * b - 2.0 * a * b - 1.0
            };
            unit + s - a + d - b
        }
    }
}

fn over(mode: BlendMode, s: [f64; 4], d: [f64; 4]) -> [f64; 4] {
    let mut out = [1.0; 4];
    for c in 0..3 {
        out[c] = q(s[3] * reference_blend(mode, s[c], d[c]) + (1.0 - s[3]) * d[c]);
    }
    out
}

fn frame(size: (u32, u32), pixels: &[[f64; 4]]) -> WorkingFrame {
    let halves = pixels.iter().flatten().map(|v| f16::from_f64(*v));
    WorkingFrame {
        width: size.0,
        height: size.1,
        pixels: Arc::new(halves.collect()),
    }
}

#[derive(Default)]
struct Stats {
    errors: Vec<f64>,
    bad: usize,
    alpha_bad: usize,
    monitor: Vec<f64>,
}

fn triple(v: &mut [f64]) -> (f64, f64, f64) {
    v.sort_by(f64::total_cmp);
    let p99 = v[((v.len() - 1) as f64 * 0.99).ceil() as usize];
    (v[v.len() - 1], p99, v.iter().sum::<f64>() / v.len() as f64)
}

impl Stats {
    /// R27 working tolerances against the oracle.
    fn add(&mut self, actual: &LinearRgbaImage, expected: &[[f64; 4]], label: &str) {
        for (i, (a, e)) in actual
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected)
            .enumerate()
        {
            self.alpha_bad += usize::from(a[3] != 1.0);
            for c in 0..3 {
                let err = (f64::from(a[c]) - e[c]).abs();
                self.errors.push(err);
                let bad = if e[c].abs() > 1.0 || a[c].abs() > 1.0 {
                    let rel = err / e[c].abs().max(f64::MIN_POSITIVE);
                    rel > 2_f64.powi(-10) || f16_key(a[c]).abs_diff(f16_key(e[c] as f32)) > 4
                } else {
                    err > 1e-3
                };
                if bad && self.bad < 3 {
                    println!("{label}: pixel {i} channel {c}: {} vs {}", a[c], e[c]);
                }
                self.bad += usize::from(bad);
            }
        }
    }

    fn monitor(&mut self, actual: &[u8], expected: &[[f64; 4]]) {
        let monitoring = Document::default().color_context.monitoring;
        for (a, e) in actual.as_chunks::<4>().0.iter().zip(expected) {
            let b = encode_monitor_rgba8_for_description(e.map(|v| v as f32), &monitoring);
            let b = b.unwrap();
            for c in 0..3 {
                self.monitor.push(f64::from(a[c].abs_diff(b[c])));
            }
        }
    }

    fn finish(&mut self, label: &str) -> bool {
        let (max, p99, mean) = triple(&mut self.errors);
        let (mm, mp, ma) = triple(&mut self.monitor);
        println!(
            "{label}: n={} max={max:.9} p99={p99:.9} mean={mean:.9} bad={} alpha_bad={} \
             monitor={mm}/{mp}/{ma:.6}",
            self.errors.len(),
            self.bad,
            self.alpha_bad
        );
        self.bad == 0 && self.alpha_bad == 0 && mm <= 2.0 && mp <= 1.0 && ma <= 0.25
    }
}

fn twin_monitor(actual: &LinearRgbaImage) -> Vec<u8> {
    let m = Document::default().color_context.monitoring;
    let pixels = actual.pixels.as_chunks::<4>().0.iter();
    pixels
        .flat_map(|p| encode_monitor_rgba8_for_description(*p, &m).unwrap())
        .collect()
}

/// Review-2: all modes over 4,257 pixels (every §3 vector and CC8 peak, a
/// 17×17 level grid × five alphas, seeded random unit/signed over-range).
fn review2_independent_blend_grid_on(context: GpuContext) {
    let compositor = Compositor::new(context);
    let size = (129, 33);
    let n = (size.0 * size.1) as usize;
    let levels = [
        -2.0, -1.0, -0.001, 0.0, 0.18, 0.4, 0.5, 0.6, 0.75, 0.999, 1.0, 1.001, 2.0, 3.0, 3.776_475,
        4.0, 46.4159,
    ];
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1_u64 << 53) as f64
    };
    let (mut ds, mut ss) = (Vec::new(), Vec::new());
    for i in 0..n {
        let (mut d, mut s) = ([1.0; 4], [1.0; 4]);
        if i < levels.len() * levels.len() * 5 {
            d[..3].fill(levels[(i / 5) % levels.len()]);
            s[..3].fill(levels[i / (5 * levels.len())]);
            s[3] = [0.0, 0.125, 0.5, 0.875, 1.0][i % 5];
        } else {
            for c in 0..3 {
                d[c] = rand();
                s[c] = rand();
                if i % 2 == 0 {
                    d[c] = 7.0 * d[c] - 2.0;
                    s[c] = 7.0 * s[c] - 2.0;
                }
            }
            s[3] = rand();
        }
        ds.push(d.map(q));
        ss.push(s.map(q));
    }
    let (d, s) = (frame(size, &ds), frame(size, &ss));
    let mut good = true;
    for mode in BlendMode::ALL {
        let expected: Vec<_> = ss
            .iter()
            .zip(&ds)
            .map(|(&s, &d)| over(mode, s, d))
            .collect();
        let layers = [pixels_layer(&d, BlendMode::Normal), pixels_layer(&s, mode)];
        let gpu = compositor.render_working(size, &layers).unwrap();
        let cpu = twin::render_working(size, &layers, None).unwrap();
        let monitor = compositor.render(size, &layers).unwrap().rgba.to_vec();
        let cpu_monitor = twin_monitor(&cpu);
        for (label, image, bytes) in [("gpu", gpu, monitor), ("twin", cpu, cpu_monitor)] {
            let label = format!("blend/{mode:?}/{label}");
            let mut stats = Stats::default();
            stats.add(&image, &expected, &label);
            stats.monitor(&bytes, &expected);
            good &= stats.finish(&label);
        }
    }
    assert!(good, "R27 failures; all per-mode statistics printed");
}

/// Bilinear interpolation in source-pixel coordinates, edges clamped.
fn sample(pixels: &[[f64; 4]], size: (u32, u32), u: f64, v: f64) -> [f64; 4] {
    let (x, y) = (u * f64::from(size.0) - 0.5, v * f64::from(size.1) - 0.5);
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let mut out = [0.0; 4];
    for j in 0..2 {
        for i in 0..2 {
            let weight = if i == 0 { 1.0 - fx } else { fx } * if j == 0 { 1.0 - fy } else { fy };
            let xx = (ix + i).clamp(0, size.0 as i32 - 1) as usize;
            let yy = (iy + j).clamp(0, size.1 as i32 - 1) as usize;
            for c in 0..4 {
                out[c] += weight * pixels[yy * size.0 as usize + xx][c];
            }
        }
    }
    out
}

struct Case<'a> {
    size: (u32, u32),
    below: &'a [[f64; 4]],
    above: &'a [[f64; 4]],
    kind: &'a str,
    dir: &'a str,
    offset: i64,
    duration: i64,
    mode: BlendMode,
    transformed: bool,
}

/// §6 by hand: pixel-centre coverage, entering/backdrop offsets, Push
/// backdrop falling back to unshifted `D0` outside the half-open raster.
fn reference_transition(c: &Case<'_>) -> Vec<[f64; 4]> {
    let active = c.duration > 1 && c.offset < c.duration - 1;
    let p = if active {
        c.offset as f64 / (c.duration - 1) as f64
    } else {
        1.0
    };
    let inside = |u: f64, v: f64| (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v);
    let mut result = Vec::new();
    for y in 0..c.size.1 {
        for x in 0..c.size.0 {
            let fx = (f64::from(x) + 0.5) / f64::from(c.size.0);
            let fy = (f64::from(y) + 0.5) / f64::from(c.size.1);
            let mut d = c.below[(y * c.size.0 + x) as usize];
            let (ex, ey, qx, qy, covered) = match c.dir {
                "left" => (p - 1.0, 0.0, p, 0.0, fx < p),
                "right" => (1.0 - p, 0.0, -p, 0.0, fx >= 1.0 - p),
                "up" => (0.0, p - 1.0, 0.0, p, fy < p),
                _ => (0.0, 1.0 - p, 0.0, -p, fy >= 1.0 - p),
            };
            if active && c.kind == "push" && inside(fx - qx, fy - qy) {
                d = sample(c.below, c.size, fx - qx, fy - qy).map(q);
            }
            let (dx, dy) = if active && c.kind != "wipe" {
                (ex, ey)
            } else {
                (0.0, 0.0)
            };
            let (scale, tx, ty) = if c.transformed {
                (1.5, 0.1, -0.05)
            } else {
                (1.0, 0.0, 0.0)
            };
            let (u, v) = (
                (fx - dx - tx - 0.5) / scale + 0.5,
                (fy - dy - ty - 0.5) / scale + 0.5,
            );
            if (!active || covered) && inside(u, v) {
                let mut s = sample(c.above, c.size, u, v);
                s[3] *= 0.6;
                d = over(c.mode, s, d);
            }
            result.push(d);
        }
    }
    result
}

/// The layers `visual_layers_at` resolves for a 60% solid entering with
/// `transition` over `d`, with the probe's `s` swapped in as its pixels.
fn transition_layers<'a>(
    doc: &Document,
    at: i64,
    below: &'a WorkingFrame,
    above: &'a WorkingFrame,
    effects: &'a [Effect],
    mode: BlendMode,
) -> [CompositorLayer<'a, WorkingFrame>; 2] {
    let resolved = visual_layers_at(doc, TimeCode(at)).unwrap();
    let TimelineVisualLayer::Solid(layer) = &resolved[0] else {
        unreachable!()
    };
    let entering = CompositorLayer {
        frame: above,
        effects,
        transition: layer.transition,
        mode: LayerMode {
            blend: mode,
            role: LayerRole::Pixels,
        },
    };
    [pixels_layer(below, BlendMode::Normal), entering]
}

/// Review-2 B1: all twelve directions on odd and even rasters (17×11,
/// 31×19, 32×18), durations 1/9/10 at first/mid/penultimate/final offsets,
/// Normal/Screen, plain and transformed translucent sources.
fn review2_independent_transition_grid_on(context: GpuContext) {
    let compositor = Compositor::new(context);
    let mut good = true;
    for kind in ["push", "slide", "wipe"] {
        for dir in ["left", "right", "up", "down"] {
            let (mut gpu_stats, mut cpu_stats) = (Stats::default(), Stats::default());
            for size in [(17, 11), (31, 19), (32, 18)] {
                let (mut ds, mut ss) = (Vec::new(), Vec::new());
                for y in 0..size.1 {
                    for x in 0..size.0 {
                        let u = f64::from(x) / f64::from(size.0);
                        let v = f64::from(y) / f64::from(size.1);
                        ds.push([q(0.1 + 0.7 * u), q(0.1 + 0.7 * v), q(0.3), 1.0]);
                        ss.push([
                            q(0.8 - 0.5 * v),
                            q(0.3 + 0.4 * u),
                            q(0.6),
                            q(0.1 + 0.8 * u * v),
                        ]);
                    }
                }
                let (d, s) = (frame(size, &ds), frame(size, &ss));
                for duration in [1, 9, 10] {
                    let offsets = if duration == 1 {
                        vec![0, 1]
                    } else {
                        vec![0, 1, (duration - 1) / 2, duration - 2, duration - 1]
                    };
                    for offset in offsets {
                        for transformed in [false, true] {
                            for mode in [BlendMode::Normal, BlendMode::Screen] {
                                let mut effects = vec![opacity(1, 60)];
                                if transformed {
                                    let moved = [
                                        ("scale_percent", 150),
                                        ("x_percent", 10),
                                        ("y_percent", -5),
                                    ];
                                    effects.push(effect(2, "transform", &moved));
                                }
                                let name = format!("{kind}_{dir}");
                                let entering = solid(1, GREY, mode, vec![]);
                                let entering = with_transition(entering, &name, duration.min(9));
                                let mut doc = document_sized(size, vec![entering]);
                                if duration == 10 {
                                    let clip = &mut doc.tracks[0].clips[0];
                                    clip.source_range.end = TimeCode(11);
                                    clip.transition_in.as_mut().unwrap().duration = TimeCode(10);
                                    doc.duration = TimeCode(11);
                                }
                                let layers =
                                    transition_layers(&doc, offset, &d, &s, &effects, mode);
                                let expected = reference_transition(&Case {
                                    size,
                                    below: &ds,
                                    above: &ss,
                                    kind,
                                    dir,
                                    offset,
                                    duration,
                                    mode,
                                    transformed,
                                });
                                let label = format!(
                                    "{name} {size:?} d={duration} at={offset} \
                                     transformed={transformed} {mode:?}"
                                );
                                let actual = compositor.render_working(size, &layers).unwrap();
                                let cpu = twin::render_working(size, &layers, None).unwrap();
                                gpu_stats.add(&actual, &expected, &format!("gpu {label}"));
                                cpu_stats.add(&cpu, &expected, &format!("twin {label}"));
                                let monitor = compositor.render(size, &layers).unwrap();
                                gpu_stats.monitor(&monitor.rgba, &expected);
                                cpu_stats.monitor(&twin_monitor(&cpu), &expected);
                            }
                        }
                    }
                }
            }
            good &= gpu_stats.finish(&format!("transition/{kind}_{dir}/gpu"));
            good &= cpu_stats.finish(&format!("transition/{kind}_{dir}/twin"));
        }
    }
    assert!(good, "R27 failures; per-direction statistics printed");
}

/// Review-2 S2 (kills M22/M25): at 4×4, d = 9, offset 3 (p = 3/8), the
/// pixel centre x = 3/8 lies exactly on the reveal edge and left coverage
/// must exclude it — on each lane, against the oracle.
fn review2_exact_pixel_centre_coverage_on(context: GpuContext) {
    let compositor = Compositor::new(context);
    let size = (4, 4);
    let (ds, ss) = (
        vec![[0.25, 0.25, 0.25, 1.0]; 16],
        vec![[0.75, 0.75, 0.75, 1.0]; 16],
    );
    let (d, s) = (frame(size, &ds), frame(size, &ss));
    let effects = vec![opacity(1, 60)];
    let entering = with_transition(solid(1, GREY, BlendMode::Normal, vec![]), "wipe_left", 9);
    let doc = document_sized(size, vec![entering]);
    let layers = transition_layers(&doc, 3, &d, &s, &effects, BlendMode::Normal);
    let expected = reference_transition(&Case {
        size,
        below: &ds,
        above: &ss,
        kind: "wipe",
        dir: "left",
        offset: 3,
        duration: 9,
        mode: BlendMode::Normal,
        transformed: false,
    });
    assert!(
        expected[0][0] > 0.25 && expected[1][0] == 0.25,
        "x = 3/8 is excluded"
    );
    let gpu = compositor.render_working(size, &layers).unwrap();
    let cpu = twin::render_working(size, &layers, None).unwrap();
    for (label, image) in [("gpu", gpu), ("twin", cpu)] {
        let mut stats = Stats::default();
        stats.add(&image, &expected, label);
        stats.monitor(&twin_monitor(&image), &expected);
        assert!(
            stats.finish(label),
            "{label}: the centre on the edge is excluded"
        );
    }
}

/// Review-1: an explicit §6 geometry and source-over oracle (no transition
/// resolution, `params_for`, `push_at` or twin blend/sampling) over every
/// frame of every geometric transition, 1×/4× scale, Normal/Screen.
fn review1_all_geometric_frames_independent_oracle_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let base = gpu(&mut r, &document(quartered()), 0).unwrap();
    for kind in ["push", "slide", "wipe"] {
        for (edge, sign, axis) in [
            ("left", -1.0_f32, 0),
            ("right", 1.0, 0),
            ("up", -1.0, 1),
            ("down", 1.0, 1),
        ] {
            let name = format!("{kind}_{edge}");
            for scale in [1.0_f32, 4.0] {
                for mode in [BlendMode::Normal, BlendMode::Screen] {
                    let fx = vec![
                        opacity(1, 50),
                        effect(2, "transform", &[("scale_percent", (scale * 100.0) as i64)]),
                        effect(
                            3,
                            "mask",
                            &[
                                ("shape_token", 1),
                                ("width_percent", 50),
                                ("height_percent", 200),
                            ],
                        ),
                    ];
                    for at in 0..=5 {
                        let mut clips = quartered();
                        clips.push(with_transition(solid(4, BLUE, mode, fx.clone()), &name, 5));
                        let doc = document(clips);
                        let label = format!("{name} s={scale} mode={mode:?} frame={at}");
                        let actual = gpu(&mut r, &doc, at).unwrap();
                        let cpu = r.twin_working(&doc, TimeCode(at), doc.resolution).unwrap();
                        assert_r27(&actual.pixels, &cpu.pixels, &label);
                        let p = at as f32 / 4.0;
                        let active = at < 4;
                        let extent = if axis == 0 { W as f32 } else { H as f32 };
                        for y in 0..H {
                            for x in 0..W {
                                let screen =
                                    [(x as f32 + 0.5) / W as f32, (y as f32 + 0.5) / H as f32];
                                let mut at_d = [x as i32, y as i32];
                                if active && kind == "push" {
                                    at_d[axis] += (sign * p * extent) as i32;
                                }
                                let inside = (0..W as i32).contains(&at_d[0])
                                    && (0..H as i32).contains(&at_d[1]);
                                let d = if inside {
                                    px(&base, at_d[0] as u32, at_d[1] as u32)
                                } else {
                                    px(&base, x, y)
                                };
                                let shift = if active && kind != "wipe" {
                                    sign * (1.0 - p)
                                } else {
                                    0.0
                                };
                                let mut uv = screen;
                                uv[axis] -= shift;
                                uv = uv.map(|v| (v - 0.5) / scale + 0.5);
                                let visible = !active
                                    || if sign < 0.0 {
                                        screen[axis] < p
                                    } else {
                                        screen[axis] >= 1.0 - p
                                    };
                                let drawn = visible
                                    && (0.25..=0.75).contains(&uv[0])
                                    && (0.0..=1.0).contains(&uv[1]);
                                let alpha = if drawn { 0.5 } else { 0.0 };
                                let s = working(BLUE);
                                let expected: [f32; 3] = std::array::from_fn(|c| {
                                    let b = if mode == BlendMode::Screen {
                                        1.0 - (1.0 - s[c]) * (1.0 - d[c])
                                    } else {
                                        s[c]
                                    };
                                    store(alpha * b + (1.0 - alpha) * d[c])
                                });
                                let at = format!("{label} ({x},{y})");
                                assert_r27(&px(&actual, x, y), &expected, &at);
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- R18/R19

/// Review-1 (G9): masked adjustments at zero/half/full strength, Normal and
/// Multiply; an exact order swap (double-then-square = 0.25 vs
/// square-then-double = 0.125) and exact upper-track invariance.
fn review1_adjustment_mask_and_exact_order_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let plate = || solid(1, GREY, BlendMode::Normal, vec![]);
    for strength in [0, 50, 100] {
        let mask = [
            ("shape_token", 1),
            ("width_percent", 50),
            ("height_percent", 50),
        ];
        let fx = vec![
            primary(1, &[("exposure_milli_stops", 1_000)]),
            opacity(2, strength),
            effect(3, "mask", &mask),
        ];
        for mode in [BlendMode::Normal, BlendMode::Multiply] {
            let doc = document(vec![plate(), adjustment(2, mode, fx.clone())]);
            let out = matched(&mut r, &doc, 0, "adjustment strength mask");
            let d = working(GREY);
            let a = strength as f32 / 100.0;
            let expected = std::array::from_fn::<_, 3, _>(|c| {
                let graded = if mode == BlendMode::Normal {
                    2.0 * d[c]
                } else {
                    2.0 * d[c] * d[c]
                };
                store(a * graded + (1.0 - a) * d[c])
            });
            assert_r27(&px(&out, 80, 44), &expected, "masked centre");
            assert_eq!(
                px(&out, 1, 1),
                d,
                "the mask exterior is exactly the below-stack"
            );
        }
    }
    // A doubles; B is Multiply with an identity grade, so it squares.
    let base = solid(
        1,
        [255; 3],
        BlendMode::Normal,
        vec![primary(1, &[("exposure_milli_stops", -2_000)])],
    );
    let a = adjustment(
        2,
        BlendMode::Normal,
        vec![primary(1, &[("exposure_milli_stops", 1_000)])],
    );
    let b = adjustment(3, BlendMode::Multiply, vec![]);
    let upper = gpu(
        &mut r,
        &document(vec![solid(4, BLUE, BlendMode::Normal, vec![])]),
        0,
    )
    .unwrap();
    for (stack, expected) in [
        (vec![base.clone(), a.clone(), b.clone()], 0.25),
        (vec![base.clone(), b.clone(), a.clone()], 0.125),
    ] {
        let out = matched(
            &mut r,
            &document(stack.clone()),
            0,
            "exact adjustment order",
        );
        let exact = [expected, expected, expected, 1.0];
        assert!(out.pixels.chunks(4).all(|p| p == exact), "{expected}");
        let mut covered = stack;
        covered.push(solid(4, BLUE, BlendMode::Normal, vec![]));
        let with_upper = matched(&mut r, &document(covered), 0, "upper invariant");
        assert_eq!(with_upper.pixels, upper.pixels);
    }
}

fn curve(points: &[(i64, i64)], hold: bool) -> AutomationCurve {
    let interpolation = if hold {
        KeyframeInterpolation::Hold
    } else {
        KeyframeInterpolation::Linear
    };
    AutomationCurve {
        keyframes: points
            .iter()
            .map(|&(at, value)| Keyframe {
                at: TimeCode(at),
                value,
                interpolation,
                tangent_in: 0,
                tangent_out: 0,
            })
            .collect(),
    }
}

/// Review-1 (G9): clip-local value and enable keys on solids and
/// adjustments, including keys outside the clip, equal the static result.
fn review1_generated_keep_outside_enable_keys_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    for adjust in [false, true] {
        let fx = vec![
            primary(1, &[("exposure_milli_stops", 1_000)]),
            opacity(2, 0),
        ];
        let mut keyed = if adjust {
            adjustment(2, BlendMode::Normal, fx)
        } else {
            solid(2, BLUE, BlendMode::Normal, fx)
        };
        keyed.timeline_start = TimeCode(10);
        let percent = curve(&[(-2, 0), (10, 100)], false);
        keyed.effects[1].keyframes.insert("percent".into(), percent);
        keyed.enabled_curve = Some(curve(&[(-3, 1), (3, 0), (5, 1), (12, 0)], true));
        keyed.effects[0].enabled_curve = Some(curve(&[(-5, 0), (2, 1), (20, 0)], true));
        let mut doc = document(vec![solid(1, GREY, BlendMode::Normal, vec![])]);
        doc.tracks[0].clips[0].source_range.end = TimeCode(19);
        doc.duration = TimeCode(19);
        doc.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![keyed.clone()],
        });
        doc.validate().unwrap();
        for local in [0, 2, 3, 4, 5, 8] {
            let out = matched(&mut r, &doc, 10 + local, "keep-outside clip-local keys");
            let mut static_doc = doc.clone();
            let plain = &mut static_doc.tracks[1].clips[0];
            plain.enabled = !(3..5).contains(&local);
            plain.enabled_curve = None;
            plain.effects[0].enabled = local >= 2;
            plain.effects[0].enabled_curve = None;
            let percent = ParamValue::Integer(((local + 2) * 100 + 6) / 12);
            plain.effects[1]
                .parameters
                .insert("percent".into(), percent);
            plain.effects[1].keyframes.clear();
            let expected = gpu(&mut r, &static_doc, 10 + local).unwrap();
            assert_eq!(
                out.pixels, expected.pixels,
                "adjustment={adjust} local={local}"
            );
        }
    }
}

/// Review-1: every renderer root and the twin refuse an invalid document
/// (a disabled adjustment with an unsupported transition) identically.
fn review1_validate_all_renderer_roots_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let mut doc = document(vec![adjustment(1, BlendMode::Normal, vec![])]);
    let clip = &mut doc.tracks[0].clips[0];
    clip.enabled = false;
    clip.transition_in = Some(Transition {
        name: "fade_from_black".into(),
        duration: TimeCode(5),
    });
    let expected = Some(MediaError::InvalidDocument(Box::new(
        doc.validate().unwrap_err(),
    )));
    let (at, size, (scale, seek)) = (TimeCode(0), doc.resolution, full());
    assert_eq!(r.render(&doc, at, size, scale, seek).err(), expected);
    assert_eq!(
        r.render_working(&doc, at, size, scale, seek).err(),
        expected
    );
    assert_eq!(
        r.render_delivery(&doc, at, size, scale, seek).err(),
        expected
    );
    let matte = r.render_matte(&doc, at, size, scale, seek, ClipId(1), EffectId(1));
    assert_eq!(matte.err(), expected);
    assert_eq!(r.twin_working(&doc, at, size).err(), expected);
}

// ---------------------------------------------------------------- proofs

/// Review-1 B3: the public proof of an adjustment's luma qualifier over grey
/// qualifies what the adjustment grades — the stack below it — exactly as
/// the full-stack renderer does.
fn review1_public_matte_adjustment_qualifier_keeps_below_on(context: GpuContext) {
    use kinewright_core::Analysis;
    let directory = crate::test_support::TempDirectory::new("mo2-b1-adjustment-matte");
    let data = directory.path("data");
    let engine = crate::FfmpegMediaEngine::new_with_gpu_and_data_dir(context.clone(), data);
    let look = effect(
        1,
        "color_wheels",
        &[
            ("gain_master_thousandths", 1_500),
            ("matte_enabled", 1),
            ("matte_qualifier_enabled", 1),
            ("matte_luma_low_basis_points", 3_000),
            ("matte_luma_high_basis_points", 7_000),
        ],
    );
    let plate = solid(1, GREY, BlendMode::Normal, vec![]);
    let doc = document(vec![plate, adjustment(2, BlendMode::Normal, vec![look])]);
    let mut renderer = FrameRenderer::new(context);
    let (scale, seek) = full();
    let reference = renderer
        .render_matte(
            &doc,
            TimeCode(0),
            doc.resolution,
            scale,
            seek,
            ClipId(2),
            EffectId(1),
        )
        .unwrap();
    assert!(
        reference.coverage.iter().all(|v| *v == 255),
        "grey qualifies"
    );
    let proof = engine
        .unwrap()
        .matte_proof_for_document(Arc::new(doc), TimeCode(0), ClipId(2), EffectId(1))
        .unwrap();
    let values: Vec<u8> = proof.coverage.pixels.chunks(4).map(|p| p[0]).collect();
    assert_eq!(
        values, reference.coverage,
        "R18: the proof qualifies the below-stack"
    );
}

/// Review-1 B2: a valid 9-frame document whose target clip spans 6 frames
/// still proves (the scratch projection stays a valid document).
fn review1_public_matte_shorter_clip_remains_valid_on(context: GpuContext) {
    use kinewright_core::Analysis;
    let directory = crate::test_support::TempDirectory::new("mo2-b1-shorter-matte");
    let data = directory.path("data");
    let engine = crate::FfmpegMediaEngine::new_with_gpu_and_data_dir(context, data).unwrap();
    let look = effect(
        1,
        "color_wheels",
        &[
            ("gain_master_thousandths", 1_500),
            ("matte_enabled", 1),
            ("matte_window_count", 1),
            ("matte_window0_shape_token", 1),
        ],
    );
    let mut top = solid(2, BLUE, BlendMode::Normal, vec![look]);
    top.source_range.end = TimeCode(6);
    let doc = document(vec![solid(1, GREY, BlendMode::Normal, vec![]), top]);
    let result =
        engine.matte_proof_for_document(Arc::new(doc), TimeCode(0), ClipId(2), EffectId(1));
    let proof = result.expect("a valid document's derived proof renders");
    assert_eq!((proof.coverage.width, proof.coverage.height), (W, H));
}

// ---------------------------------------------------------------- R16

/// Review-1 S1: the twin reproduces a supported legacy `cube_lut` (an
/// identity and a channel-swapping lattice) on an adjustment and a solid.
fn review1_twin_covers_supported_legacy_cube_on(context: GpuContext) {
    let directory = crate::test_support::TempDirectory::new("mo2-b1-legacy-cube");
    let mut r = FrameRenderer::new(context);
    let lattices = [
        (
            "identity.cube",
            "0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
        ),
        (
            "swap.cube",
            "0 0 0\n0 1 0\n1 0 0\n1 1 0\n0 0 1\n0 1 1\n1 0 1\n1 1 1\n",
        ),
    ];
    for (name, body) in lattices {
        let path = directory.path(name);
        std::fs::write(&path, format!("LUT_3D_SIZE 2\n{body}")).unwrap();
        let mut lut = effect(1, "cube_lut", &[("intensity_percent", 70)]);
        let path = ParamValue::Text(path.to_string_lossy().into_owned());
        lut.parameters.insert("path".into(), path);
        let plate = solid(1, ORANGE, BlendMode::Normal, vec![]);
        for top in [
            adjustment(2, BlendMode::Normal, vec![lut.clone()]),
            // 70%, not the review's 60%: at 60% the exact BLUE Screen over
            // ORANGE lies just above an f16 rounding midpoint and lavapipe's
            // blend store rounds it down with no LUT at all (0.87353516 vs
            // 0.87402344), one monitor code on every pixel of this uniform
            // frame. The LUT's coverage is unchanged.
            solid(
                2,
                BLUE,
                BlendMode::Screen,
                vec![lut.clone(), opacity(2, 70)],
            ),
        ] {
            matched(&mut r, &document(vec![plate.clone(), top]), 0, name);
        }
    }
}

gpu_lanes! {
    review1_r10_normal_adjustment_storage_must_refuse => review1_r10_normal_adjustment_storage_must_refuse_on,
    review1_r10_nan_extrema_must_refuse => review1_r10_nan_extrema_must_refuse_on,
    review1_r10_valid_darken_overflow_must_refuse => review1_r10_valid_darken_overflow_must_refuse_on,
    review2_nonfinite_inputs_and_normal_adjustment => review2_nonfinite_inputs_and_normal_adjustment_on,
    review2_representable_result_must_not_refuse => review2_representable_result_must_not_refuse_on,
    review2_independent_blend_grid => review2_independent_blend_grid_on,
    review2_independent_transition_grid => review2_independent_transition_grid_on,
    review2_exact_pixel_centre_coverage => review2_exact_pixel_centre_coverage_on,
    review1_all_geometric_frames_independent_oracle => review1_all_geometric_frames_independent_oracle_on,
    review1_adjustment_mask_and_exact_order => review1_adjustment_mask_and_exact_order_on,
    review1_generated_keep_outside_enable_keys => review1_generated_keep_outside_enable_keys_on,
    review1_validate_all_renderer_roots => review1_validate_all_renderer_roots_on,
    review1_public_matte_adjustment_qualifier_keeps_below => review1_public_matte_adjustment_qualifier_keeps_below_on,
    review1_public_matte_shorter_clip_remains_valid => review1_public_matte_shorter_clip_remains_valid_on,
    review1_twin_covers_supported_legacy_cube => review1_twin_covers_supported_legacy_cube_on,
}

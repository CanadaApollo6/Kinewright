//! MO2 final render verification (final-mo2-1, S1): the survivor probes,
//! retained as committed regressions. Each kills a mutation the rest of the
//! suite let through — F02/F07 sampled-alpha guards (`Normal` GPU and twin),
//! F05 subnormal RTE quantum, F06 `Normal`-adjustment RTE, F08/F09 the ME9
//! topmost/effect restrictions, F16 a zero `render_timed` duration.
//!
//! The RTE oracle enumerates half values as exact binary rationals and picks
//! the nearest (ties to even); it never uses a half conversion or the
//! shader's exponent/quantum algorithm to compute expected values.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::manual_midpoint,
    clippy::chunks_exact_to_as_chunks,
    clippy::if_not_else,
    clippy::bool_to_int_with_if
)]

use super::*;
use wgpu::util::DeviceExt;

fn context() -> GpuContext {
    fixture_gpu_or_skip().expect("lavapipe")
}

// Independent reference: enumerate half numbers as exact binary rational f64,
// binary-search neighbours, select minimum distance and even low bit. No half
// conversion or copy of the shader's exponent/quantum algorithm is used.
fn half_value(h: u16) -> f64 {
    let e = i32::from(h >> 10);
    let m = f64::from(h & 1023);
    if e == 0 {
        m * 2_f64.powi(-24)
    } else {
        (1024.0 + m) * 2_f64.powi(e - 25)
    }
}

fn reference(x: f32) -> f32 {
    let a = f64::from(x.abs());
    if a >= 65520.0 {
        return f32::INFINITY.copysign(x);
    }
    let (mut lo, mut hi) = (0_u16, 0x7bff_u16);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if half_value(mid) <= a {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let out = if lo == 0x7bff {
        lo
    } else {
        let down = a - half_value(lo);
        let up = half_value(lo + 1) - a;
        if down < up || (down == up && lo & 1 == 0) {
            lo
        } else {
            lo + 1
        }
    };
    (half_value(out) as f32).copysign(x)
}

#[test]
fn final_rte_all_half_boundaries_and_dense_f32() {
    let gpu = context();
    let mut values = Vec::new();
    for h in 0..=0x7bff_u16 {
        let lo = half_value(h) as f32;
        let hi = if h == 0x7bff {
            65536.0
        } else {
            half_value(h + 1) as f32
        };
        let mid = (lo + hi) * 0.5;
        for x in [lo, mid.next_down(), mid, mid.next_up()] {
            values.extend([x, -x]);
        }
    }
    for exp in 0..255_u32 {
        for frac in 0..2048_u32 {
            let bits = (exp << 23) | (frac << 12);
            values.extend([f32::from_bits(bits), f32::from_bits(bits | 0x8000_0000)]);
        }
    }
    values.extend([
        f32::MAX,
        -f32::MAX,
        65504.0_f32.next_up(),
        65520.0_f32.next_down(),
    ]);
    let source = include_str!("compositor.wgsl");
    let start = source.find("fn f16_rte(").unwrap();
    let end = start + source[start..].find("\n}\n").unwrap() + 2;
    let function = &source[start..end];
    let shader = format!(
        "{function}\n@group(0) @binding(0) var<storage,read> input:array<f32>;\n@group(0) @binding(1) var<storage,read_write> output:array<u32>;\n@compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id:vec3<u32>) {{ if id.x < arrayLength(&input) {{ output[id.x]=bitcast<u32>(f16_rte(vec3<f32>(input[id.x])).x); }} }}"
    );
    let shader = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("independent RTE sweep"),
            source: wgpu::ShaderSource::Wgsl(shader.into()),
        });
    let pipeline = gpu
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let bytes: Vec<u8> = values.iter().flat_map(|x| x.to_le_bytes()).collect();
    let input = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
    let output = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bindings = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.dispatch_workgroups((values.len() as u32).div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes.len() as u64);
    gpu.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = readback.slice(..).get_mapped_range();
    let mut mismatches = Vec::new();
    for (x, b) in values.iter().zip(mapped.chunks_exact(4)) {
        let got = u32::from_le_bytes(b.try_into().unwrap());
        let expected = reference(*x).to_bits();
        if got != expected && mismatches.len() < 16 {
            mismatches.push((x.to_bits(), got, expected));
        }
        assert_eq!(
            f16::from_f32(*x).to_f32().to_bits(),
            expected,
            "independent reference vs CPU x={x}"
        );
    }
    println!(
        "RTE_SWEEP values={} all signed finite half values, every midpoint and +/-1 f32 ULP, 2048 mantissas per f32 exponent; mismatches={mismatches:x?}",
        values.len()
    );
    assert!(mismatches.is_empty());
}

#[test]
fn final_me9_just_outside_each_precondition() {
    let ramp = row(&[grey4(0.0), grey4(1.0)]);
    let uniform = row(&[grey4(0.5)]);
    let varying = row(&[[0.0, 0.0, 0.0, 0.5], [1.0, 1.0, 1.0, 1.0]]);
    let moves = vec![effect(1, "transform", &[("x_basis_points", 1)])];
    let nonlinear = vec![primary(1, &[("exposure_milli_stops", 1)])];
    fn top<'a>(
        f: &'a WorkingFrame,
        fx: &'a [Effect],
        mode: LayerMode,
        transition: TransitionRenderParams,
    ) -> CompositorLayer<'a, WorkingFrame> {
        CompositorLayer {
            frame: f,
            effects: fx,
            mode,
            transition,
        }
    }
    let normal = LayerMode::NORMAL;
    let transition = TransitionRenderParams::default();
    let good = || top(&ramp, &moves, normal, transition);
    assert!(twin::subtexel_envelope((17, 1), &[good()], None).is_ok());
    let mut failures = Vec::new();
    let cases = vec![
        (
            "two resamples",
            vec![pixels_layer(&ramp, BlendMode::Normal), good()],
        ),
        (
            "later uniform layer",
            vec![good(), pixels_layer(&uniform, BlendMode::Normal)],
        ),
        (
            "nonlinear grade",
            vec![top(&ramp, &nonlinear, normal, transition)],
        ),
        (
            "varying alpha",
            vec![top(&varying, &moves, normal, transition)],
        ),
        (
            "non-normal",
            vec![top(
                &ramp,
                &moves,
                LayerMode {
                    blend: BlendMode::Add,
                    role: LayerRole::Pixels,
                },
                transition,
            )],
        ),
        (
            "adjustment",
            vec![top(
                &ramp,
                &moves,
                LayerMode {
                    blend: BlendMode::Normal,
                    role: LayerRole::Adjustment,
                },
                transition,
            )],
        ),
        (
            "push anywhere",
            vec![
                top(
                    &uniform,
                    &[],
                    normal,
                    TransitionRenderParams {
                        backdrop: Some([0.0, 0.0]),
                        ..transition
                    },
                ),
                good(),
            ],
        ),
    ];
    for (label, layers) in cases {
        if twin::subtexel_envelope((17, 1), &layers, None).is_ok() {
            failures.push(label);
        }
    }
    assert!(
        failures.is_empty(),
        "outside proved subset accepted: {failures:?}"
    );
}

#[test]
fn final_r10_boundaries_all_layer_paths() {
    let c = Compositor::new(context());
    let mut count = 0;
    // Exactly max and one representable half ULP above (65536 uploads as inf),
    // all nonfinites including sampled alpha, and fade/mask erasure attempts.
    for mode in BlendMode::ALL {
        for role in [LayerRole::Pixels, LayerRole::Adjustment] {
            for transition_kind in ["none", "fade", "push", "wipe"] {
                let transition = match transition_kind {
                    "fade" => TransitionRenderParams {
                        fade_mix: 0.5,
                        fade_white: 1.0,
                        ..Default::default()
                    },
                    "push" => TransitionRenderParams {
                        backdrop: Some([-0.5, 0.0]),
                        offset: [0.5, 0.0],
                        ..Default::default()
                    },
                    "wipe" => TransitionRenderParams {
                        coverage: Some(crate::timeline::TransitionCoverage {
                            axis: kinewright_core::TransitionAxis::Horizontal,
                            below_edge: true,
                            edge: 0.5,
                        }),
                        ..Default::default()
                    },
                    _ => TransitionRenderParams::default(),
                };
                for masked in [false, true] {
                    let fx = if masked {
                        vec![effect(
                            1,
                            "mask",
                            &[
                                ("shape_token", 1),
                                ("width_percent", 1),
                                ("height_percent", 1),
                            ],
                        )]
                    } else {
                        vec![]
                    };
                    for (label, rgb, alpha) in [
                        ("max", 65504.0, 1.0),
                        ("one_half_ulp_past", 65536.0, 1.0),
                        ("+inf", f32::INFINITY, 1.0),
                        ("-inf", f32::NEG_INFINITY, 1.0),
                        ("nan_rgb", f32::NAN, 1.0),
                        ("nan_alpha", 0.5, f32::NAN),
                        ("+inf_alpha", 0.5, f32::INFINITY),
                        ("-inf_alpha", 0.5, f32::NEG_INFINITY),
                    ] {
                        let src = row(&[[rgb, rgb, rgb, alpha]; 4]);
                        let base = row(&[grey4(0.0); 4]);
                        // Adjustment's input is its below-stack, not its placeholder.
                        let layers = if matches!(role, LayerRole::Adjustment) {
                            vec![
                                pixels_layer(&src, BlendMode::Normal),
                                CompositorLayer {
                                    frame: &base,
                                    effects: &fx,
                                    transition,
                                    mode: LayerMode { blend: mode, role },
                                },
                            ]
                        } else {
                            vec![
                                pixels_layer(&base, BlendMode::Normal),
                                CompositorLayer {
                                    frame: &src,
                                    effects: &fx,
                                    transition,
                                    mode: LayerMode { blend: mode, role },
                                },
                            ]
                        };
                        let a = c.render_working((4, 1), &layers);
                        let b = twin::render_working((4, 1), &layers, None);
                        let tag =
                            format!("{mode:?}/{role:?}/{transition_kind}/mask={masked}/{label}");
                        if label != "max" {
                            let expected_layer = if matches!(role, LayerRole::Adjustment) {
                                0
                            } else {
                                1
                            };
                            refused(a, expected_layer, &tag);
                            refused(b, expected_layer, &tag);
                        } else {
                            match (a, b) {
                                (Ok(a), Ok(b)) => assert_eq!(a.pixels, b.pixels, "{tag}"),
                                (
                                    Err(MediaError::NonFiniteRender { layer: a, .. }),
                                    Err(MediaError::NonFiniteRender { layer: b, .. }),
                                ) => assert_eq!(a, b, "{tag}"),
                                (a, b) => panic!("{tag} GPU={a:?} twin={b:?}"),
                            }
                        }
                        count += 1;
                    }
                }
            }
        }
    }
    println!("R10_GRID {count} cases x GPU/twin");
}

#[test]
fn final_rte_adjustment_midpoints() {
    let c = Compositor::new(context());
    let grade = vec![primary(1, &[("exposure_milli_stops", 1000)])];
    for d in [1.0_f32, 1.000_976_6, 2048.0, 2050.0] {
        let source = row(&[grey4(d)]);
        let step = if d < 2.0 { 2.0_f32.powi(-11) } else { 1.0 };
        for delta in [step.next_down(), step, step.next_up(), 3.0 * step] {
            let alpha = delta / d;
            let layers = [
                pixels_layer(&source, BlendMode::Normal),
                CompositorLayer {
                    frame: &source,
                    effects: &grade,
                    transition: TransitionRenderParams {
                        alpha,
                        ..Default::default()
                    },
                    mode: LayerMode {
                        blend: BlendMode::Normal,
                        role: LayerRole::Adjustment,
                    },
                },
            ];
            let a = c.render_working((1, 1), &layers).unwrap();
            let b = twin::render_working((1, 1), &layers, None).unwrap();
            assert_eq!(a.pixels, b.pixels, "adjustment d={d} delta={delta}");
        }
    }
}

/// F16: the compositor frame time is measured and bounded by the call.
#[test]
fn final_render_timed_measures_inside_the_call() {
    let mut r = FrameRenderer::new(context());
    let doc = document_sized((3, 3), vec![solid(1, GREY, BlendMode::Normal, vec![])]);
    r.set_cache_budget(1 << 30);
    let full = (RenderScale::FullResolution, DecodeStrategy::Sequential);
    r.render_timed(&doc, TimeCode(0), doc.resolution, full.0, full.1)
        .unwrap();
    let started = std::time::Instant::now();
    let (_, measured) = r
        .render_timed(&doc, TimeCode(0), doc.resolution, full.0, full.1)
        .unwrap();
    let total = started.elapsed();
    assert!(
        measured > std::time::Duration::ZERO,
        "a compositor frame takes time"
    );
    assert!(
        measured <= total,
        "measured {measured:?} within the call's {total:?}"
    );
}

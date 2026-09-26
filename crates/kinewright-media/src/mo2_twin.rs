//! MO2 R16: the CPU twin, an explicit raster/sampling reference for the
//! compositor (B8).
//!
//! It reuses `params_for`, the blit rule and the unquantized CPU colour
//! kernels, and independently reproduces rasterization (the vertex transform
//! on an 8-bit sub-pixel grid with the top-left rule), nearest/bilinear
//! clamp-to-edge sampling, the legacy stage, key/spill, fades, crop, mask,
//! coverage, the blend equations, the Push backdrop and the opaque over. It
//! rounds only where the GPU stores (`Rgba16Float` targets and snapshots).
//!
//! Single-letter names follow the shader's; exact float compares and the
//! `(x + 1) * 0.5` forms mirror WGSL expressions on purpose.

#![allow(clippy::many_single_char_names)]
#![allow(clippy::float_cmp)]
#![allow(clippy::manual_midpoint)]

use half::f16;
use kinewright_core::{LinearRgbaImage, MediaError};

use super::{
    Compositor, CompositorInput, CompositorLayer, LayerParams, LayerRole, LutLibrary, blend_word,
    legacy_stage_active, params_for,
};
use crate::color_pipeline::{
    apply_color_nodes_at, decode_bt709, encode_bt709, resolve_color_nodes_with,
};

type Rgba = [f32; 4];

/// One sampled texture in its stored domain, row-major.
struct Texture {
    width: usize,
    height: usize,
    texels: Vec<Rgba>,
}

impl Texture {
    fn of<F: CompositorInput>(frame: &F) -> Self {
        let bytes = frame.upload_bytes();
        let texels = if F::LINEAR {
            let half = |p: &[u8; 8], c: usize| f16::from_le_bytes([p[2 * c], p[2 * c + 1]]);
            let pixels = bytes.as_chunks::<8>().0.iter();
            pixels
                .map(|p| std::array::from_fn(|c| half(p, c).to_f32()))
                .collect()
        } else {
            let pixels = bytes.as_chunks::<4>().0.iter();
            pixels.map(|p| p.map(|b| f32::from(b) / 255.0)).collect()
        };
        Self {
            width: frame.width() as usize,
            height: frame.height() as usize,
            texels,
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap
    )]
    fn texel(&self, x: i64, y: i64) -> Rgba {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        self.texels[y * self.width + x]
    }

    /// The point sampler (nearest) or the filtering sampler (bilinear), both
    /// clamp-to-edge, at a texture coordinate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn sample(&self, uv: [f32; 2], filtering: bool) -> Rgba {
        let (x, y) = (uv[0] * self.width as f32, uv[1] * self.height as f32);
        if !filtering {
            return self.texel(x.floor() as i64, y.floor() as i64);
        }
        let (x, y) = (x - 0.5, y - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let t = |dx: i64, dy: i64| self.texel(x0 as i64 + dx, y0 as i64 + dy);
        let (a, b, c, d) = (t(0, 0), t(1, 0), t(0, 1), t(1, 1));
        std::array::from_fn(|i| {
            (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy
        })
    }
}

fn store(value: f32) -> f32 {
    f16::from_f32(value).to_f32()
}

fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn luma(rgb: [f32; 3]) -> f32 {
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

/// MO2 R9: the per-channel blend `B(S, D)`; `Screen`/`Overlay` extended.
pub(crate) fn blend(mode: u32, s: f32, d: f32) -> f32 {
    let (cs, cd) = (s.clamp(0.0, 1.0), d.clamp(0.0, 1.0));
    let residual = (s - cs) + (d - cd);
    match mode {
        1 => s * d,
        2 => 1.0 - (1.0 - cs) * (1.0 - cd) + residual,
        3 if cd <= 0.5 => 2.0 * cs * cd + residual,
        3 => 1.0 - 2.0 * (1.0 - cs) * (1.0 - cd) + residual,
        4 => s.min(d),
        5 => s.max(d),
        6 => s + d,
        _ => s,
    }
}

/// The quad uv the rasterizer interpolates at an NDC pixel centre (MO1 R3,
/// inverted).
fn quad_uv(p: &LayerParams, ndc: [f32; 2]) -> [f32; 2] {
    let anchor = [p.anchor_x * 2.0 - 1.0, 1.0 - p.anchor_y * 2.0];
    let mut q = [
        ndc[0] - anchor[0] - p.offset_x,
        ndc[1] - anchor[1] + p.offset_y,
    ];
    if p.rotation != 0.0 {
        let (s, c) = p.rotation.sin_cos();
        let r = [q[0], q[1] * p.frame_aspect];
        q = [r[0] * c - r[1] * s, (r[0] * s + r[1] * c) / p.frame_aspect];
    }
    let corner = [q[0] / p.scale_x + anchor[0], q[1] / p.scale_y + anchor[1]];
    [(corner[0] + 1.0) / 2.0, (1.0 - corner[1]) / 2.0]
}

/// Whether the rasterizer covers pixel `(x, y)`: the MO1 R3 vertex transform
/// run forward, corners snapped to 8 sub-pixel bits (the D3D-mandated grid
/// Vulkan drivers share), edge functions with the top-left tie rule.
#[allow(clippy::cast_precision_loss, clippy::float_cmp)]
fn rasterized(p: &LayerParams, size: [f32; 2], pixel: [usize; 2]) -> bool {
    let anchor = [p.anchor_x * 2.0 - 1.0, 1.0 - p.anchor_y * 2.0];
    let corner = |x: f32, y: f32| {
        let c = [(x - anchor[0]) * p.scale_x, (y - anchor[1]) * p.scale_y];
        let r = if p.rotation == 0.0 {
            c
        } else {
            let (s, cos, a) = (p.rotation.sin(), p.rotation.cos(), p.frame_aspect);
            [c[0] * cos + c[1] * a * s, (-c[0] * s + c[1] * a * cos) / a]
        };
        let t = [r[0] + anchor[0] + p.offset_x, r[1] + anchor[1] - p.offset_y];
        let snap = |v: f32| (f64::from(v) * 256.0).round() / 256.0;
        [
            snap((t[0] + 1.0) * 0.5 * size[0]),
            snap((1.0 - t[1]) * 0.5 * size[1]),
        ]
    };
    let mut quad = [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ];
    let edges = |quad: [[f64; 2]; 4]| (0..4).map(move |k| (quad[k], quad[(k + 1) % 4]));
    let area: f64 = edges(quad).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum();
    if area < 0.0 {
        quad.reverse();
    }
    let q = [pixel[0] as f64 + 0.5, pixel[1] as f64 + 0.5];
    area != 0.0
        && edges(quad).all(|(a, b)| {
            let value = (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0]);
            let top_left = (b[1] == a[1] && b[0] > a[0]) || b[1] < a[1];
            value > 0.0 || (value == 0.0 && top_left)
        })
}

/// The legacy display-coded compatibility stage (`legacy_stage_active`).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn legacy(p: &LayerParams, linear: [f32; 3]) -> [f32; 3] {
    let mut rgb = linear.map(|v| (encode_bt709(v) + p.brightness - 0.5) * p.contrast + 0.5);
    let l = luma(rgb);
    rgb = rgb.map(|v| l * (1.0 - p.saturation) + v * p.saturation);
    let pre = rgb;
    let offset =
        |gain: f32, lift: [f32; 3]| std::array::from_fn(|i| (rgb[i] - 0.5) * gain + lift[i]);
    rgb = match p.lut_preset.round() as u32 {
        1 => offset(1.08, [0.54, 0.50, 0.46]),
        2 => offset(1.12, [0.46, 0.50, 0.55]),
        3 => [luma(rgb); 3],
        4 => {
            let l = luma(rgb);
            rgb.map(|v| ((l * 0.65 + v * 0.35) - 0.5) * 1.35 + 0.5)
        }
        _ => rgb,
    };
    let mixed: [f32; 3] =
        std::array::from_fn(|i| pre[i] * (1.0 - p.lut_intensity) + rgb[i] * p.lut_intensity);
    mixed.map(|v| decode_bt709(v.clamp(0.0, 1.0)))
}

/// Shade one fragment: straight working RGB and processed alpha.
///
/// `pixel` is the output pixel and `extent` the raster size: R21 coverage is
/// decided exactly, `(i + 0.5) / n` against the edge in f64 (ME5).
fn shade(
    p: &LayerParams,
    rgb: [f32; 3],
    a: f32,
    uv: [f32; 2],
    pixel: [usize; 2],
    extent: [f32; 2],
) -> Rgba {
    let (mut rgb, mut alpha) = (rgb, (a * p.opacity).clamp(0.0, 1.0));
    if p.legacy_stage_active > 0.5 {
        rgb = legacy(p, rgb);
    }
    if p.key_threshold >= 0.0 {
        let key = [p.key_red, p.key_green, p.key_blue];
        let coded = rgb.map(|v| encode_bt709(v).clamp(0.0, 1.0));
        let distance = (0..3)
            .map(|i| (coded[i] - key[i]).powi(2))
            .sum::<f32>()
            .sqrt()
            / 1.732_050_8;
        let low = (p.key_threshold - p.key_softness).max(0.0);
        let key_alpha = smoothstep(
            low,
            (p.key_threshold + p.key_softness + 0.00001).min(1.0),
            distance,
        );
        alpha *= key_alpha;
        if key_alpha < 1.0 {
            let s = rgb.map(encode_bt709);
            let dominance = (s[1] - s[0].max(s[2])).max(0.0);
            rgb[1] = decode_bt709(s[1] - dominance * p.key_spill * (1.0 - key_alpha));
        }
    }
    rgb = rgb.map(|v| v * (1.0 - p.fade_mix) + p.fade_white * p.fade_mix);
    if p.fade_mix > 0.0 {
        alpha = 1.0;
    }
    if uv[0] < p.crop_left
        || uv[0] > 1.0 - p.crop_right
        || uv[1] < p.crop_top
        || uv[1] > 1.0 - p.crop_bottom
    {
        alpha = 0.0;
    }
    if p.mask_shape > 0.5 {
        let half = [
            (p.mask_width * 0.5).max(0.005),
            (p.mask_height * 0.5).max(0.005),
        ];
        let n = [
            (uv[0] - p.mask_center_x).abs() / half[0],
            (uv[1] - p.mask_center_y).abs() / half[1],
        ];
        let distance = if p.mask_shape > 1.5 {
            n[0].hypot(n[1])
        } else {
            n[0].max(n[1])
        };
        let feather = p.mask_feather * 0.5;
        let mut m = 1.0 - smoothstep((1.0 - feather).max(0.0), 1.0, distance);
        if feather <= 0.00001 {
            m = f32::from(u8::from(distance <= 1.0));
        }
        alpha *= if p.mask_invert > 0.5 { 1.0 - m } else { m };
    }
    if p.coverage_on > 0.5 {
        let axis = usize::from(p.coverage_axis > 0.5);
        #[allow(clippy::cast_precision_loss)]
        let centre = pixel[axis] as f64 + 0.5;
        let below = centre < f64::from(p.coverage_edge) * f64::from(extent[axis]);
        let keep = below == (p.coverage_on < 1.5);
        alpha *= f32::from(u8::from(keep));
    }
    [rgb[0], rgb[1], rgb[2], alpha]
}

/// Composite `layers` bottom-to-top on the CPU, as `render_working` does.
///
/// # Panics
///
/// On an active legacy `cube_lut`, which the twin does not reproduce.
#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
pub(crate) fn render_working<F: CompositorInput>(
    resolution: (u32, u32),
    layers: &[CompositorLayer<'_, F>],
    library: Option<&LutLibrary>,
) -> Result<LinearRgbaImage, MediaError> {
    let (width, height) = (resolution.0 as usize, resolution.1 as usize);
    let (w, h) = (width as f32, height as f32);
    let centre = |i: usize| {
        [
            ((i % width) as f32 + 0.5) / w,
            ((i / width) as f32 + 0.5) / h,
        ]
    };
    let mut accumulator = vec![[0.0, 0.0, 0.0, 1.0]; width * height];
    let mut flagged = None;
    let empty = LutLibrary::default();
    for (index, layer) in layers.iter().enumerate() {
        assert!(
            !layer
                .effects
                .iter()
                .any(|e| e.enabled && e.name == "cube_lut"),
            "the twin does not reproduce the legacy cube_lut"
        );
        let d0 = Texture {
            width,
            height,
            texels: accumulator.clone(),
        };
        if let Some(shift) = layer.transition.backdrop {
            // R13: `D0(x − q)` inside the raster, unshifted `D0(x)` outside.
            for (i, texel) in accumulator.iter_mut().enumerate() {
                let [x, y] = centre(i);
                let source = [x - shift[0], y - shift[1]];
                if source.iter().all(|v| (0.0..1.0).contains(v)) {
                    let [r, g, b, _] = d0.sample(source, true);
                    *texel = [store(r), store(g), store(b), 1.0];
                }
            }
        }
        let adjustment = matches!(layer.mode.role, LayerRole::Adjustment);
        let mut p = params_for(layer.effects, layer.transition);
        p.input_linear = f32::from(u8::from(F::LINEAR || adjustment));
        p.legacy_stage_active = f32::from(u8::from(legacy_stage_active(layer.effects)));
        p.frame_aspect = h / w;
        p.blend_mode = blend_word(layer.mode.blend);
        let filtering = !Compositor::is_pixel_exact_blit(layer, &p, resolution.0, resolution.1);
        let nodes = resolve_color_nodes_with(layer.effects, library.unwrap_or(&empty))
            .map_err(|error| MediaError::Backend(error.to_string()))?;
        let source = if adjustment {
            d0
        } else {
            Texture::of(layer.frame)
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let mode = p.blend_mode as u32;
        for (i, texel) in accumulator.iter_mut().enumerate() {
            if !rasterized(&p, [w, h], [i % width, i / width]) {
                continue;
            }
            let screen = centre(i);
            let uv = quad_uv(&p, [screen[0] * 2.0 - 1.0, 1.0 - screen[1] * 2.0]);
            let mut sample_uv = uv;
            if p.reframe_aspect > 0.0 {
                let aspect = source.width as f32 / source.height.max(1) as f32;
                let visible = (p.reframe_aspect / aspect).min(aspect / p.reframe_aspect);
                let (axis, focus) = if aspect > p.reframe_aspect {
                    (0, p.reframe_focus_x)
                } else {
                    (1, p.reframe_focus_y)
                };
                if aspect != p.reframe_aspect {
                    let start = (focus - visible * 0.5).clamp(0.0, 1.0 - visible);
                    sample_uv[axis] = start + uv[axis] * visible;
                }
            }
            let [r, g, b, a] = source.sample(sample_uv, filtering);
            let mut rgb = [r, g, b];
            if p.input_linear < 0.5 {
                rgb = rgb.map(decode_bt709);
            }
            rgb = apply_color_nodes_at(&nodes, rgb, uv, w / h);
            let [r, g, b, alpha] = shade(&p, rgb, a, uv, [i % width, i / width], [w, h]);
            let below = *texel;
            let over = |s: f32, d: f32| alpha * s + (1.0 - alpha) * d;
            let blended = if mode == 0 {
                [r, g, b]
            } else {
                std::array::from_fn(|c| blend(mode, [r, g, b][c], below[c]))
            };
            let out: [f32; 3] = std::array::from_fn(|c| over(blended[c], below[c]));
            // R10: every special layer; finite operands and intermediates,
            // magnitude only at the store.
            let finite = [r, g, b, alpha, below[0], below[1], below[2]]
                .iter()
                .chain(&blended)
                .chain(&out)
                .all(|v| v.is_finite());
            let invalid = !finite || out.iter().any(|v| v.abs() > 65504.0);
            if (mode != 0 || adjustment) && flagged.is_none() && invalid {
                flagged = Some(index);
            }
            *texel = [
                store(out[0]),
                store(out[1]),
                store(out[2]),
                store(over(1.0, below[3])),
            ];
        }
    }
    if let Some(layer) = flagged {
        return Err(MediaError::NonFiniteRender {
            layer,
            clip: None,
            at: None,
        });
    }
    Ok(LinearRgbaImage {
        width: resolution.0,
        height: resolution.1,
        pixels: accumulator.into_iter().flatten().collect(),
    })
}

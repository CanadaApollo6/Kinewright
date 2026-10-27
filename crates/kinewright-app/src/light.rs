//! Frame-derived light. All sampling and filtering runs on the preview worker.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
use eframe::egui::{self, Color32, Pos2, Rect, TextureHandle};
use std::time::Instant;

pub(crate) const WIDTH: usize = 32;
pub(crate) const HEIGHT: usize = 18;
#[derive(Clone)]
pub(crate) struct Sample {
    pub(crate) document: usize,
    pub(crate) at: kinewright_core::TimeCode,
    pixels: Vec<[f32; 3]>,
    glass_pixels: Vec<[f32; 3]>,
    pub(crate) worker_ms: f64,
}
pub(crate) fn subscribe() -> crossbeam_channel::Receiver<Sample> {
    let (tx, rx) = crossbeam_channel::bounded(3);
    kinewright_media::po0_light::install(Box::new(move |document, at, width, height, pixel| {
        let began = Instant::now();
        let pixels: Vec<[f32; 3]> = (0..WIDTH * HEIGHT)
            .map(|index| {
                let x = ((index % WIDTH) as u32 * width + width / 2) / WIDTH as u32;
                let y = ((index / WIDTH) as u32 * height + height / 2) / HEIGHT as u32;
                pixel(x.min(width - 1), y.min(height - 1))
            })
            .collect();
        let pixels = blur(&pixels, 3);
        let glass_pixels = blur(&pixels, 6);
        let _ = tx.try_send(Sample {
            document,
            at,
            pixels,
            glass_pixels,
            worker_ms: began.elapsed().as_secs_f64() * 1000.0,
        });
    }));
    rx
}
fn blur(input: &[[f32; 3]], radius: usize) -> Vec<[f32; 3]> {
    let mut horizontal = vec![[0.0; 3]; WIDTH * HEIGHT];
    let mut output = horizontal.clone();
    for (destination, source, horizontal_pass) in [(&mut horizontal, input, true)] {
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let mut n = 0.0;
                for delta in 0..=radius * 2 {
                    let coordinate = (if horizontal_pass { x } else { y })
                        .saturating_add(delta)
                        .saturating_sub(radius);
                    let index = if horizontal_pass {
                        y * WIDTH + coordinate.min(WIDTH - 1)
                    } else {
                        coordinate.min(HEIGHT - 1) * WIDTH + x
                    };
                    for channel in 0..3 {
                        destination[y * WIDTH + x][channel] += source[index][channel];
                    }
                    n += 1.0;
                }
                for channel in &mut destination[y * WIDTH + x] {
                    *channel /= n;
                }
            }
        }
    }
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            for delta in 0..=radius * 2 {
                let yy = y
                    .saturating_add(delta)
                    .saturating_sub(radius)
                    .min(HEIGHT - 1);
                for channel in 0..3 {
                    output[y * WIDTH + x][channel] +=
                        horizontal[yy * WIDTH + x][channel] / (radius * 2 + 1) as f32;
                }
            }
        }
    }
    output
}
#[allow(
    clippy::many_single_char_names,
    reason = "standard OKLab matrix coordinates"
)]
fn saturated(rgb: [f32; 3]) -> Color32 {
    // OKLab conversion, increasing a and b together preserves OKLCH hue.
    let linear = rgb.map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    let [r, g, b] = linear;
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let ll = 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s;
    let aa = (1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s) * 1.6;
    let bb = (0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s) * 1.6;
    let l = (ll + 0.396_337_78 * aa + 0.215_803_76 * bb).powi(3);
    let m = (ll - 0.105_561_346 * aa - 0.063_854_17 * bb).powi(3);
    let s = (ll - 0.089_484_18 * aa - 1.291_485_5 * bb).powi(3);
    let rgb = [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ];
    let [r, g, b] = rgb.map(|v| {
        let v = v.clamp(0.0, 1.0);
        let v = if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (v * 255.0).round() as u8
    });
    Color32::from_rgb(r, g, b)
}
fn image(pixels: &[[f32; 3]]) -> egui::ColorImage {
    egui::ColorImage::new(
        [WIDTH, HEIGHT],
        pixels.iter().copied().map(saturated).collect(),
    )
}

pub(crate) struct Light {
    pub(crate) samples: crossbeam_channel::Receiver<Sample>,
    from: Vec<[f32; 3]>,
    target: Vec<[f32; 3]>,
    current: Vec<[f32; 3]>,
    glass_from: Vec<[f32; 3]>,
    glass_target: Vec<[f32; 3]>,
    glass_current: Vec<[f32; 3]>,
    pending: Option<Sample>,
    began: f64,
    document: usize,
    duration: f64,
    glow: Option<TextureHandle>,
    glass: Option<TextureHandle>,
    grain: Option<crate::grain::Grain>,
    renderer: eframe::egui_wgpu::RenderState,
    pub(crate) lit: Color32,
    pub(crate) worker_ms: Vec<f64>,
    pub(crate) sampled_frames: Vec<i64>,
}
impl Light {
    pub(crate) fn new(renderer: &eframe::egui_wgpu::RenderState, enabled: bool) -> Self {
        Self {
            samples: if enabled {
                subscribe()
            } else {
                crossbeam_channel::bounded(1).1
            },
            renderer: renderer.clone(),
            from: vec![[0.0; 3]; WIDTH * HEIGHT],
            target: vec![[0.0; 3]; WIDTH * HEIGHT],
            current: vec![[0.0; 3]; WIDTH * HEIGHT],
            glass_from: vec![[0.0; 3]; WIDTH * HEIGHT],
            glass_target: vec![[0.0; 3]; WIDTH * HEIGHT],
            glass_current: vec![[0.0; 3]; WIDTH * HEIGHT],
            pending: None,
            began: 0.0,
            document: 0,
            duration: 0.0,
            glow: None,
            glass: None,
            grain: None,
            lit: Color32::from_rgb(138, 138, 142),
            worker_ms: Vec::new(),
            sampled_frames: Vec::new(),
        }
    }
    pub(crate) fn update(&mut self, ui: &egui::Ui, document: usize, at: kinewright_core::TimeCode) {
        let now = ui.input(|i| i.time);
        let mut sample = None;
        if let Some(pending) = self.pending.take() {
            if pending.document == document && pending.at == at {
                sample = Some(pending);
            } else if pending.document == document && pending.at > at {
                self.pending = Some(pending);
            }
        }
        while let Ok(next) = self.samples.try_recv() {
            if next.document != document {
                continue;
            }
            if next.at == at {
                sample = Some(next);
            } else if next.at > at {
                self.pending = Some(next);
            }
        }
        if let Some(sample) = sample {
            self.from.clone_from(&self.current);
            self.glass_from.clone_from(&self.glass_current);
            self.glass_target = sample.glass_pixels;
            self.target = sample.pixels;
            self.began = now;
            self.duration = if crate::motion::reduced(ui.ctx()) {
                0.0
            } else if self.document == document {
                0.250
            } else {
                f64::from(crate::motion::Token::Ambient.seconds())
            };
            self.document = document;
            if self.worker_ms.len() < 100_000 {
                self.worker_ms.push(sample.worker_ms);
                self.sampled_frames.push(sample.at.0);
            }
        }
        let t = if self.duration == 0.0 {
            1.0
        } else {
            ((now - self.began) / self.duration).clamp(0.0, 1.0) as f32
        };
        let t = crate::motion::ease_in_out(t);
        for ((pixel, from), to) in self.current.iter_mut().zip(&self.from).zip(&self.target) {
            *pixel = std::array::from_fn(|c| egui::lerp(from[c]..=to[c], t));
        }
        for ((pixel, from), to) in self
            .glass_current
            .iter_mut()
            .zip(&self.glass_from)
            .zip(&self.glass_target)
        {
            *pixel = std::array::from_fn(|channel| egui::lerp(from[channel]..=to[channel], t));
        }
        if self.glow.is_none() || t < 1.0 || sample_is_new(now, self.began) {
            let glow = image(&self.current);
            let glass = image(&self.glass_current);
            if let Some(texture) = &mut self.glow {
                texture.set(glow, egui::TextureOptions::LINEAR);
            } else {
                self.glow = Some(ui.ctx().load_texture(
                    "PO0 light",
                    glow,
                    egui::TextureOptions::LINEAR,
                ));
            }
            if let Some(texture) = &mut self.glass {
                texture.set(glass, egui::TextureOptions::LINEAR);
            } else {
                self.glass = Some(ui.ctx().load_texture(
                    "PO0 glass light",
                    glass,
                    egui::TextureOptions::LINEAR,
                ));
            }
            let mut mean = [0.0; 3];
            for rgb in self.current[..WIDTH]
                .iter()
                .chain((0..HEIGHT).map(|y| &self.current[y * WIDTH + WIDTH - 1]))
            {
                for c in 0..3 {
                    mean[c] += rgb[c] / (WIDTH + HEIGHT) as f32;
                }
            }
            self.lit = saturated(mean);
        }
        if t < 1.0 {
            crate::motion::light_repaint(ui.ctx());
        }
    }
    pub(crate) fn paint_glow(
        &self,
        painter: &egui::Painter,
        room: Rect,
        picture: Rect,
        glass: bool,
    ) {
        let texture = if glass { &self.glass } else { &self.glow };
        let Some(texture) = texture else {
            return;
        };
        let mut mesh = egui::Mesh::with_texture(texture.id());
        let columns = 24_u32;
        for y in 0..=columns {
            for x in 0..=columns {
                let pos = room.min
                    + room.size()
                        * egui::vec2(x as f32 / columns as f32, y as f32 / columns as f32);
                let uv = (pos - picture.center()) / (picture.size() * 1.5) + egui::vec2(0.5, 0.5);
                let distance =
                    ((pos - picture.center()) / (picture.size() * egui::vec2(1.05, 1.2))).length();
                let falloff = if distance < 0.28 {
                    1.0
                } else if distance < 0.6 {
                    egui::remap(distance, 0.28..=0.6, 1.0..=0.4)
                } else {
                    egui::remap_clamp(distance, 0.6..=1.0, 0.4..=0.0)
                };
                mesh.vertices.push(egui::epaint::Vertex {
                    pos,
                    uv: Pos2::new(uv.x, uv.y),
                    color: Color32::from_white_alpha((falloff * 235.0) as u8),
                });
                if x < columns && y < columns {
                    let i = y * (columns + 1) + x;
                    mesh.indices.extend_from_slice(&[
                        i,
                        i + 1,
                        i + columns + 1,
                        i + 1,
                        i + columns + 2,
                        i + columns + 1,
                    ]);
                }
            }
        }
        painter.add(mesh);
    }
    pub(crate) fn paint_grain(&mut self, ui: &egui::Ui, surfaces: &[Rect]) {
        let grain = self.grain.get_or_insert_with(|| {
            let tile = grain_tile();
            let bytes: Vec<_> = tile.pixels.iter().flat_map(Color32::to_array).collect();
            crate::grain::Grain::new(&self.renderer, &bytes)
        });
        for rect in surfaces {
            if rect.is_positive() {
                grain.paint(ui.painter(), *rect);
            }
        }
    }

    pub(crate) fn bytes(&self) -> u64 {
        u64::from(self.glow.is_some()) * 2304
            + u64::from(self.glass.is_some()) * 2304
            + u64::from(self.grain.is_some()) * 65536
    }
}
fn sample_is_new(now: f64, began: f64) -> bool {
    now.to_bits() == began.to_bits()
}
fn grain_tile() -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(128 * 128);
    let mut random = 0x738a_512b_u32;
    for _ in 0..128 * 64 {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        let value = (random % 256) as u8;
        pixels.push(Color32::from_gray(value));
        pixels.push(Color32::from_gray(255 - value));
    }
    egui::ColorImage::new([128, 128], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blur_preserves_a_constant_frame() {
        let output = blur(&vec![[0.5; 3]; WIDTH * HEIGHT], 3);
        assert!(output.iter().flatten().all(|v| (*v - 0.5).abs() < 0.00001));
    }
    #[test]
    fn grain_has_balanced_signed_deviations() {
        let tile = grain_tile();
        assert_eq!(tile.pixels.len() * 4, 65536);
        for pair in tile.pixels.as_chunks::<2>().0 {
            assert_eq!(pair[0].a(), pair[1].a());
            assert_eq!(u16::from(pair[0].r()) + u16::from(pair[1].r()), 255);
        }
    }
}

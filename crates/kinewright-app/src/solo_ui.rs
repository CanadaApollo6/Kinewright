//! MO2 R26: the person's `preview_solo` — the header's Solo buttons run the
//! agent capability itself on a worker and show its strip and report.

use std::sync::Arc;

use crossbeam_channel::Receiver;
use eframe::egui;
use kinewright_agent::{SoloArgs, SoloContext, SoloError, SoloStrip};
use kinewright_core::ClipId;

use crate::{app::KinewrightApp, mixer_ui::record_strip_rect, theme::color};

/// The strip as native-size tiles, each inside the GPU's texture side, so no
/// strip is ever downscaled: `(top-left in strip pixels, texture)`.
type SoloTiles = Vec<(egui::Vec2, egui::TextureHandle)>;
type SoloOutcome = Result<(SoloStrip, SoloTiles), SoloError>;

/// One solo render: pending on the worker, then its strip or typed refusal.
pub(crate) struct SoloDialog {
    pub(crate) clip: ClipId,
    pub(crate) full_res: bool,
    /// R24's choices, sent on Render: samples 2..=16 and context (`None`
    /// defaults by blend).
    pub(crate) samples: u32,
    pub(crate) context: Option<SoloContext>,
    /// Original pixels 1:1 (panned by scrolling) rather than fit to width.
    pub(crate) actual_size: bool,
    pending: Receiver<Result<SoloStrip, SoloError>>,
    outcome: Option<SoloOutcome>,
}

impl KinewrightApp {
    /// Render `clip` solo against the focused session's current snapshot.
    pub(crate) fn open_solo_dialog(&mut self, clip: ClipId, full_res: bool) {
        self.solo_dialog = Some(SoloDialog {
            clip,
            full_res,
            samples: 8,
            context: None,
            actual_size: full_res,
            pending: crossbeam_channel::never(),
            outcome: None,
        });
        self.render_solo();
    }

    /// (Re)run `preview_solo` with the dialog's arguments on a worker.
    fn render_solo(&mut self) {
        let analysis = Arc::clone(&self.analysis);
        let document = Arc::clone(&self.focused().document);
        let revision = self.focused().revision;
        let Some(dialog) = self.solo_dialog.as_mut() else {
            return;
        };
        let args = SoloArgs {
            expected_revision: revision,
            clip_id: dialog.clip,
            samples: dialog.samples,
            context: dialog.context,
            full_res: dialog.full_res,
        };
        let (sender, pending) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let strip = kinewright_agent::preview_solo(&*analysis, revision, &document, &args);
            let _ = sender.send(strip);
        });
        (dialog.pending, dialog.outcome) = (pending, None);
    }

    pub(crate) fn show_solo_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.solo_dialog.as_mut() else {
            return;
        };
        if dialog.outcome.is_none()
            && let Ok(result) = dialog.pending.try_recv()
        {
            dialog.outcome = Some(result.map(|strip| {
                let tiles = strip_tiles(ctx, &strip.image);
                (strip, tiles)
            }));
        }
        let (mut open, mut render) = (true, false);
        let mode = if dialog.full_res { " (full-res)" } else { "" };
        egui::Window::new(format!("Solo clip {}{mode}", dialog.clip))
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let contexts = [
                        (None, "By blend"),
                        (Some(SoloContext::Isolated), "Isolated"),
                        (Some(SoloContext::Below), "Over below-stack"),
                    ];
                    let current = contexts.iter().find(|(value, _)| *value == dialog.context);
                    let combo = egui::ComboBox::from_id_salt("solo_context")
                        .selected_text(current.map_or("", |(_, label)| *label))
                        .show_ui(ui, |ui| {
                            for (value, label) in contexts {
                                let item = ui.selectable_value(&mut dialog.context, value, label);
                                record_strip_rect(label, item.rect);
                            }
                        });
                    record_strip_rect("solo_context", combo.response.rect);
                    let samples = egui::DragValue::new(&mut dialog.samples)
                        .range(2..=16)
                        .prefix("samples ");
                    let samples = ui.add_enabled(!dialog.full_res, samples);
                    record_strip_rect("solo_samples", samples.rect);
                    let button = ui.button("Render");
                    record_strip_rect("solo_render", button.rect);
                    render = button.clicked();
                    ui.checkbox(&mut dialog.actual_size, "1:1 (scroll to pan)");
                });
                show_solo_outcome(ui, dialog);
            });
        if !open {
            self.solo_dialog = None;
        } else if render {
            self.render_solo();
        }
    }
}

fn show_solo_outcome(ui: &mut egui::Ui, dialog: &SoloDialog) {
    match &dialog.outcome {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Rendering the solo strip…");
            });
            ui.ctx().request_repaint();
        }
        Some(Ok((strip, tiles))) => {
            let report = &strip.report;
            let provenance = &report["provenance"];
            ui.label(format!(
                "{} · {} of {} samples · {} ({})",
                report["context"].as_str().unwrap_or_default(),
                report["emitted"],
                report["requested"],
                provenance["adapter"].as_str().unwrap_or_default(),
                provenance["backend"].as_str().unwrap_or_default(),
            ));
            if report["pairs"] == true {
                ui.colored_label(color::TEXT_MUTED, "Top: BEFORE · bottom: AFTER");
            }
            if let Some(degraded) = report["degraded"].as_str() {
                ui.colored_label(color::STATUS_WARNING, format!("degraded: {degraded}"));
            }
            let inactive: Vec<_> = (report["samples"].as_array().into_iter().flatten())
                .filter(|sample| sample["active"] == false)
                .map(|sample| sample["frame"].to_string())
                .collect();
            if !inactive.is_empty() {
                let frames = inactive.join(", ");
                ui.colored_label(color::TEXT_MUTED, format!("disabled at {frames}"));
            }
            #[allow(clippy::cast_precision_loss)]
            let size = egui::vec2(strip.image.width as f32, strip.image.height as f32);
            let native = 1.0 / ui.ctx().pixels_per_point();
            let fit = (ui.available_width() / size.x).min(native);
            let scale = if dialog.actual_size { native } else { fit };
            if scale < native {
                let shown = (scale / native * 100.0).round();
                let notice = format!("shown at {shown}% — tick 1:1 for original pixels");
                ui.colored_label(color::STATUS_WARNING, notice);
            }
            egui::ScrollArea::both().max_height(720.0).show(ui, |ui| {
                let (area, _) = ui.allocate_exact_size(size * scale, egui::Sense::hover());
                for (at, texture) in tiles {
                    let size = texture.size_vec2() * scale;
                    let rect = egui::Rect::from_min_size(area.min + *at * scale, size);
                    egui::Image::new(texture).paint_at(ui, rect);
                }
            });
        }
        Some(Err(error)) => {
            ui.colored_label(color::STATUS_DANGER, format!("{}: {error}", error.code()));
        }
    }
}

/// Cut the strip into native-size tiles no larger than the texture side.
#[allow(clippy::cast_precision_loss)]
fn strip_tiles(ctx: &egui::Context, strip: &kinewright_core::RgbaImage) -> SoloTiles {
    let limit = ctx.input(|input| input.max_texture_side).max(1);
    let (width, height) = (strip.width as usize, strip.height as usize);
    let mut tiles = Vec::new();
    for (y, x) in (0..height)
        .step_by(limit)
        .flat_map(|y| (0..width).step_by(limit).map(move |x| (y, x)))
    {
        let size = [limit.min(width - x), limit.min(height - y)];
        let pixels: Vec<u8> = (y..y + size[1])
            .flat_map(|row| &strip.pixels[(row * width + x) * 4..(row * width + x + size[0]) * 4])
            .copied()
            .collect();
        let image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
        let texture = ctx.load_texture(
            format!("solo-strip-{x}-{y}"),
            image,
            egui::TextureOptions::NEAREST,
        );
        tiles.push((egui::vec2(x as f32, y as f32), texture));
    }
    tiles
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::{Duration, Instant};

    use eframe::egui;
    use kinewright_core::Analysis;
    use kinewright_media::{
        in1_sources::{In1Source, in1_source},
        test_support::single_clip_document,
    };

    use super::{SoloStrip, SoloTiles};
    use crate::app::KinewrightApp;

    /// A headless egui surface over the solo dialog that keeps every texture
    /// upload, so a test can read back exactly what the GUI holds.
    pub(crate) struct SoloView {
        ctx: egui::Context,
        time: f64,
        pub(crate) uploads: Vec<(egui::TextureId, egui::epaint::ImageDelta)>,
    }

    impl SoloView {
        pub(crate) fn new() -> Self {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let (time, uploads) = (0.0, Vec::new());
            Self { ctx, time, uploads }
        }

        pub(crate) fn frame(
            &mut self,
            app: &mut KinewrightApp,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            self.time += 0.02;
            let _ = crate::mixer_ui::take_strip_rects();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let output = self.ctx.run_ui(input, |ui| app.show_solo_dialog(ui.ctx()));
            self.uploads
                .extend(output.textures_delta.set.iter().cloned());
            output
        }

        /// Frame until the worker answers (or panic on a hang), then once more.
        pub(crate) fn settle(&mut self, app: &mut KinewrightApp) -> egui::FullOutput {
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                self.frame(app, Vec::new());
                if (app.solo_dialog.as_ref()).is_some_and(|dialog| dialog.outcome.is_some()) {
                    return self.frame(app, Vec::new());
                }
                assert!(
                    Instant::now() < deadline,
                    "the solo worker answered nothing"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Press and release the widget recorded as `name` last frame.
        pub(crate) fn click(&mut self, app: &mut KinewrightApp, name: &str) {
            self.frame(app, Vec::new());
            let rects = crate::mixer_ui::take_strip_rects();
            let at = (rects.iter().find(|(id, _)| id == name))
                .unwrap_or_else(|| panic!("{name} is drawn: {rects:?}"))
                .1
                .center();
            for pressed in [true, false] {
                let button = egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                self.frame(app, vec![egui::Event::PointerMoved(at), button]);
            }
        }
    }

    impl super::SoloDialog {
        /// The rendered strip, once the worker answered with one.
        pub(crate) fn strip(&self) -> Option<&SoloStrip> {
            self.outcome.as_ref()?.as_ref().ok().map(|(strip, _)| strip)
        }
    }

    /// Frame the solo dialog until its worker answers, then return one
    /// settled frame's painted text.
    pub(crate) fn settle_solo_dialog(app: &mut KinewrightApp) -> Vec<String> {
        crate::theme::painted_text(&SoloView::new().settle(app))
    }

    /// The strip reassembled from the tiles' uploaded texture pixels.
    fn uploaded_strip(view: &SoloView, strip: &SoloStrip, tiles: &SoloTiles) -> Vec<u8> {
        let width = strip.image.width as usize;
        let mut canvas = vec![0; strip.image.pixels.len()];
        for (at, texture) in tiles {
            let upload = (view.uploads.iter().rev())
                .find(|(id, _)| *id == texture.id())
                .expect("every tile is uploaded");
            let egui::ImageData::Color(image) = &upload.1.image;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (left, top) = (at.x as usize, at.y as usize);
            for (index, pixel) in image.pixels.iter().enumerate() {
                let (x, y) = (left + index % image.size[0], top + index / image.size[0]);
                canvas[(y * width + x) * 4..][..4].copy_from_slice(&pixel.to_array());
            }
        }
        canvas
    }

    /// R26: the dialog renders a tagged media clip solo through the agent's
    /// `preview_solo` — strip texture, report line, no refusal.
    #[test]
    fn mo2_the_solo_dialog_shows_the_strip_and_its_report() {
        let engine = kinewright_media::FfmpegMediaEngine::new().expect("the engine starts");
        let fixture = in1_source(In1Source::TaggedMp4);
        let asset = engine.probe(fixture.path()).expect("the fixture probes");
        let (mut app, _engine) = crate::app::in1_tests::in1_harness(single_clip_document(asset));
        let clip = app.focused().document.tracks[0].clips[0].id;
        app.open_solo_dialog(clip, false);
        let mut view = SoloView::new();
        let painted = crate::theme::painted_text(&view.settle(&mut app));
        let dialog = app.solo_dialog.as_ref().expect("still open");
        let Some(Ok((strip, tiles))) = &dialog.outcome else {
            panic!("a strip, not a refusal: {painted:?}");
        };
        assert_eq!(strip.report["context"], "isolated");
        assert_eq!(strip.report["pairs"], false);
        assert!(
            strip.image.width > 2_048,
            "the default strip outgrows egui's default texture side"
        );
        assert!(tiles.len() > 1, "tiled, never downscaled");
        assert!(tiles.iter().all(|(_, texture)| texture.size()[0] <= 2_048));
        let held = uploaded_strip(&view, strip, tiles) == strip.image.pixels;
        assert!(held, "the GUI holds every strip pixel");
        assert!(
            painted.iter().any(|text| text.starts_with("shown at")),
            "fitting to width says so: {painted:?}"
        );
        let emitted = &strip.report["emitted"];
        assert!(
            painted
                .iter()
                .any(|text| text.starts_with(&format!("isolated · {emitted} of 8 samples"))),
            "the report line is shown: {painted:?}"
        );
        crate::app::in1_tests::in1_shutdown(&mut app);
    }

    // Injected inside solo_ui::tests; experiments only.
    #[test]
    fn reviewer2_mo2_full_res_dialog_keeps_inspectable_pixels() {
        use kinewright_core::{
            AssetId, BlendMode, Clip, ClipContent, ClipId, Document, SolidColor, TimeCode, Track,
            TrackId, TrackKind,
        };
        let clip = Clip {
            id: ClipId(1),
            asset: AssetId(0),
            content: ClipContent::Solid(SolidColor {
                r: 128,
                g: 64,
                b: 32,
            }),
            timeline_start: TimeCode(0),
            source_range: TimeCode(0)..TimeCode(60),
            effects: vec![],
            transition_in: None,
            link: None,
            enabled: true,
            enabled_curve: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode(0),
            audio_fade_out_frames: TimeCode(0),
            speed_percent: 100,
            audio_gain_curve: None,
            blend_mode: BlendMode::Screen,
        };
        let document = Document {
            resolution: (3840, 2160),
            duration: TimeCode(60),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![clip],
            }],
            ..Document::default()
        };
        let (mut app, _engine) = crate::app::in1_tests::in1_harness(document);
        app.open_solo_dialog(ClipId(1), true);
        let mut view = SoloView::new();
        let output = view.settle(&mut app);
        let painted = crate::theme::painted_text(&output);
        let dialog = app.solo_dialog.as_ref().unwrap();
        let Some(Ok((strip, tiles))) = &dialog.outcome else {
            panic!("full-res strip should render: {painted:?}")
        };
        assert_eq!(
            strip.report["full_res"], true,
            "request must reach capability"
        );
        assert_eq!(
            strip.report["context"], "below",
            "non-normal default must reach capability"
        );
        assert_eq!((strip.image.width, strip.image.height), (3840, 2160));
        // Fix round 1: the GUI holds every original pixel, in tiles inside
        // the texture side, and full-res opens at 1:1 — each tile painted at
        // its native size, with no scaling notice.
        assert_eq!(tiles.len(), 4, "2×2 tiles of at most 2048");
        assert!(
            uploaded_strip(&view, strip, tiles) == strip.image.pixels,
            "full-res detail survives into the GUI"
        );
        for (_, texture) in tiles {
            let painted_size = (output.shapes.iter()).find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Rect(rect) if rect.fill_texture_id() == texture.id() => {
                    Some(rect.rect.size())
                }
                _ => None,
            });
            assert_eq!(painted_size, Some(texture.size_vec2()), "1:1");
        }
        assert!(!painted.iter().any(|text| text.starts_with("shown at")));
        app.solo_dialog.as_mut().unwrap().actual_size = false;
        let painted = crate::theme::painted_text(&view.frame(&mut app, Vec::new()));
        crate::app::in1_tests::in1_shutdown(&mut app);
        assert!(
            painted.iter().any(|text| text.starts_with("shown at")),
            "a fitted view says it is scaled: {painted:?}"
        );
    }

    #[test]
    fn reviewer2_mo2_solo_requests_preserve_mode_context_pairs_and_pixels() {
        use kinewright_core::{
            AssetId, BlendMode, Clip, ClipContent, ClipId, Document, SolidColor, TimeCode, Track,
            TrackId, TrackKind,
        };
        for (adjustment, blend, full_res, context) in [
            (false, BlendMode::Normal, false, "isolated"),
            (false, BlendMode::Screen, false, "below"),
            (false, BlendMode::Normal, true, "isolated"),
            (true, BlendMode::Normal, true, "below"),
        ] {
            let clip = Clip {
                id: ClipId(1),
                asset: AssetId(0),
                content: if adjustment {
                    ClipContent::Adjustment
                } else {
                    ClipContent::Solid(SolidColor {
                        r: 128,
                        g: 64,
                        b: 32,
                    })
                },
                timeline_start: TimeCode(0),
                source_range: TimeCode(0)..TimeCode(60),
                effects: vec![],
                transition_in: None,
                link: None,
                enabled: true,
                enabled_curve: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode(0),
                audio_fade_out_frames: TimeCode(0),
                speed_percent: 100,
                audio_gain_curve: None,
                blend_mode: blend,
            };
            let document = Document {
                resolution: (320, 180),
                duration: TimeCode(60),
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![clip],
                }],
                ..Document::default()
            };
            let (mut app, _engine) = crate::app::in1_tests::in1_harness(document);
            app.open_solo_dialog(ClipId(1), full_res);
            let painted = settle_solo_dialog(&mut app);
            let dialog = app.solo_dialog.as_ref().unwrap();
            let Some(Ok((strip, _))) = &dialog.outcome else {
                panic!("strip should render: {painted:?}")
            };
            assert_eq!(strip.report["full_res"], full_res);
            assert_eq!(strip.report["context"], context);
            assert_eq!(strip.report["pairs"], adjustment);
            assert_eq!(strip.report["emitted"], if full_res { 1 } else { 8 });
            let args = kinewright_agent::SoloArgs {
                expected_revision: app.focused().revision,
                clip_id: ClipId(1),
                samples: 8,
                context: None,
                full_res,
            };
            let agent = kinewright_agent::preview_solo(
                &*app.analysis,
                app.focused().revision,
                &app.focused().document,
                &args,
            )
            .unwrap();
            assert_eq!(
                strip.png, agent.png,
                "GUI uses identical pixels to direct agent capability"
            );
            crate::app::in1_tests::in1_shutdown(&mut app);
        }
    }

    /// Fix round 1 (review 2 S2): the dialog sends R24's choices — context
    /// and sample count — and renders exactly what the agent gets for them.
    #[test]
    fn mo2_the_solo_dialog_sends_context_and_sample_choices() {
        use kinewright_core::{
            AssetId, BlendMode, Clip, ClipContent, ClipId, Document, SolidColor, TimeCode, Track,
            TrackId, TrackKind,
        };
        let solid = |id, [r, g, b]: [u8; 3]| Clip {
            id: ClipId(id),
            asset: AssetId(0),
            content: ClipContent::Solid(SolidColor { r, g, b }),
            timeline_start: TimeCode(0),
            source_range: TimeCode(0)..TimeCode(60),
            effects: vec![],
            transition_in: None,
            link: None,
            enabled: true,
            enabled_curve: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode(0),
            audio_fade_out_frames: TimeCode(0),
            speed_percent: 100,
            audio_gain_curve: None,
            blend_mode: BlendMode::Normal,
        };
        let track = |id, clip| Track {
            id: TrackId(id),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![clip],
        };
        let document = Document {
            resolution: (64, 36),
            duration: TimeCode(60),
            tracks: vec![
                track(1, solid(1, [20, 90, 160])),
                track(2, solid(2, [200, 60, 30])),
            ],
            ..Document::default()
        };
        let (mut app, _engine) = crate::app::in1_tests::in1_harness(document);
        app.open_solo_dialog(ClipId(2), false);
        let mut view = SoloView::new();
        view.settle(&mut app);
        view.click(&mut app, "solo_context");
        view.click(&mut app, "Over below-stack");
        view.click(&mut app, "solo_samples");
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        view.frame(&mut app, vec![egui::Event::Text("4".into()), enter]);
        view.click(&mut app, "solo_render");
        view.settle(&mut app);
        let dialog = app.solo_dialog.as_ref().unwrap();
        let Some(Ok((strip, _))) = &dialog.outcome else {
            panic!("a strip, not a refusal");
        };
        assert_eq!(strip.report["context"], "below", "{}", strip.report);
        assert_eq!(strip.report["requested"], 4);
        let args = kinewright_agent::SoloArgs {
            expected_revision: app.focused().revision,
            clip_id: ClipId(2),
            samples: 4,
            context: Some(kinewright_agent::SoloContext::Below),
            full_res: false,
        };
        let revision = app.focused().revision;
        let document = std::sync::Arc::clone(&app.focused().document);
        let agent = kinewright_agent::preview_solo(&*app.analysis, revision, &document, &args);
        assert!(strip.png == agent.unwrap().png, "the agent's exact strip");
        crate::app::in1_tests::in1_shutdown(&mut app);
    }
    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn new_mo2_fullres_far_corner_and_all_tile_pixels() {
        let mut pixels = Vec::with_capacity(3840 * 2160 * 4);
        for y in 0..2160 {
            for x in 0..3840 {
                pixels.extend_from_slice(&[
                    (x % 251) as u8,
                    (y % 241) as u8,
                    ((x + y) % 239) as u8,
                    255,
                ]);
            }
        }
        let image = kinewright_core::RgbaImage {
            width: 3840,
            height: 2160,
            pixels,
        };
        for limit in [2048, 1024, 1537] {
            let mut view = SoloView::new();
            let mut tiles = vec![];
            let output = view.ctx.run_ui(
                egui::RawInput {
                    max_texture_side: Some(limit),
                    ..Default::default()
                },
                |ui| {
                    tiles = super::strip_tiles(ui.ctx(), &image);
                },
            );
            view.uploads.extend(output.textures_delta.set);
            let strip = SoloStrip {
                image: image.clone(),
                png: vec![],
                report: serde_json::json!({}),
            };
            let actual = uploaded_strip(&view, &strip, &tiles);
            let far = ((2159 * 3840 + 3839) * 4) as usize;
            assert_eq!(
                &actual[far..far + 4],
                &image.pixels[far..far + 4],
                "far corner limit={limit}"
            );
            assert_eq!(actual, image.pixels, "all original pixels limit={limit}");
            for (_, tile) in &tiles {
                assert!(tile.size().iter().all(|n| *n <= limit));
            }
        }
    }
    #[test]
    fn new_mo2_solo_control_argument_matrix() {
        use kinewright_core::{
            AssetId, BlendMode, Clip, ClipContent, ClipId, Document, SolidColor, TimeCode, Track,
            TrackId, TrackKind,
        };
        let solid = |id, [r, g, b]: [u8; 3]| Clip {
            id: ClipId(id),
            asset: AssetId(0),
            content: ClipContent::Solid(SolidColor { r, g, b }),
            timeline_start: TimeCode(0),
            source_range: TimeCode(0)..TimeCode(60),
            effects: vec![],
            transition_in: None,
            link: None,
            enabled: true,
            enabled_curve: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode(0),
            audio_fade_out_frames: TimeCode(0),
            speed_percent: 100,
            audio_gain_curve: None,
            blend_mode: BlendMode::Normal,
        };
        let track = |id, clip| Track {
            id: TrackId(id),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![clip],
        };
        let document = Document {
            resolution: (64, 36),
            duration: TimeCode(60),
            tracks: vec![
                track(1, solid(1, [20, 90, 160])),
                track(2, solid(2, [200, 60, 30])),
            ],
            ..Document::default()
        };
        let (mut app, _engine) = crate::app::in1_tests::in1_harness(document);
        app.open_solo_dialog(ClipId(2), false);
        let mut view = SoloView::new();
        view.settle(&mut app);
        let before = app.focused().document.clone();
        let revision = app.focused().revision;
        for (label, context) in [
            ("By blend", None),
            ("Isolated", Some(kinewright_agent::SoloContext::Isolated)),
            (
                "Over below-stack",
                Some(kinewright_agent::SoloContext::Below),
            ),
        ] {
            for count in [2, 16] {
                view.click(&mut app, "solo_context");
                view.click(&mut app, label);
                view.click(&mut app, "solo_samples");
                let enter = egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                };
                view.frame(&mut app, vec![egui::Event::Text(count.to_string()), enter]);
                view.click(&mut app, "solo_render");
                view.settle(&mut app);
                let dialog = app.solo_dialog.as_ref().unwrap();
                let strip = dialog.strip().unwrap();
                let args = kinewright_agent::SoloArgs {
                    expected_revision: revision,
                    clip_id: ClipId(2),
                    samples: count,
                    context,
                    full_res: false,
                };
                let agent =
                    kinewright_agent::preview_solo(&*app.analysis, revision, &before, &args)
                        .unwrap();
                assert_eq!(strip.report["requested"], count);
                assert_eq!(strip.report["context"], agent.report["context"]);
                assert_eq!(strip.png, agent.png, "{label} {count}");
                assert_eq!(dialog.context, context);
                assert_eq!(dialog.samples, count);
                assert_eq!(app.focused().revision, revision);
                assert_eq!(*app.focused().document, *before);
            }
        }
        crate::app::in1_tests::in1_shutdown(&mut app);
    }
}

//! The ten PO0 scenes. This replaces the layout only for an explicit probe launch.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
use crate::{
    app::KinewrightApp,
    light::Light,
    motion::{self, AnimatedList, HeldFrame, Token},
    performance_gpu::Timings,
};
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use kinewright_core::{Analysis, Document, TimeCode};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

static SURFACE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
pub(crate) fn surface_caps(
    adapter: &eframe::wgpu::Adapter,
    surface: Option<&eframe::wgpu::Surface<'_>>,
) {
    if let Some(surface) = surface {
        let caps = surface.get_capabilities(adapter);
        let _=SURFACE.set(serde_json::json!({"alpha_modes":format!("{:?}",caps.alpha_modes),"formats":format!("{:?}",caps.formats)}));
    }
}
pub(crate) fn enabled() -> bool {
    std::env::var("KINEWRIGHT_MOTION_PROBE").is_ok_and(|value| value == "1")
}

pub(crate) fn document(
    media: &kinewright_media::FfmpegMediaEngine,
    fallback: Document,
) -> Document {
    if !enabled() {
        return fallback;
    }
    let path =
        std::env::var_os("KINEWRIGHT_PROBE_MEDIA").expect("probe requires KINEWRIGHT_PROBE_MEDIA");
    let mut asset = media
        .probe(Path::new(&path))
        .expect("probe source must be readable");
    asset.id = kinewright_core::AssetId(1);
    assert_eq!(
        asset.resolution,
        Some((1920, 1080)),
        "probe requires 1080p media"
    );
    assert_eq!(
        asset.fps,
        kinewright_core::Rational::new(30, 1).expect("fps")
    );
    let duration = asset.duration;
    let clip = kinewright_core::Clip {
        id: kinewright_core::ClipId(1),
        asset: asset.id,
        enabled: true,
        enabled_curve: None,
        source_range: TimeCode::ZERO..duration,
        timeline_start: TimeCode::ZERO,
        content: kinewright_core::ClipContent::Media,
        effects: vec![kinewright_core::Effect {
            id: kinewright_core::EffectId(1),
            name: "reframe".into(),
            parameters: std::collections::BTreeMap::from([
                (
                    "target_aspect_basis_points".into(),
                    kinewright_core::ParamValue::Integer(17_778),
                ),
                (
                    "focus_x_percent".into(),
                    kinewright_core::ParamValue::Integer(50),
                ),
                (
                    "focus_y_percent".into(),
                    kinewright_core::ParamValue::Integer(50),
                ),
            ]),
            keyframes: std::collections::BTreeMap::default(),
            enabled: true,
            enabled_curve: None,
        }],
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
        blend_mode: kinewright_core::BlendMode::Normal,
    };
    let mut audio = clip.clone();
    audio.id = kinewright_core::ClipId(2);
    audio.effects.clear();
    Document {
        tracks: vec![
            kinewright_core::Track {
                id: kinewright_core::TrackId(1),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: true,
                clips: vec![clip],
            },
            kinewright_core::Track {
                id: kinewright_core::TrackId(2),
                kind: kinewright_core::TrackKind::Audio,
                sync_lock: true,
                clips: vec![audio],
            },
        ],
        media_pool: vec![asset],
        fps: kinewright_core::Rational::new(30, 1).expect("fps"),
        resolution: (1920, 1080),
        duration,
        ..Default::default()
    }
}

struct Tile {
    index: usize,
    image: kinewright_core::RgbaImage,
    ms: f64,
}
struct PlaybackWindow {
    began: Instant,
    stats: kinewright_core::PlaybackStats,
}
#[derive(Clone)]
struct FillPaint {
    began: Instant,
    painted: Arc<std::sync::Mutex<Option<Instant>>>,
}
impl eframe::egui_wgpu::CallbackTrait for FillPaint {
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        _: &mut eframe::wgpu::RenderPass<'static>,
        _: &eframe::egui_wgpu::CallbackResources,
    ) {
        if let Ok(mut painted) = self.painted.try_lock() {
            painted.get_or_insert_with(Instant::now);
        }
    }
}
struct Strip {
    incoming: crossbeam_channel::Receiver<Result<Tile, String>>,
    atlas: egui::TextureHandle,
    grey: egui::TextureHandle,
    filled: usize,
    began: Instant,
}
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent probe settings and launch state"
)]
pub(crate) struct Probe {
    moment: u8,
    playback: bool,
    effects: bool,
    repeats: u32,
    pub(crate) completed: u32,
    started: Instant,
    trial_at: f64,
    next_at: f64,
    trial: u32,
    light: Light,
    timings: Option<Timings>,
    list: AnimatedList,
    at_bottom: bool,
    neutral: bool,
    reduce: bool,
    initialized: bool,
    held: Option<HeldFrame>,
    pending_epoch: Option<u64>,
    dissolve_at: Option<f64>,
    current_document: Option<Arc<Document>>,
    versions: Option<[Arc<Document>; 2]>,
    strip: Option<Strip>,
    version_strips: [Option<Strip>; 2],
    prewarm_finished: bool,
    tile_ms: Vec<f64>,
    fill_ms: Vec<f64>,
    intervals: Vec<f64>,
    cpu_ms: Vec<f64>,
    last_frame: Option<Instant>,
    repaint_windows: Vec<f64>,
    last_motion_at: f64,
    post_settle_repaints: u32,
    settled_trial: u32,
    gpu_adapter: String,
    failures: Vec<String>,
    pin: Vec<(i64, [f32; 2])>,
    pin_rx: Option<crossbeam_channel::Receiver<Result<serde_json::Value, String>>>,
    process_idle: Option<f64>,
    process_animated: Option<f64>,
    idle_cpu_start: Option<(Instant, f64)>,
    animation_cpu_start: Option<(Instant, f64)>,
    held_peak: u64,
    dissolve_delays: Vec<f64>,
    first_paint_epoch: Option<u64>,
    deadline_snap: bool,
    snap_residuals: Vec<[f32; 2]>,
    capture_dir: Option<std::path::PathBuf>,
    captures: u32,
    max_position: i64,
    progressive: Option<(usize, egui::TextureHandle)>,
    playback_window: Option<PlaybackWindow>,
    window_closing: Option<u64>,
    fill_pending_paint: Option<FillPaint>,
    window_due: u64,
    window_bad: u64,
    window_present_ms: Vec<f64>,
    last_paint: Option<Instant>,
    viewport_pixels: [f32; 2],
    interrupted_trial: u32,
    completions: Vec<motion::Completion>,
}
impl Probe {
    pub(crate) fn from_environment(state: &eframe::egui_wgpu::RenderState) -> Option<Self> {
        if !enabled() {
            return None;
        }
        let moment = std::env::var("KINEWRIGHT_PROBE_MOMENT")
            .unwrap_or_else(|_| "6".into())
            .parse()
            .expect("moment 1..10");
        assert!((1..=10).contains(&moment));
        let effects = std::env::var("KINEWRIGHT_PROBE_EFFECTS").map_or(true, |s| s != "0");
        Some(Self {
            moment,
            playback: std::env::var("KINEWRIGHT_PROBE_PLAYBACK").is_ok_and(|s| s == "1"),
            effects,
            repeats: std::env::var("KINEWRIGHT_PROBE_REPEATS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(200)
                .clamp(1, 200),
            completed: 0,
            started: Instant::now(),
            trial_at: 0.0,
            next_at: 6.0,
            trial: 0,
            light: Light::new(state, effects),
            timings: Timings::new(state),
            list: AnimatedList::new(),
            at_bottom: true,
            neutral: false,
            reduce: false,
            initialized: false,
            held: None,
            pending_epoch: None,
            dissolve_at: None,
            current_document: None,
            versions: None,
            strip: None,
            version_strips: [None, None],
            prewarm_finished: moment != 3,
            tile_ms: Vec::new(),
            fill_ms: Vec::new(),
            intervals: Vec::new(),
            cpu_ms: Vec::new(),
            last_frame: None,
            repaint_windows: Vec::new(),
            last_motion_at: 0.0,
            post_settle_repaints: 0,
            settled_trial: 0,
            gpu_adapter: format!("{:?}", state.adapter.get_info()),
            failures: Vec::new(),
            pin: Vec::new(),
            pin_rx: None,
            process_idle: None,
            process_animated: None,
            idle_cpu_start: None,
            animation_cpu_start: None,
            held_peak: 0,
            dissolve_delays: Vec::new(),
            first_paint_epoch: None,
            deadline_snap: std::env::var("KINEWRIGHT_PROBE_DEADLINE_SNAP").is_ok_and(|s| s == "1"),
            snap_residuals: Vec::new(),
            capture_dir: std::env::var_os("KINEWRIGHT_PROBE_CAPTURE_DIR").map(Into::into),
            captures: 0,
            max_position: 0,
            progressive: None,
            playback_window: None,
            window_closing: None,
            fill_pending_paint: None,
            window_due: 0,
            window_bad: 0,
            window_present_ms: Vec::new(),
            last_paint: None,
            viewport_pixels: [0.0; 2],
            interrupted_trial: 0,
            completions: Vec::new(),
        })
    }
    fn initialize(&mut self, app: &KinewrightApp) {
        self.initialized = true;
        let a = Arc::clone(&app.focused().document);
        let mut b = (*a).clone();
        b.tracks[0].clips[0].source_range.start = TimeCode(60);
        b.tracks[0].clips[0].source_range.end = b.duration;

        self.current_document = Some(Arc::clone(&a));
        self.versions = Some([a, Arc::new(b)]);
    }
    fn start_trial(&mut self, app: &mut KinewrightApp, ui: &egui::Ui, now: f64) {
        self.trial += 1;
        self.trial_at = now;
        self.next_at = now
            + match self.moment {
                1 | 8 => 0.75,
                10 => 0.85,
                2 => 3.6,
                3 => 0.5,
                5 => 1.65,
                6 => 0.2,
                7 => 0.01,
                _ => 1.0,
            };
        if self.trial == 1 {
            self.process_idle = core_percent(self.idle_cpu_start);
            self.animation_cpu_start = process_cpu().map(|cpu| (Instant::now(), cpu));
        }
        if self.moment == 10 {
            app.playback.seek(TimeCode::ZERO);
        }
        match self.moment {
            3 => self.switch_version(app, now, (self.trial % 2) as usize),
            4 | 9 => {
                self.at_bottom = self.moment == 4 || (self.trial / 20).is_multiple_of(2);
                self.list.insert(40 + u64::from(self.trial), self.at_bottom);
            }
            7 => self.start_strip(app, ui, None),
            10 if self.trial == 1 => {
                let (tx, rx) = crossbeam_channel::bounded(1);
                self.pin_rx = Some(rx);
                let core = app.focused().core.clone();
                let playback = Arc::clone(&app.playback);
                let analysis = Arc::clone(&app.analysis);
                app.off_ui(
                    ui.ctx(),
                    move || {
                        let result =
                            kinewright_agent::po0_probe::track_pin(core, playback, analysis);
                        let _ = tx.try_send(result);
                    },
                    |_, _, ()| {},
                );
            }
            _ => {}
        }
    }
    fn switch_version(&mut self, app: &mut KinewrightApp, now: f64, version: usize) {
        self.finish_window(app.playback.stats());
        if let Some(display) = &app.native_display
            && let Some((view, _, _)) = display.session.bound_image()
        {
            let old = self.held.take();
            let held = if let Some(old) = &old {
                let t = self.dissolve_at.map_or(0.0, |start| {
                    motion::ease_in_out(
                        ((now - start) / f64::from(Token::Standard.seconds())).clamp(0.0, 1.0)
                            as f32,
                    )
                });
                self.held_peak = self.held_peak.max(old.bytes() * 2);
                HeldFrame::blend(&display.renderer, old, view, t)
            } else {
                HeldFrame::capture(&display.renderer, view)
            };
            if let Some(old) = old {
                old.release(&display.renderer);
            }
            self.held_peak = self.held_peak.max(held.bytes());
            self.held = Some(held);
        }
        let document = Arc::clone(&self.versions.as_ref().expect("versions")[version]);
        if let Err(error) = app.lut_publisher.po0_switch_version(Arc::clone(&document)) {
            self.worker_failure(error.to_string());
            return;
        }
        if !self.playback {
            app.playback.request_frame(TimeCode::ZERO);
        }
        self.current_document = Some(document);
        self.pending_epoch = Some(app.playback.stamp().epoch);
        self.dissolve_at = None;
    }
    fn start_strip(&mut self, app: &KinewrightApp, ui: &egui::Ui, version: Option<usize>) {
        if version.is_none() {
            self.playback_window = Some(PlaybackWindow {
                began: Instant::now(),
                stats: app.playback.stats(),
            });
        }
        let (tx, rx) = crossbeam_channel::bounded(16);
        let analysis = Arc::clone(&app.analysis);
        let media = Arc::clone(&app.lut_publisher);
        let document = Arc::clone(if let Some(version) = version {
            &self.versions.as_ref().expect("versions")[version]
        } else {
            self.current_document.as_ref().expect("document")
        });
        let atlas = ui.ctx().load_texture(
            "PO0 cold filmstrip",
            egui::ColorImage::filled([144 * 16, 80], Color32::BLACK),
            egui::TextureOptions::LINEAR,
        );
        let grey = ui.ctx().load_texture(
            "PO0 neutral filmstrip",
            egui::ColorImage::filled([144 * 16, 80], Color32::BLACK),
            egui::TextureOptions::LINEAR,
        );
        let strip = Strip {
            grey,
            incoming: rx,
            atlas,
            filled: 0,
            began: Instant::now(),
        };
        if let Some(version) = version {
            self.version_strips[version] = Some(strip);
        } else {
            self.strip = Some(strip);
        }
        app.off_ui(
            ui.ctx(),
            move || {
                if let Err(error) = media.po0_clear_thumbnails() {
                    let _ = tx.try_send(Err(error.to_string()));
                    return;
                }
                for index in 0..16 {
                    let began = Instant::now();
                    let at = TimeCode(
                        document.duration.0 * i64::try_from(index).expect("sixteen tiles") / 16,
                    );
                    let tile = analysis
                        .thumbnail_for_document(Arc::clone(&document), at, 144)
                        .map(|frame| {
                            let image =
                                image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
                                    .expect("thumbnail RGBA dimensions");
                            let image = image::imageops::resize(
                                &image,
                                144,
                                80,
                                image::imageops::FilterType::Triangle,
                            );
                            Tile {
                                index,
                                image: kinewright_core::RgbaImage {
                                    width: 144,
                                    height: 80,
                                    pixels: image.into_raw(),
                                },
                                ms: began.elapsed().as_secs_f64() * 1000.0,
                            }
                        })
                        .map_err(|e| e.to_string());
                    if tx.send(tile).is_err() {
                        break;
                    }
                }
            },
            |_, _, ()| {},
        );
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one gated probe frame collects the measurement boundaries"
    )]
    pub(crate) fn ui(&mut self, app: &mut KinewrightApp, ui: &mut egui::Ui) {
        if !self.initialized {
            self.initialize(app);
            if self.moment == 3 {
                self.next_at = 590.0;
                self.start_strip(app, ui, Some(0));
                self.start_strip(app, ui, Some(1));
            }
            if self.playback {
                app.playing = true;
                app.playback.play(TimeCode::ZERO);
            }
        }
        motion::begin(ui.ctx());
        motion::deadline(ui.ctx(), self.deadline_snap);
        motion::set_reduced(ui.ctx(), self.reduce);
        let now = self.started.elapsed().as_secs_f64();
        if now < 3.0 {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(3.0 - now));
        }
        if now >= 3.0 && self.trial == 0 && self.idle_cpu_start.is_none() {
            self.idle_cpu_start = process_cpu().map(|cpu| (Instant::now(), cpu));
        }
        if now >= self.next_at && self.completed < self.repeats {
            if self.trial > 0 {
                if (self.moment == 7
                    && (self.strip.as_ref().is_some_and(|s| s.filled < 16)
                        || self.playback_window.is_some()))
                    || (self.moment == 3
                        && (self.pending_epoch.is_some()
                            || self.held.is_some()
                            || self.window_closing.is_some()))
                    || (self.moment == 10 && self.pin_rx.is_some())
                {
                    self.next_at = now + 0.05;
                } else {
                    self.completed += 1;
                    self.repaint_windows
                        .push((self.last_motion_at - self.trial_at).max(0.0) * 1000.0);
                }
            }
            if self.completed < self.repeats && now >= self.next_at {
                self.start_trial(app, ui, now);
            }
        }
        if self.completed >= self.repeats && self.process_animated.is_none() {
            self.process_animated = core_percent(self.animation_cpu_start);
        }
        if self.moment == 3
            && self.trial > 0
            && self.trial.is_multiple_of(20)
            && self.interrupted_trial != self.trial
            && self
                .dissolve_at
                .is_some_and(|start| now - start >= 0.09 && now - start < 0.18)
        {
            self.interrupted_trial = self.trial;
            self.switch_version(app, now, ((self.trial + 1) % 2) as usize);
        }
        app.finalize_preview(ui.ctx());
        if let Some(display) = &app.native_display
            && let Some((_, _, stamp)) = display.session.bound_image()
            && self.pending_epoch == Some(stamp.epoch)
        {
            self.pending_epoch = None;
            self.dissolve_at = Some(now);
            self.playback_window = Some(PlaybackWindow {
                began: Instant::now(),
                stats: app.playback.stats(),
            });

            self.first_paint_epoch = Some(ui.ctx().cumulative_frame_nr());
        }
        self.poll_workers(ui);
        if let Some(close) = self.window_closing {
            if ui.ctx().cumulative_frame_nr() >= close {
                self.finish_window(app.playback.stats());
            } else {
                ui.ctx().request_repaint();
            }
        }
        let room = ui.max_rect();
        self.viewport_pixels = [
            room.width() * ui.ctx().pixels_per_point(),
            room.height() * ui.ctx().pixels_per_point(),
        ];
        let target = if self.moment == 1 && self.trial % 2 == 1 {
            0.0
        } else {
            336.0
        };
        let rail_width = motion::animate(ui, egui::Id::new("po0-rail"), target, Token::Panel);
        let margin = 24.0;
        let rail = Rect::from_min_max(
            Pos2::new(room.right() - margin - rail_width, room.top() + 80.0),
            Pos2::new(room.right() - margin, room.bottom() - 176.0),
        );
        let stage = Rect::from_min_max(
            Pos2::new(room.left() + margin, room.top() + 80.0),
            Pos2::new(rail.left() - 24.0, room.bottom() - 304.0),
        );
        let scale = (stage.width() / 1920.0).min(stage.height() / 1080.0);
        let picture =
            egui::Rect::from_center_size(stage.center(), Vec2::new(1920.0, 1080.0) * scale);
        let composer = Rect::from_center_size(
            Pos2::new(stage.center().x, room.bottom() - 202.0),
            Vec2::new(560.0, 52.0),
        );
        let timeline = Rect::from_min_max(
            Pos2::new(room.left() + 24.0, room.bottom() - 136.0),
            Pos2::new(room.right() - 24.0, room.bottom() - 24.0),
        );
        let painter = ui.painter();
        if let Some(timings) = &self.timings {
            timings.boundary(painter, 0);
        }
        painter.rect_filled(room, 0, Color32::from_rgb(11, 11, 13));
        if let Some(timings) = &self.timings {
            timings.boundary(painter, 1);
        }
        if let Some(display) = &app.native_display
            && let Some((_, at, _)) = display.session.bound_image()
            && self.pending_epoch.is_none()
            && self.effects
        {
            let doc = kinewright_media::po0_light::version_key(
                self.current_document.as_ref().expect("document"),
            );
            self.light.update(ui, doc, at);
        }
        if self.effects && !self.neutral {
            self.light.paint_glow(painter, room, picture, false);
            for rect in [rail, composer] {
                self.light.paint_glow(
                    &painter.with_clip_rect(rect.shrink(1.0)),
                    rect,
                    picture,
                    true,
                );
            }
        }
        if let Some(timings) = &self.timings {
            timings.boundary(painter, 2);
        }
        if self.effects {
            self.light.paint_grain(ui, &[room, rail, timeline]);
        }
        if let Some(timings) = &self.timings {
            timings.boundary(painter, 3);
        }
        self.paint_scene(app, ui, picture, rail, composer, timeline, now);
        if let Some(fill) = &self.fill_pending_paint {
            ui.painter()
                .add(eframe::egui_wgpu::Callback::new_paint_callback(
                    ui.clip_rect(),
                    fill.clone(),
                ));
        }
        if let Some(timings) = &self.timings {
            timings.boundary(ui.painter(), 4);
        }
        if self.completed < self.repeats {
            let moving = match self.moment {
                3 => self.pending_epoch.is_some() || self.held.is_some(),
                5 => {
                    now - self.trial_at
                        < f64::from(
                            if self.reduce {
                                0.0
                            } else {
                                Token::Flight.seconds()
                            } + Token::Badge.seconds(),
                        )
                }
                6 => true,
                7 => self.strip.as_ref().is_some_and(|strip| strip.filled < 16),
                8 => now - self.trial_at < 0.7,
                10 => {
                    if self.reduce {
                        now - self.trial_at < f64::from(Token::Fade.seconds())
                            || (now - self.trial_at >= f64::from(Token::Mark.seconds())
                                && now - self.trial_at
                                    < f64::from(Token::Mark.seconds() + Token::Fade.seconds()))
                    } else {
                        now - self.trial_at
                            < f64::from(Token::Mark.seconds() + Token::MarkFade.seconds())
                    }
                }
                _ => motion::requests(ui.ctx()) > 0,
            };
            if self.trial > 0 && moving {
                motion::repaint(ui.ctx());
            }
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64((self.next_at - now).max(0.001)));
        }
    }
    pub(crate) fn worker_failure(&mut self, error: String) {
        if self.failures.len() < 512 {
            self.failures.push(error);
        }
    }
    pub(crate) fn finished(&self) -> bool {
        self.completed >= self.repeats
    }
    fn finish_window(&mut self, stats: kinewright_core::PlaybackStats) {
        self.window_closing = None;
        if let Some(window) = self.playback_window.take() {
            self.window_due += stats.due_frames.saturating_sub(window.stats.due_frames);
            self.window_bad += (stats.late + stats.dropped)
                .saturating_sub(window.stats.late + window.stats.dropped);
        }
    }
    pub(crate) fn acknowledge(&mut self, epoch: u64, ack: crate::presenter::Ack) {
        if let Some(window) = &self.playback_window
            && let Some(last) = self.last_paint
            && last >= window.began
            && self.window_present_ms.len() < 100_000
        {
            self.window_present_ms
                .push(ack.painted.duration_since(last).as_secs_f64() * 1000.0);
        }
        self.last_paint = Some(ack.painted);
        if let Some(first) = self.first_paint_epoch.take()
            && self.pending_epoch.is_none()
            && self.dissolve_at.is_some()
        {
            self.dissolve_delays
                .push(epoch.saturating_sub(first).saturating_sub(1) as f64);
        }
        let _ = ack;
    }

    #[allow(clippy::too_many_lines, reason = "drains bounded probe worker results")]
    fn poll_workers(&mut self, ui: &egui::Ui) {
        let visible = self.fill_pending_paint.as_ref().and_then(|fill| {
            fill.painted
                .try_lock()
                .ok()
                .and_then(|painted| *painted)
                .map(|painted| painted.saturating_duration_since(fill.began).as_secs_f64() * 1000.0)
        });
        if let Some(ms) = visible {
            self.fill_ms.push(ms);
            self.fill_pending_paint = None;
        }
        for strip in self
            .strip
            .iter_mut()
            .chain(self.version_strips.iter_mut().filter_map(Option::as_mut))
        {
            while let Ok(tile) = strip.incoming.try_recv() {
                match tile {
                    Ok(tile) => {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [tile.image.width as usize, tile.image.height as usize],
                            &tile.image.pixels,
                        );
                        if image.size == [144, 80] {
                            let grey = egui::ColorImage::new(
                                image.size,
                                image
                                    .pixels
                                    .iter()
                                    .map(|color| {
                                        Color32::from_gray(
                                            (0.2126 * f32::from(color.r())
                                                + 0.7152 * f32::from(color.g())
                                                + 0.0722 * f32::from(color.b()))
                                            .round()
                                                as u8,
                                        )
                                    })
                                    .collect(),
                            );
                            strip.grey.set_partial(
                                [tile.index * 144, 0],
                                grey,
                                egui::TextureOptions::LINEAR,
                            );
                            strip.atlas.set_partial(
                                [tile.index * 144, 0],
                                image,
                                egui::TextureOptions::LINEAR,
                            );
                        } else {
                            self.failures
                                .push(format!("tile dimensions {:?}", image.size));
                        }
                        self.tile_ms.push(tile.ms);
                        strip.filled += 1;
                        if strip.filled == 16 {
                            if self.moment == 7 {
                                self.fill_pending_paint = Some(FillPaint {
                                    began: strip.began,
                                    painted: Arc::new(std::sync::Mutex::new(None)),
                                });
                            } else {
                                self.fill_ms
                                    .push(strip.began.elapsed().as_secs_f64() * 1000.0);
                            }
                        }
                    }
                    Err(error) => {
                        self.failures.push(error);
                        strip.filled = 16;
                    }
                }
            }
            if strip.filled < 16 {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        }
        if self.moment == 7
            && self.playback_window.is_some()
            && self.window_closing.is_none()
            && self.strip.as_ref().is_some_and(|strip| strip.filled == 16)
        {
            self.window_closing = Some(ui.ctx().cumulative_frame_nr() + 2);
        }
        if !self.prewarm_finished
            && self
                .version_strips
                .iter()
                .all(|strip| strip.as_ref().is_some_and(|s| s.filled == 16))
        {
            self.prewarm_finished = true;
            let now = self.started.elapsed().as_secs_f64();
            self.next_at = (now + 3.0).max(6.0);
            self.idle_cpu_start = process_cpu().map(|cpu| (Instant::now(), cpu));
        }
        if let Some(result) = self.pin_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.pin_rx = None;
            match result {
                Ok(result) => {
                    if let Some(samples) = result
                        .get("subject_samples")
                        .and_then(serde_json::Value::as_array)
                    {
                        for sample in samples {
                            if let (Some(frame), Some(x), Some(y)) = (
                                sample["project_frame"].as_i64(),
                                sample["composite_x_basis_points"].as_f64(),
                                sample["composite_y_basis_points"].as_f64(),
                            ) {
                                self.pin
                                    .push((frame, [x as f32 / 10000.0, y as f32 / 10000.0]));
                            }
                        }
                    }
                    if self.pin.is_empty() {
                        self.failures
                            .push(format!("tracker returned no samples: {result}"));
                    }
                    self.trial_at = self.started.elapsed().as_secs_f64();
                    self.next_at = self.trial_at + 0.85;
                }
                Err(error) => self.failures.push(error),
            }
        }
    }
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the ten isolated visual scenes share one composition"
    )]
    fn paint_scene(
        &mut self,
        app: &mut KinewrightApp,
        ui: &mut egui::Ui,
        picture: Rect,
        rail: Rect,
        composer: Rect,
        timeline: Rect,
        now: f64,
    ) {
        let phase = (now - self.trial_at).max(0.0) as f32;
        let painter = ui.painter().clone();
        let blue = Color32::from_rgb(10, 132, 255);
        let text = Color32::from_rgb(245, 245, 247);
        let lit = if self.neutral {
            Color32::GRAY
        } else {
            self.light.lit
        };
        painter.rect_filled(rail, 16, Color32::from_rgba_unmultiplied(24, 24, 28, 133));
        painter.rect_filled(
            composer,
            26,
            Color32::from_rgba_unmultiplied(30, 30, 34, 128),
        );
        painter.rect_filled(timeline, 12, Color32::from_rgb(19, 19, 22));
        painter.line_segment(
            [rail.left_top(), rail.left_bottom()],
            egui::Stroke::new(1.0, lit.gamma_multiply(0.5)),
        );
        painter.line_segment(
            [composer.left_top(), composer.right_top()],
            egui::Stroke::new(1.0, lit.gamma_multiply(0.5)),
        );
        painter.text(
            Pos2::new(picture.left(), 32.0),
            egui::Align2::LEFT_TOP,
            "TALKING HEAD  ·  PO0 MOTION PROBE",
            egui::FontId::proportional(11.0),
            Color32::GRAY,
        );
        painter.text(
            Pos2::new(picture.left(), 50.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Moment {}   ·   {} / {}",
                self.moment, self.completed, self.repeats
            ),
            egui::FontId::proportional(18.0),
            text,
        );
        let neutral_response = ui.put(
            Rect::from_min_size(Pos2::new(rail.left(), 32.0), Vec2::new(150.0, 28.0)),
            egui::Checkbox::new(&mut self.neutral, "Neutral surround"),
        );
        let _ = neutral_response;
        ui.put(
            Rect::from_min_size(Pos2::new(rail.left() + 160.0, 32.0), Vec2::new(150.0, 28.0)),
            egui::Checkbox::new(&mut self.reduce, "Reduce motion"),
        );
        painter.rect_filled(picture.expand(1.0), 4, Color32::BLACK);
        let fade = self.dissolve_at.map_or(1.0, |start| {
            let duration = if self.reduce {
                0.1
            } else {
                f64::from(Token::Standard.seconds())
            };
            motion::ease_in_out(((now - start) / duration).clamp(0.0, 1.0) as f32)
        });
        if let Some(held) = &self.held {
            painter.image(
                held.id,
                picture,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if let Some((id, _)) = app.program_picture()
            && self.pending_epoch.is_none()
            && self.moment != 8
        {
            painter.image(
                id,
                picture,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::from_white_alpha((fade * 255.0).max(1.0) as u8),
            );
            app.presenter
                .marker(
                    ui.ctx().cumulative_frame_nr_for(egui::ViewportId::ROOT),
                    app.paint_clock(),
                )
                .add_to(&painter, picture);
        }
        if fade >= 1.0
            && self.pending_epoch.is_none()
            && let Some(held) = self.held.take()
            && let Some(display) = &app.native_display
        {
            held.release(&display.renderer);
            self.window_closing = Some(ui.ctx().cumulative_frame_nr() + 2);
        }
        let target_version = usize::from(
            kinewright_media::po0_light::version_key(
                self.current_document.as_ref().expect("document"),
            ) != 0,
        );
        let displayed = if self.pending_epoch.is_some() {
            1 - target_version
        } else {
            target_version
        };
        let thumb = motion::animate(
            ui,
            egui::Id::new("po0-version-thumb"),
            displayed as f32 * 36.0,
            Token::Spring,
        );
        let picker = Rect::from_min_size(
            picture.left_bottom() + Vec2::new(0.0, 16.0),
            Vec2::new(80.0, 28.0),
        );
        painter.rect_filled(picker, 14, Color32::from_white_alpha(17));
        painter.rect_filled(
            Rect::from_min_size(
                picker.min + Vec2::new(4.0 + thumb, 3.0),
                Vec2::new(32.0, 22.0),
            ),
            11,
            blue,
        );
        for (index, label) in ["A", "B"].iter().enumerate() {
            painter.text(
                picker.min + Vec2::new(20.0 + index as f32 * 36.0, 14.0),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(12.0),
                text,
            );
        }
        let elapsed = self.dissolve_at.map(|start| (now - start).max(0.0) as f32);
        let (version, alpha, rise) = if let Some(elapsed) = elapsed {
            if self.reduce {
                (
                    displayed,
                    motion::ease_in_out((elapsed / Token::Fade.seconds()).min(1.0)),
                    0.0,
                )
            } else if elapsed < Token::LabelExit.seconds() {
                (
                    1 - displayed,
                    1.0 - motion::ease_in(elapsed / Token::LabelExit.seconds()),
                    0.0,
                )
            } else {
                let t = Token::LabelRise.curve(
                    ((elapsed - Token::LabelExit.seconds()) / Token::LabelRise.seconds()).min(1.0),
                );
                (displayed, t, 3.0 * (1.0 - t))
            }
        } else {
            (displayed, 1.0, 0.0)
        };
        painter.text(
            picture.left_bottom() + Vec2::new(96.0, 24.0 + rise),
            egui::Align2::LEFT_TOP,
            if version == 0 {
                "Version A"
            } else {
                "Version B"
            },
            egui::FontId::proportional(15.0),
            text.gamma_multiply(alpha),
        );
        let at = app
            .native_display
            .as_ref()
            .and_then(|display| display.session.bound_image().map(|(_, at, _)| at.0))
            .unwrap_or(0)
            .max(0);
        painter.text(
            picture.right_bottom() + Vec2::new(0.0, 24.0),
            egui::Align2::RIGHT_TOP,
            format!("{:02}:{:02}:{:02}", at / 1800, at / 30 % 60, at % 30),
            egui::FontId::monospace(15.0),
            text,
        );
        painter.text(
            composer.center(),
            egui::Align2::CENTER_CENTER,
            "Tell the room what you want to change",
            egui::FontId::proportional(13.0),
            text.gamma_multiply(0.64),
        );
        if self.moment == 2 {
            if self.trial > 0 {
                ui.ctx().request_repaint_after(Duration::from_secs_f32(
                    (0.3 - phase.rem_euclid(0.3)).max(0.001),
                ));
            }
            let mut row = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
                picture.left_bottom() + Vec2::new(0.0, 60.0),
                Vec2::new(picture.width(), 40.0),
            )));
            row.horizontal(|ui| {
                for button in 0..12 {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(72.0, 32.0), egui::Sense::click());
                    let active = (self.trial > 0 && ((phase / 0.3) as u32).min(11) == button)
                        || response.hovered();
                    let wash = motion::animate(
                        ui,
                        egui::Id::new(("po0-button", button)),
                        f32::from(active),
                        if active { Token::Micro } else { Token::Quick },
                    );
                    let pressed = (phase.rem_euclid(0.3) < 0.05 && active && self.trial > 0)
                        || response.is_pointer_button_down_on();
                    let press = motion::animate(
                        ui,
                        egui::Id::new(("po0-press", button)),
                        f32::from(pressed),
                        if pressed {
                            Token::Instant
                        } else {
                            Token::RowExit
                        },
                    );
                    ui.painter().rect_filled(
                        rect.shrink(press),
                        16,
                        Color32::from_white_alpha(egui::lerp(wash * 17.0..=41.0, press) as u8),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("Action {}", button + 1),
                        egui::FontId::proportional(12.0),
                        text,
                    );
                }
            });
        }
        if rail.width() > 40.0 {
            self.paint_commentary(ui, rail, text);
        }
        if self.moment == 3 && self.prewarm_finished {
            let current = self.version_strips[displayed]
                .as_ref()
                .expect("prewarmed current strip");
            if fade < 1.0 {
                paint_strip(
                    &painter,
                    self.version_strips[1 - displayed]
                        .as_ref()
                        .expect("prewarmed outgoing strip"),
                    timeline,
                    self.neutral,
                    1.0,
                );
            }
            paint_strip(&painter, current, timeline, self.neutral, fade);
        } else if let Some(strip) = &self.strip {
            paint_strip(&painter, strip, timeline, self.neutral, 1.0);
        } else {
            for index in 0..16 {
                painter.rect_filled(
                    Rect::from_min_size(
                        timeline.min
                            + Vec2::new(
                                8.0 + index as f32 * (timeline.width() - 16.0) / 16.0,
                                24.0,
                            ),
                        Vec2::new((timeline.width() - 16.0) / 16.0 - 2.0, 40.0),
                    ),
                    5,
                    Color32::from_rgb(28, 37, 43),
                );
            }
        }
        if self.moment == 5 {
            let t = if self.reduce {
                1.0
            } else {
                (phase / Token::Flight.seconds()).min(1.0)
            };
            let from = composer.left_center();
            let to = timeline.center_top() + Vec2::new(timeline.width() * 0.2, 12.0);
            let peak = from.y.min(to.y) - 46.0;
            let x = egui::lerp(from.x..=to.x, motion::ease_in_out(t));
            let y = if t < 0.5 {
                egui::lerp(from.y..=peak, motion::ease_out(t * 2.0))
            } else {
                egui::lerp(peak..=to.y, motion::ease_in(t * 2.0 - 1.0))
            };
            if t < 1.0 {
                painter.circle_filled(Pos2::new(x, y), 3.5, blue);
            } else {
                let id = egui::Id::new(("po0-note-land", self.trial));
                if ui.ctx().data(|data| data.get_temp::<bool>(id)).is_none() {
                    motion::animate(ui, id.with("radius"), 7.35, Token::Spring);
                    ui.ctx().data_mut(|data| data.insert_temp(id, true));
                }
                let radius = motion::animate(ui, id.with("radius"), 3.5, Token::Spring);
                let landing = if self.reduce {
                    phase / Token::Fade.seconds()
                } else {
                    (phase - Token::Flight.seconds()) / Token::Landing.seconds()
                };
                painter.circle_filled(to, if self.reduce { 3.5 } else { radius }, blue);
                if !self.reduce {
                    painter.circle_stroke(
                        to,
                        7.0 + landing * 10.0,
                        egui::Stroke::new(
                            1.0,
                            blue.gamma_multiply((1.0 - landing).clamp(0.0, 1.0)),
                        ),
                    );
                }
            }
            let landed_at = if self.reduce {
                0.0
            } else {
                Token::Flight.seconds()
            };
            if phase >= landed_at && phase < landed_at + Token::Badge.seconds() {
                painter.text(
                    composer.right_center() - Vec2::new(20.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    "Notes",
                    egui::FontId::proportional(12.0),
                    blue,
                );
            }
        }
        if self.moment == 8 {
            let progress = (phase / 0.7).min(1.0);
            let resolution = if progress < 0.25 {
                16
            } else if progress < 0.5 {
                32
            } else if progress < 0.75 {
                64
            } else {
                128
            };
            if self
                .progressive
                .as_ref()
                .is_none_or(|(size, _)| *size != resolution)
            {
                let image = egui::ColorImage::new(
                    [resolution, resolution * 9 / 16],
                    (0..resolution * (resolution * 9 / 16))
                        .map(|pixel| {
                            Color32::from_rgb(
                                (pixel % resolution * 255 / resolution) as u8,
                                ((pixel / resolution) * 255 / (resolution * 9 / 16)) as u8,
                                110,
                            )
                        })
                        .collect(),
                );
                self.progressive = Some((
                    resolution,
                    ui.ctx().load_texture(
                        "PO0 progressive pass",
                        image,
                        egui::TextureOptions::LINEAR,
                    ),
                ));
            }
            if let Some((_, texture)) = &self.progressive {
                painter.image(
                    texture.id(),
                    picture,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            let x = egui::lerp(timeline.left()..=timeline.right(), progress);
            painter.line_segment(
                [
                    Pos2::new(x, timeline.top()),
                    Pos2::new(x, timeline.bottom()),
                ],
                egui::Stroke::new(2.0, lit),
            );
        }
        if self.moment == 10 && !self.neutral && !self.pin.is_empty() {
            let t = if self.reduce {
                1.0
            } else {
                Token::Mark.curve((phase / Token::Mark.seconds()).min(1.0))
            };
            let mark_alpha = if self.reduce {
                (phase / Token::Fade.seconds()).min(1.0)
            } else {
                1.0
            };
            let exit = 1.0
                - motion::ease_in(
                    ((phase - Token::Mark.seconds())
                        / if self.reduce {
                            Token::Fade.seconds()
                        } else {
                            Token::MarkFade.seconds()
                        })
                    .clamp(0.0, 1.0),
                );
            let mark_alpha = mark_alpha * exit;
            let count = 99_usize;
            let points: Vec<_> = (0..=count)
                .map(|index| {
                    let angle = index as f32 / 99.0 * std::f32::consts::TAU * 1.08;
                    picture.center()
                        + Vec2::new(
                            angle.cos() * picture.width() * 0.12,
                            angle.sin() * picture.height() * 0.19,
                        )
                })
                .collect();
            let points = cut_path(&points, t);
            let scale = picture.width() / 960.0;
            for (width, color) in [
                (5.4, Color32::from_black_alpha(117)),
                (2.4, Color32::from_rgb(164, 191, 94)),
            ] {
                painter.add(egui::Shape::line(
                    points.clone(),
                    egui::Stroke::new(width * scale, color.gamma_multiply(mark_alpha)),
                ));
            }
            let at = app
                .native_display
                .as_ref()
                .and_then(|display| display.session.bound_image().map(|(_, at, _)| at.0))
                .unwrap_or(0);
            if let Some(center) = pin_center(&self.pin, at) {
                let anchor = picture.min + picture.size() * Vec2::new(center[0], center[1]);
                let id = egui::Id::new(("po0-pin", self.trial));
                if ui.ctx().data(|data| data.get_temp::<bool>(id)).is_none() {
                    motion::animate(ui, id.with("scale"), 16.5, Token::Spring);
                    ui.ctx().data_mut(|data| data.insert_temp(id, true));
                }
                let scale =
                    motion::animate(ui, id.with("scale"), 30.0, Token::Spring) / 30.0 * scale;
                let head = anchor + Vec2::new(17.0, -30.0) * scale;
                for (width, color) in [(4.0, Color32::from_black_alpha(117)), (1.8, blue)] {
                    painter.line_segment(
                        [anchor, head],
                        egui::Stroke::new(width * scale, color.gamma_multiply(mark_alpha)),
                    );
                    painter.circle_stroke(
                        anchor,
                        3.3 * scale,
                        egui::Stroke::new(width * scale, color.gamma_multiply(mark_alpha)),
                    );
                    painter.circle_stroke(
                        head,
                        8.6 * scale,
                        egui::Stroke::new(width * scale, color.gamma_multiply(mark_alpha)),
                    );
                }
            }
        }
        let at = app.playback.position().0 as f32 / (30.0 * 660.0);
        let x = egui::lerp(timeline.left()..=timeline.right(), at);
        painter.line_segment(
            [
                Pos2::new(x, timeline.top()),
                Pos2::new(x, timeline.bottom()),
            ],
            egui::Stroke::new(2.0, text),
        );
    }
    fn paint_commentary(&mut self, ui: &mut egui::Ui, rail: Rect, text: Color32) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rail.shrink(16.0)));
        let scroll = egui::ScrollArea::vertical()
            .id_salt("po0-commentary")
            .stick_to_bottom(true)
            .max_height(rail.height() - 60.0);
        let scroll = if self.moment == 9 && !self.at_bottom {
            scroll.vertical_scroll_offset(120.0)
        } else {
            scroll
        };
        scroll.show(&mut child, |ui| {
            for row in &self.list.rows {
                paint_row(ui, row, text, self.neutral, self.reduce);
            }
        });
        if self.list.queued > 0 {
            let id = egui::Id::new("po0-new-pill");
            if child.ctx().data(|data| data.get_temp::<bool>(id)).is_none() {
                motion::animate(&child, id.with("rise"), 10.0, Token::Spring);
                child.ctx().data_mut(|data| data.insert_temp(id, true));
            }
            let offset = motion::animate(&child, id.with("rise"), 0.0, Token::Spring);
            if child
                .put(
                    Rect::from_center_size(
                        rail.center_bottom() - Vec2::new(0.0, 20.0 - offset),
                        Vec2::new(100.0, 28.0),
                    ),
                    egui::Button::new(format!("↓ {} new", self.list.queued)),
                )
                .clicked()
            {
                self.list.release();
                self.at_bottom = true;
            }
        }
    }

    pub(crate) fn observe(
        &mut self,
        app: &KinewrightApp,
        ctx: &egui::Context,
        cpu: Option<f32>,
        began: Instant,
    ) {
        let now = began;
        let elapsed = self.started.elapsed().as_secs_f64();
        let completions = motion::take_completions(ctx);
        if self.trial > 0 && self.completions.len() < 20_000 {
            self.completions.extend(completions);
        }
        if elapsed >= 6.0
            && self.completed < self.repeats
            && motion::any_requests(ctx)
            && self.intervals.len() < 100_000
        {
            if let Some(last) = self.last_frame {
                self.intervals
                    .push(now.duration_since(last).as_secs_f64() * 1000.0);
            }
            if let Some(cpu) = cpu {
                self.cpu_ms.push(f64::from(cpu) * 1000.0);
            }
        }
        self.last_frame = motion::any_requests(ctx).then_some(now);
        if motion::requests(ctx) > 0 || motion::finished(ctx) {
            self.last_motion_at = elapsed;
        }
        if self.trial > 0
            && self.settled_trial == self.trial
            && matches!(self.moment, 1)
            && motion::requests(ctx) > 0
        {
            self.post_settle_repaints += 1;
        }
        if motion::finished(ctx) {
            self.settled_trial = self.trial;
        }
        if let Some(residual) = motion::snap_residual(ctx) {
            self.snap_residuals.push(residual);
        }
        if let Some(directory) = &self.capture_dir {
            let images = ctx.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| {
                        if let egui::Event::Screenshot {
                            user_data, image, ..
                        } = event
                        {
                            let number = user_data.data.as_ref()?.downcast_ref::<u32>()?;
                            Some((*number, Arc::clone(image)))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            });
            for (number, image) in images {
                let directory = directory.clone();
                app.off_ui(
                    ctx,
                    move || {
                        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
                        let pixels: Vec<u8> =
                            image.pixels.iter().flat_map(Color32::to_array).collect();
                        image::save_buffer(
                            directory.join(format!("frame-{number:04}.png")),
                            &pixels,
                            image.size[0] as u32,
                            image.size[1] as u32,
                            image::ColorType::Rgba8,
                        )
                        .map_err(|e| e.to_string())
                    },
                    |app, _, result: Result<(), String>| {
                        if let Err(error) = result
                            && let Some(probe) = &mut app.motion_probe
                        {
                            probe.worker_failure(error);
                        }
                    },
                );
            }
            if self.trial == 1 && elapsed - self.trial_at >= 0.40 && elapsed - self.trial_at <= 0.72
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                    self.captures,
                )));
                self.captures += 1;
            }
        }
        self.max_position = self.max_position.max(app.playback.position().0);
    }
    pub(crate) fn report(&self, stats: kinewright_core::PlaybackStats) -> serde_json::Value {
        let hz = std::env::var("KINEWRIGHT_PROBE_HZ")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(60.0);
        let period = 1000.0 / hz;
        let dropped = self
            .intervals
            .iter()
            .filter(|ms| **ms > period * 1.5)
            .count();
        let consecutive = self
            .intervals
            .windows(2)
            .any(|w| w.iter().all(|ms| *ms > period * 1.5));
        let gpu = self.timings.as_ref().map(Timings::samples);
        let columns: Option<Vec<_>> = gpu.as_ref().map(|samples| {
            (0..3)
                .map(|i| distribution(&samples.iter().map(|s| s[i]).collect::<Vec<_>>()))
                .collect()
        });
        let g1 = if stats.due_frames > 0 {
            Some((stats.late + stats.dropped) as f64 / stats.due_frames as f64 * 100.0)
        } else {
            None
        };
        let mut report = serde_json::json!({"moment":self.moment,"actions_per_repetition":if self.moment==2 {12}else{1},"viewport_pixels":self.viewport_pixels,"spring_variant":if self.deadline_snap {"deadline-snap-520ms"} else {"specified-physics"},"snap_residuals_distance_pt_velocity_pt_s":self.snap_residuals,"playback":self.playback,"max_position_frame":self.max_position,"effects":self.effects,"requested_repetitions":self.repeats,
            "completed_repetitions":self.completed,"failures":self.failures,"adapter":self.gpu_adapter,"refresh_hz":hz,
            "interval_ms":distribution(&self.intervals),"cpu_frame_ms":distribution(&self.cpu_ms)});
        let extra = serde_json::json!({"dropped_frames":dropped,"dropped_percent":if self.intervals.is_empty(){None}else{Some(dropped as f64/self.intervals.len() as f64*100.0)},
            "consecutive_drops":consecutive,"gpu_light_grain_rest_ms":columns,"gpu_effects_ms":gpu.as_ref().map(|samples|distribution(&samples.iter().map(|s|s[0]+s[1]).collect::<Vec<_>>())),"gpu_samples":gpu.as_ref().map(Vec::len),
            "idle_cpu_percent_of_one_core":if self.playback {None}else{self.process_idle},"static_baseline_cpu_percent_of_one_core":self.process_idle,"animated_cpu_percent_of_one_core":self.process_animated.or_else(||core_percent(self.animation_cpu_start)),
            "motion_repaints_after_settle":self.post_settle_repaints,"repaint_windows_ms":distribution(&self.repaint_windows),
            "downsample_worker_ms":distribution(&self.light.worker_ms),"downsample_updates":self.light.worker_ms.len(),
            "sampled_frames":self.light.sampled_frames,"thumbnail_decode_ms":distribution(&self.tile_ms),"strip_fill_ms":distribution(&self.fill_ms),
            "pf1_g1_bad_percent":g1,"pf1_window_bad_percent":if self.window_due==0 {None}else{Some(self.window_bad as f64/self.window_due as f64*100.0)},"pf1_window_due":self.window_due,"pf1_window_present_ms":distribution(&self.window_present_ms),"pf1_due_frames":stats.due_frames,"pf1_on_time":stats.on_time,"pf1_acks_unmatched":stats.acks_unmatched,"pf1_late":stats.late,"pf1_dropped":stats.dropped,"pf1_max_held_ms":stats.max_held_ms,
            "atlas_bytes":(usize::from(self.strip.is_some())+self.version_strips.iter().flatten().count())*144*80*4*16*2,"held_frame_peak_bytes":self.held_peak,"light_texture_bytes":self.light.bytes(),
            "dissolve_start_delay_frames":distribution(&self.dissolve_delays),"tracked_pin_samples":self.pin.len(),
            "surface_capabilities":SURFACE.get(),"transition_completions":self.completions});
        if let serde_json::Value::Object(extra) = extra {
            report
                .as_object_mut()
                .expect("motion report object")
                .extend(extra);
        }
        report
    }
}
fn paint_strip(painter: &egui::Painter, strip: &Strip, timeline: Rect, neutral: bool, alpha: f32) {
    for index in 0..16 {
        let rect = Rect::from_min_size(
            timeline.min + Vec2::new(8.0 + index as f32 * (timeline.width() - 16.0) / 16.0, 24.0),
            Vec2::new((timeline.width() - 16.0) / 16.0 - 1.0, 40.0),
        );
        painter.image(
            if neutral {
                strip.grey.id()
            } else {
                strip.atlas.id()
            },
            rect,
            Rect::from_min_max(
                Pos2::new(index as f32 / 16.0, 0.0),
                Pos2::new((index + 1) as f32 / 16.0, 1.0),
            ),
            Color32::from_gray(153).gamma_multiply(alpha),
        );
    }
}

fn paint_row(ui: &mut egui::Ui, row: &motion::ListRow, text: Color32, neutral: bool, reduce: bool) {
    let id = egui::Id::new(("po0-row", row.id));
    let fresh = row.id >= 40;
    if fresh
        && ui
            .ctx()
            .data(|data| data.get_temp::<bool>(id.with("initialized")))
            .is_none()
    {
        motion::animate(ui, id, 0.0, Token::Spring);
        ui.ctx()
            .data_mut(|data| data.insert_temp(id.with("initialized"), true));
    }
    let now = ui.input(|input| input.time);
    let removed = if row.removing {
        ui.ctx()
            .data_mut(|data| *data.get_temp_mut_or_insert_with(id.with("removed"), || now))
    } else {
        now
    };
    let removing_for = (now - removed) as f32;
    let collapse = row.removing && removing_for >= Token::RowExit.seconds();
    let height = motion::animate(
        ui,
        id,
        if collapse { 0.0 } else { 64.0 },
        if reduce {
            Token::Instant
        } else if row.removing {
            Token::Quick
        } else {
            Token::Spring
        },
    );
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let time = ui.input(|input| input.time);
    let born = ui
        .ctx()
        .data_mut(|data| *data.get_temp_mut_or_insert_with(id.with("born"), || time));
    let elapsed = (time - born) as f32;
    let fade = if fresh {
        motion::ease_in_out(
            ((elapsed - Token::RowDelay.seconds())
                / if reduce {
                    Token::RowFade.seconds()
                } else {
                    Token::Standard.seconds()
                })
            .clamp(0.0, 1.0),
        )
    } else {
        1.0
    };
    let fade = fade
        * if row.removing {
            (1.0 - removing_for / Token::RowExit.seconds()).clamp(0.0, 1.0)
        } else {
            1.0
        };
    let offset = if reduce { 0.0 } else { 5.0 * (1.0 - fade) };
    let painter = ui.painter().with_clip_rect(rect);
    let hue = if neutral {
        Color32::GRAY
    } else {
        Color32::from_rgb(189, 215, 124)
    };
    if fresh && !reduce && elapsed < Token::Ring.seconds() {
        let width = 1.0 + 2.0 * (std::f32::consts::PI * elapsed / Token::Ring.seconds()).sin();
        painter.circle_stroke(
            rect.left_top() + Vec2::new(12.0, 20.0),
            10.0,
            egui::Stroke::new(width, hue.gamma_multiply(0.42)),
        );
    }
    painter.text(
        rect.left_top() + Vec2::new(28.0, 8.0 + offset),
        egui::Align2::LEFT_TOP,
        format!("Review  ·  {}", row.id),
        egui::FontId::proportional(13.0),
        hue.gamma_multiply(fade),
    );
    painter.text(
        rect.left_top() + Vec2::new(4.0, 30.0 + offset),
        egui::Align2::LEFT_TOP,
        "The cut is ready for your look.",
        egui::FontId::proportional(13.0),
        text.gamma_multiply(0.82 * fade),
    );
}

fn cut_path(points: &[Pos2], fraction: f32) -> Vec<Pos2> {
    let Some(first) = points.first() else {
        return Vec::new();
    };
    let mut remaining = points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum::<f32>()
        * fraction.clamp(0.0, 1.0);
    let mut output = vec![*first];
    for pair in points.windows(2) {
        let length = pair[0].distance(pair[1]);
        if remaining >= length {
            output.push(pair[1]);
            remaining -= length;
        } else {
            if length > 0.0 {
                output.push(pair[0] + (pair[1] - pair[0]) * (remaining / length));
            }
            break;
        }
    }
    output
}

fn pin_center(samples: &[(i64, [f32; 2])], at: i64) -> Option<[f32; 2]> {
    let first = samples.first()?;
    for pair in samples.windows(2) {
        if at >= pair[0].0 && at <= pair[1].0 {
            let t = (at - pair[0].0) as f32 / (pair[1].0 - pair[0].0).max(1) as f32;
            return Some(std::array::from_fn(|i| {
                egui::lerp(pair[0].1[i]..=pair[1].1[i], t)
            }));
        }
    }
    Some(
        samples
            .last()
            .filter(|last| at >= last.0)
            .unwrap_or(first)
            .1,
    )
}
fn process_cpu() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        let rest = stat.rsplit_once(") ")?.1;
        let fields: Vec<_> = rest.split_whitespace().collect();
        // Linux /proc clock ticks are queried, never inferred from frame counts.
        let ticks = std::process::Command::new("getconf")
            .arg("CLK_TCK")
            .output()
            .ok()?;
        let ticks = std::str::from_utf8(&ticks.stdout)
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()?;
        Some((fields.get(11)?.parse::<f64>().ok()? + fields.get(12)?.parse::<f64>().ok()?) / ticks)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
fn core_percent(start: Option<(Instant, f64)>) -> Option<f64> {
    let (time, cpu) = start?;
    Some((process_cpu()? - cpu) / time.elapsed().as_secs_f64() * 100.0)
}
pub(crate) fn distribution(values: &[f64]) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: usize| sorted[(sorted.len() * p).div_ceil(100).saturating_sub(1)];
    serde_json::json!({"count":sorted.len(),"p50":percentile(50),"p95":percentile(95),"p99":percentile(99),"max":sorted.last()})
}

pub(crate) fn spring_report() -> bool {
    if !enabled() {
        return false;
    }
    let Some(output) = std::env::var_os("KINEWRIGHT_PROBE_SPRING_REPORT") else {
        return false;
    };
    let mut rows = Vec::new();
    for panel in [false, true] {
        for distance in [32.0, 64.0, 160.0, 336.0] {
            for hz in [60.0, 120.0, 144.0] {
                let mut spring = motion::Spring::new(0.0, panel);
                spring.target(distance);
                let mut frames = 0_u32;
                while frames < 240 {
                    frames += 1;
                    if !spring.step(1.0 / hz) {
                        break;
                    }
                }
                let mut snap = motion::Spring::new(0.0, panel);
                snap.target(distance);
                snap.step(0.520);
                rows.push(serde_json::json!({"panel":panel,"distance_pt":distance,"hz":hz,"settle_ms":f64::from(frames)*1000.0/f64::from(hz),"token_window_ms":if panel {514}else{488},"snap_520_distance_pt":distance-snap.value,"snap_520_velocity_pt_s":snap.velocity}));
            }
        }
    }
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&rows).expect("spring JSON"),
    )
    .expect("spring report output");
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracked_pin_interpolates_the_tracker_samples() {
        assert_eq!(
            pin_center(&[(0, [0.2, 0.4]), (10, [0.8, 0.6])], 5),
            Some([0.5, 0.5])
        );
    }
}

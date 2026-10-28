//! Studio composition and selection. Browsing never changes the manual Core.
use crate::{
    app::KinewrightApp,
    icons::Icon,
    ja1_attachment::Action,
    presenter::{DisplayCell, PresentationContext},
    theme::{self, space},
};
use eframe::egui::{self, Color32, Rect, Vec2};
use kinewright_core::{Document, TimeCode};
use kinewright_project::{studio_records::SnapshotKey, studio_store::FrozenContext};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    Studio,
    #[default]
    Editor,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    Commentary,
    Notes,
    Timeline,
    Compare,
}

pub(crate) struct Still {
    pub(crate) cell: DisplayCell,
    image: StillImage,
    _charge: kinewright_media::owned_io::LifetimeGuard,
}
enum StillImage {
    Cpu(egui::TextureHandle),
    Gpu {
        id: egui::TextureId,
        size: Vec2,
        _texture: eframe::wgpu::Texture,
        renderer: eframe::egui_wgpu::RenderState,
    },
}
impl StillImage {
    fn image(&self) -> (egui::TextureId, Vec2) {
        match self {
            Self::Cpu(texture) => (texture.id(), texture.size_vec2()),
            Self::Gpu { id, size, .. } => (*id, *size),
        }
    }
}
impl Drop for StillImage {
    fn drop(&mut self) {
        if let Self::Gpu { id, renderer, .. } = self {
            renderer.renderer.write().free_texture(id);
        }
    }
}
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent window, scroll and surround preferences"
)]
pub(crate) struct Studio {
    pub(crate) mode: Mode,
    pub(crate) selection: Option<Arc<FrozenContext>>,
    return_to: Option<Arc<FrozenContext>>,
    return_at: TimeCode,
    compare_open: bool,
    pub(crate) position: TimeCode,
    pub(crate) failure: Option<String>,
    pub(crate) cpu_frame: Option<kinewright_core::FrameTexture>,
    /// The presenter frame id of `cpu_frame`, so a proof hashes only the bound image.
    pub(crate) cpu_frame_id: Option<u64>,
    pub(crate) feedback: crate::studio_feedback::Feedback,
    pub(crate) manual_inputs: Option<Arc<kinewright_media::owned_io::PlaybackInputs>>,
    manual_scope: Option<(u64, kinewright_project::bundle_records::ObjectRef)>,
    versions: Vec<SnapshotKey>,
    versions_open: bool,
    pub(crate) tab: Tab,
    draft_snapshot: Option<SnapshotKey>,
    stills: [Option<Still>; 2],
    following: bool,
    seen: usize,
    unseen: usize,
    jump: bool,
    neutral: bool,
}

/// PO0 rev 2.2 tokens that the current theme can express (§2): colour, radii,
/// spacing and type. Glass, frame light, grain and springs stay in the PO0 rollout.
pub(crate) mod po0 {
    use eframe::egui::Color32;

    const fn white(alpha: u8) -> Color32 {
        Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha)
    }
    const fn ink(alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied_const(0xEB, 0xEB, 0xF5, alpha)
    }
    pub(crate) const BASE: Color32 = Color32::from_rgb(0x0B, 0x0B, 0x0D);
    pub(crate) const WELL: Color32 = Color32::from_rgb(0x13, 0x13, 0x16);
    pub(crate) const RAIL: Color32 = Color32::from_rgb(0x18, 0x18, 0x1C);
    pub(crate) const COMPOSER: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x20);
    /// The recessed track behind segmented controls and the time chip.
    pub(crate) const TRACK: Color32 = Color32::from_black_alpha(71);
    pub(crate) const FILL: Color32 = white(17);
    pub(crate) const FILL_2: Color32 = white(26);
    pub(crate) const FILL_3: Color32 = white(41);
    pub(crate) const LINE: Color32 = white(18);
    pub(crate) const SHEEN: Color32 = white(20);
    pub(crate) const TEXT: Color32 = Color32::from_rgb(0xF5, 0xF5, 0xF7);
    pub(crate) const TEXT_2: Color32 = ink(163);
    pub(crate) const TEXT_3: Color32 = ink(133);
    pub(crate) const TEXT_OFF: Color32 = ink(71);
    /// `you`: the person's ink, and the send arrow.
    pub(crate) const YOU: Color32 = Color32::from_rgb(0x0A, 0x84, 0xFF);
    /// The active-chip treatment: `you` at 26% with a 60% ring.
    pub(crate) const YOU_TINT: Color32 =
        Color32::from_rgba_unmultiplied_const(0x0A, 0x84, 0xFF, 66);
    pub(crate) const YOU_RING: Color32 =
        Color32::from_rgba_unmultiplied_const(0x0A, 0x84, 0xFF, 153);
    /// `accent-fill`: fills that carry white text.
    pub(crate) const ACCENT_FILL: Color32 = Color32::from_rgb(0x00, 0x71, 0xE3);
    pub(crate) const ACCENT_FILL_HOVER: Color32 = Color32::from_rgb(0x00, 0x68, 0xD1);
    pub(crate) const WARNING: Color32 = Color32::from_rgb(0xE7, 0xB2, 0x4F);
    pub(crate) const DANGER: Color32 = Color32::from_rgb(0xF2, 0x6B, 0x73);
    /// Neutral clip bodies: no role hues and no thumbnails until the PO0 rollout.
    pub(crate) const CLIP: Color32 = Color32::from_rgb(0x24, 0x24, 0x29);
    pub(crate) const CLIP_CURRENT: Color32 = Color32::from_rgb(0x2E, 0x2E, 0x34);
    pub(crate) const CLIP_AUDIO: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x20);
    pub(crate) const CLIP_LABEL: Color32 =
        Color32::from_rgba_unmultiplied_const(0xFF, 0xFF, 0xFF, 235);
}
const RAIL_WIDTH: f32 = 336.0;
const PLATE_HEIGHT: f32 = 40.0;
const COMPOSER_HEIGHT: f32 = 52.0;
const COMPOSER_WIDTH: f32 = 560.0;
const STUDIO_TIMELINE_HEIGHT: f32 = 112.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub(crate) narrow: bool,
    pub(crate) header: Rect,
    pub(crate) picture: Rect,
    pub(crate) plate: Rect,
    pub(crate) composer: Rect,
    pub(crate) rail: Rect,
    pub(crate) timeline: Rect,
}
impl Layout {
    /// PO0 §2 Spacing: one 24 pt margin, a 64 pt top bar, the stage and a 336 pt
    /// rail after a 24 pt gutter, and a 16 pt rhythm from picture to timeline.
    pub(crate) fn new(view: Rect, editor: bool) -> Self {
        let narrow = view.width() < 1000.0;
        let margin = if narrow { space::THREE } else { space::SIX };
        let header = Rect::from_min_size(
            egui::pos2(view.left() + margin, view.top()),
            egui::vec2(
                view.width() - 2.0 * margin,
                if narrow { 52.0 } else { 64.0 },
            ),
        );
        let inner = Rect::from_min_max(
            egui::pos2(view.left() + margin, header.bottom()),
            view.max - egui::vec2(margin, margin),
        );
        let top = inner.top();
        let timeline_h = if editor {
            240.0
        } else {
            STUDIO_TIMELINE_HEIGHT
        };
        let timeline = Rect::from_min_max(
            egui::pos2(inner.left(), inner.bottom() - timeline_h),
            inner.max,
        );
        let stage = if narrow {
            inner.width()
        } else {
            (inner.width() - RAIL_WIDTH - space::SIX).max(1.0)
        };
        let gap = if narrow { space::TWO } else { space::FOUR };
        let plate_h = if narrow {
            2.0 * PLATE_HEIGHT + space::ONE
        } else {
            PLATE_HEIGHT
        };
        let below = gap + plate_h + gap + COMPOSER_HEIGHT;
        let floor = if narrow {
            inner.bottom() - 220.0
        } else {
            timeline.top() - space::FOUR
        };
        let available_h = (floor - top - below).max(120.0);
        let picture_h = (stage * 9.0 / 16.0).min(available_h);
        let picture_w = if narrow {
            stage
        } else {
            (picture_h * 16.0 / 9.0).min(stage)
        };
        let picture = Rect::from_min_size(
            egui::pos2(inner.left() + (stage - picture_w) / 2.0, top),
            egui::vec2(picture_w, picture_h),
        );
        let plate = Rect::from_min_size(
            egui::pos2(picture.left(), picture.bottom() + gap),
            egui::vec2(picture_w, plate_h),
        );
        let composer_width = if narrow {
            picture_w
        } else {
            picture_w.min(COMPOSER_WIDTH)
        };
        let composer = Rect::from_min_size(
            egui::pos2(
                picture.center().x - composer_width / 2.0,
                plate.bottom() + gap,
            ),
            egui::vec2(composer_width, COMPOSER_HEIGHT),
        );
        let rail = if narrow {
            Rect::from_min_max(
                egui::pos2(inner.left(), composer.bottom() + space::THREE),
                inner.max,
            )
        } else {
            Rect::from_min_max(
                egui::pos2(inner.left() + stage + space::SIX, picture.top()),
                egui::pos2(inner.right(), composer.bottom()),
            )
        };
        Self {
            narrow,
            header,
            picture,
            plate,
            composer,
            rail,
            timeline,
        }
    }
}
fn at(ui: &mut egui::Ui, id: &'static str, rect: Rect, f: impl FnOnce(&mut egui::Ui)) {
    ui.scope_builder(egui::UiBuilder::new().id_salt(id).max_rect(rect), f);
}
pub(crate) fn fit(rect: Rect, size: Vec2) -> Rect {
    let scale = (rect.width() / size.x.max(1.0)).min(rect.height() / size.y.max(1.0));
    Rect::from_center_size(rect.center(), size * scale)
}

impl KinewrightApp {
    pub(crate) fn prepare_studio_screenshot(
        &mut self,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let mut attachment =
            crate::ja1_attachment::Ja1Attachment::open(path).map_err(|error| error.to_string())?;
        let versions = attachment
            .studio_versions()
            .map_err(|error| error.to_string())?;
        let key = versions
            .first()
            .ok_or("screenshot bundle has no published version")?
            .clone();
        let context = Arc::new(
            attachment
                .studio_context(&key)
                .map_err(|error| error.to_string())?,
        );
        attachment.panel_open = false;
        attachment.view = None;
        self.focused_mut().ja1 = Some(attachment);
        self.focused_mut().studio.feedback.scene =
            std::env::var("KINEWRIGHT_SCREENSHOT_STUDIO_SCENE").ok();
        self.focused_mut().studio.versions = versions;
        self.focused_mut().studio.mode = Mode::Studio;
        self.install_studio_context(context)
    }
    pub(crate) fn update_owned_playback(
        &mut self,
        doc: Arc<Document>,
        change: kinewright_core::LiveAudioChange,
        position: TimeCode,
    ) -> bool {
        if self.focused().ja1.is_none() && self.focused().ja1_job.is_none() {
            return false;
        }
        let retarget = self
            .focused()
            .studio
            .manual_inputs
            .as_ref()
            .and_then(|inputs| inputs.with_document(Arc::clone(&doc)).ok())
            .map(Arc::new);
        if let Some(inputs) = retarget {
            self.focused_mut().studio.manual_inputs = Some(Arc::clone(&inputs));
            if change == kinewright_core::LiveAudioChange::None {
                let stamp = self.lut_publisher.set_certified_document(inputs);
                self.presenter.set_context(stamp.epoch, None);
                self.playback.seek(position);
                self.playback.request_frame(position);
                self.playing = false;
            } else {
                self.playback.update_audio(change, doc);
            }
        } else {
            self.presenter.suspend();
            self.playback.pause();
            self.playing = false;
            self.focused_mut().studio.manual_inputs = None;
            self.focused_mut().studio.failure = None;
        }
        true
    }
    pub(crate) fn program_document(&self) -> &Document {
        self.focused()
            .studio
            .selection
            .as_ref()
            .map_or(&self.focused().document, |context| &context.document)
    }
    pub(crate) fn program_position(&self) -> TimeCode {
        if self.focused().studio.selection.is_some() {
            self.focused().studio.position
        } else {
            self.focused().position
        }
    }
    pub(crate) fn set_program_position(&mut self, position: TimeCode) {
        if self.focused().studio.selection.is_some() {
            self.focused_mut().studio.position = position;
        } else {
            self.focused_mut().position = position;
        }
    }
    pub(crate) fn install_studio_context(
        &mut self,
        context: Arc<FrozenContext>,
    ) -> Result<(), String> {
        let inputs = context
            .playback_inputs()
            .map_err(|error| error.to_string())?;
        let key = context.snapshot.descriptor.key.clone();
        let stamp = self
            .lut_publisher
            .set_certified_document(Arc::clone(&inputs));
        self.presenter.set_context(
            stamp.epoch,
            Some(Arc::new(PresentationContext {
                snapshot: key,
                inputs,
            })),
        );
        let studio = &mut self.focused_mut().studio;
        studio.selection = Some(context);
        studio.position = TimeCode::ZERO;
        studio.failure = None;
        self.playing = false;
        self.playback.request_frame(TimeCode::ZERO);
        Ok(())
    }
    fn return_to_original_version(&mut self) {
        let Some(context) = self.focused().studio.return_to.clone() else {
            return;
        };
        let position = self.focused().studio.return_at;
        match self.install_studio_context(context) {
            Ok(()) => {
                self.focused_mut().studio.return_to = None;
                self.seek_to(position);
            }
            Err(error) => self.status = error,
        }
    }
    pub(crate) fn sync_studio_selection(&mut self) {
        let Some(attachment) = &mut self.focused_mut().ja1 else {
            return;
        };
        let current_scope = attachment.studio_playback_scope().ok();
        let manual = attachment.studio_manual_ready.take();
        let context = attachment.studio_ready.take();
        let versions = attachment.studio_versions_ready.take();
        let failure = attachment.studio_error_ready.take();
        let editing = attachment.studio_edit_pending && !attachment.read_only();
        if editing {
            attachment.studio_edit_pending = false;
        }
        if let Some(failure) = failure {
            self.focused_mut().studio.failure = Some(failure);
        }
        if let Some(versions) = versions {
            self.focused_mut().studio.versions = versions;
        }
        if let Some(context) = context
            && let Err(error) = self.install_studio_context(context)
        {
            self.focused_mut().studio.failure = Some(error);
        }
        if self.focused().studio.manual_scope != current_scope {
            self.focused_mut()
                .studio
                .manual_scope
                .clone_from(&current_scope);
            self.focused_mut().studio.manual_inputs = None;
        }
        if let Some((scope, inputs)) = manual
            && Some(scope) == current_scope
            && inputs.document() == &*self.focused().document
        {
            self.focused_mut().studio.manual_inputs = Some(Arc::clone(&inputs));
            if self.focused().studio.selection.is_none() {
                let stamp = self.lut_publisher.set_certified_document(inputs);
                self.presenter.set_context(stamp.epoch, None);
                self.playback.seek(self.focused().position);
                self.playback.request_frame(self.focused().position);
            }
        }
        if editing {
            self.presenter.suspend();
            self.playback.pause();
            self.playing = false;
            let studio = &mut self.focused_mut().studio;
            studio.return_at = studio.position;
            studio.return_to = studio.selection.take();
            studio.mode = Mode::Editor;
            self.focused_mut().studio.manual_inputs = None;
        }
        if self.focused().studio.selection.is_none()
            && self.focused().studio.manual_inputs.is_none()
            && self
                .focused()
                .ja1
                .as_ref()
                .is_some_and(|attachment| !attachment.read_only())
            && self.focused().ja1_job.is_none()
            && self.focused().studio.failure.is_none()
        {
            self.presenter.suspend();
            self.start_ja1(Action::StudioManualPlayback(Arc::clone(
                &self.focused().document,
            )));
        }
    }
    #[allow(clippy::too_many_lines, reason = "one immediate-mode layout pass")]
    pub(crate) fn studio_layout(&mut self, ui: &mut egui::Ui) {
        if !ui.ctx().egui_wants_keyboard_input() {
            for (index, key) in [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3]
                .into_iter()
                .enumerate()
            {
                if ui
                    .ctx()
                    .input_mut(|input| input.consume_key(egui::Modifiers::ALT, key))
                    && let Some(version) = self.focused().studio.versions.get(index)
                {
                    self.start_ja1(Action::StudioSelect(version.clone()));
                }
            }
            if ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::ALT, egui::Key::N))
            {
                self.focused_mut().studio.tab = Tab::Notes;
                let id = egui::Id::new(("studio-composer-input", self.focused().id));
                ui.ctx().memory_mut(|memory| memory.request_focus(id));
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(if self.focused().studio.neutral {
                Color32::BLACK
            } else {
                po0::BASE
            }))
            .show(ui, |ui| {
                po0_style(ui.style_mut());
                let layout = Layout::new(ui.max_rect(), self.focused().studio.mode == Mode::Editor);
                at(ui, "studio-header", layout.header, |ui| {
                    self.studio_header(ui, &layout);
                });
                at(ui, "studio-picture", layout.picture, |ui| {
                    self.studio_picture(ui);
                });
                at(ui, "studio-plate", layout.plate, |ui| {
                    self.studio_plate(ui, layout.narrow);
                });
                at(ui, "studio-composer", layout.composer, |ui| {
                    self.studio_composer(ui);
                });
                if layout.narrow {
                    at(ui, "studio-narrow-tabs", layout.rail, |ui| {
                        self.studio_tabs(ui, true);
                        match self.focused().studio.tab {
                            Tab::Timeline => {
                                let rect = Rect::from_min_size(
                                    ui.cursor().min,
                                    egui::vec2(ui.available_width(), STUDIO_TIMELINE_HEIGHT),
                                );
                                ui.allocate_rect(rect, egui::Sense::hover());
                                self.studio_timeline(ui, rect);
                            }
                            Tab::Compare => self.studio_compare(ui),
                            _ => self.studio_rail_content(ui),
                        }
                    });
                } else {
                    paint_rail(ui.painter(), layout.rail);
                    at(ui, "studio-rail", layout.rail.shrink(10.0), |ui| {
                        self.studio_tabs(ui, false);
                        self.studio_rail_content(ui);
                    });
                    at(ui, "studio-timeline", layout.timeline, |ui| {
                        self.studio_timeline(ui, layout.timeline);
                    });
                }
            });
        let mut open = self.focused().studio.compare_open;
        if open {
            egui::Window::new("A/B · paused versions")
                .open(&mut open)
                .default_width(720.0)
                .show(ui.ctx(), |ui| self.studio_compare(ui));
        }
        self.focused_mut().studio.compare_open = open;
        let mut versions_open = self.focused().studio.versions_open;
        if versions_open {
            let versions = self.focused().studio.versions.clone();
            let mut selected = None;
            egui::Window::new("Versions")
                .open(&mut versions_open)
                .show(ui.ctx(), |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if versions.is_empty() {
                            ui.label("No published versions yet");
                        }
                        for (index, version) in versions.iter().enumerate() {
                            if ui
                                .button(format!(
                                    "Version {} · {}",
                                    index + 1,
                                    version.version.as_str()
                                ))
                                .clicked()
                            {
                                selected = Some(version.clone());
                            }
                        }
                    });
                });
            if let Some(key) = selected {
                self.start_ja1(Action::StudioSelect(key));
                versions_open = false;
            }
        }
        self.focused_mut().studio.versions_open = versions_open;
    }
    fn studio_header(&mut self, ui: &mut egui::Ui, layout: &Layout) {
        let rect = ui.max_rect();
        let mode = self.focused().studio.mode;
        let modes = Rect::from_center_size(
            egui::pos2(layout.picture.center().x, rect.center().y),
            egui::vec2(segmented_width(ui, &["Studio", "Editor"]), 32.0),
        );
        let mark = Rect::from_min_size(rect.min, egui::vec2(28.0, rect.height()));
        at_with(ui, "studio-mark", mark, centered_row(), |ui| {
            ui.label(theme::wordmark("K", po0::TEXT));
        });
        let title_right = (modes.left() - space::FOUR).max(mark.right() + 64.0);
        let title = Rect::from_min_max(
            egui::pos2(mark.right() + space::ONE, rect.center().y - 17.5),
            egui::pos2(title_right, rect.center().y + 17.5),
        );
        let eyebrow = self.studio_eyebrow();
        at(ui, "studio-title", title, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add(
                egui::Label::new(
                    egui::RichText::new(eyebrow.to_uppercase())
                        .font(theme::semibold(10.5))
                        .extra_letter_spacing(0.7)
                        .color(po0::TEXT_3),
                )
                .truncate(),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(self.focused().display_name())
                        .font(theme::semibold(17.0))
                        .color(po0::TEXT),
                )
                .truncate(),
            );
        });
        at_with(ui, "studio-mode", modes, centered_row(), |ui| {
            if let Some(mode) = segmented(
                ui,
                &[(Mode::Studio, "Studio"), (Mode::Editor, "Editor")],
                mode,
            ) {
                self.focused_mut().studio.mode = mode;
            }
        });
        let actions = Rect::from_min_max(
            egui::pos2(
                if layout.narrow {
                    rect.right() - 96.0
                } else {
                    layout.rail.left()
                },
                rect.top(),
            ),
            rect.max,
        );
        at_with(
            ui,
            "studio-actions",
            actions,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = space::ONE;
                if layout.narrow {
                    ui.menu_button(
                        egui::RichText::new("Project").font(theme::medium(12.5)),
                        |ui| self.studio_actions(ui),
                    );
                } else {
                    self.studio_actions(ui);
                }
            },
        );
    }
    /// The eyebrow names the kind of work (PO0 §2 Type). Nothing records a project
    /// kind yet, so it names the picture's format instead.
    fn studio_eyebrow(&self) -> String {
        let document = self.program_document();
        let (width, height) = document.resolution;
        let fps = crate::timeline_ui::nominal_fps(document.fps);
        if width == 0 || height == 0 {
            format!("{fps} fps")
        } else {
            format!("{width} × {height} · {fps} fps")
        }
    }
    fn studio_actions(&mut self, ui: &mut egui::Ui) {
        // Right to left: the primary action sits at the rail's right edge.
        if let Some(context) = &self.focused().studio.selection {
            let key = context.snapshot.descriptor.key.clone();
            if quiet_button(ui, "Edit a copy", true, true).clicked() {
                self.start_ja1(Action::StudioEditCopy(key));
            }
        } else if quiet_button(ui, "Save", true, true).clicked() {
            self.save_project(false);
        }
        if quiet_button(ui, "Versions", true, false).clicked() {
            self.focused_mut().studio.versions_open = true;
            if self.focused().ja1.is_none() {
                self.enable_ja1();
            }
            self.start_ja1(Action::StudioVersions);
        }
        let manual_ready = self
            .focused()
            .ja1
            .as_ref()
            .is_none_or(|attachment| !attachment.read_only());
        if quiet_button(ui, "Current edit", manual_ready, false).clicked()
            && self.focused().studio.selection.is_some()
        {
            self.focused_mut().studio.selection = None;
            if self.focused().studio.failure.is_some() {
                self.focused_mut().studio.manual_inputs = None;
            }
            self.focused_mut().studio.failure = None;
            self.presenter.suspend();
            self.playback.pause();
            self.playing = false;
            if let Some(inputs) = self.focused().studio.manual_inputs.clone() {
                let stamp = self.lut_publisher.set_certified_document(inputs);
                self.presenter.set_context(stamp.epoch, None);
                self.playback.seek(self.focused().position);
                self.playback.request_frame(self.focused().position);
            } else if self.focused().ja1.is_some() {
                self.start_ja1(Action::StudioManualPlayback(Arc::clone(
                    &self.focused().document,
                )));
            } else {
                self.playback
                    .set_document(Arc::clone(&self.focused().document));
                self.presenter
                    .set_context(self.playback.stamp().epoch, None);
                self.playback.request_frame(self.focused().position);
            }
        }
    }
    fn studio_picture(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        // depth-picture: two shadows and a hairline, all outside the frame.
        for (y, blur, alpha) in [(28, 70, 140), (6, 18, 89)] {
            ui.painter().add(
                egui::Shadow {
                    offset: [0, y],
                    blur,
                    spread: 0,
                    color: Color32::from_black_alpha(alpha),
                }
                .as_shape(rect, 4),
            );
        }
        ui.painter().rect(
            rect,
            4,
            Color32::BLACK,
            egui::Stroke::new(1.0, po0::LINE),
            egui::StrokeKind::Outside,
        );
        if let Some((texture, size)) = self.program_picture() {
            let image = fit(rect, size);
            ui.painter().image(
                texture,
                image,
                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            let epoch = ui.ctx().cumulative_frame_nr_for(egui::ViewportId::ROOT);
            self.presenter
                .marker(epoch, self.paint_clock())
                .add_to(ui.painter(), image);
            self.feedback_overlay(ui, rect, size);
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Add media to begin",
                egui::FontId::proportional(17.0),
                po0::TEXT_2,
            );
        }
    }
    /// The selected version's letter: its place among the first three versions.
    fn selected_letter(&self) -> Option<char> {
        let selection = self.focused().studio.selection.as_ref()?;
        let index = self
            .focused()
            .studio
            .versions
            .iter()
            .take(3)
            .position(|version| *version == selection.snapshot.descriptor.key)?;
        Some(['A', 'B', 'C'][index])
    }
    /// The plate's label block: a human name, then a dot and a status line.
    /// The version identity stays in the tooltip for diagnostics.
    fn plate_label(&self) -> (String, Color32, String, String) {
        let studio = &self.focused().studio;
        let document = self.program_document();
        let (length, _) = clock(document.duration, document.fps);
        let name = match (&studio.selection, self.selected_letter()) {
            (None, _) => "Current edit".to_owned(),
            (Some(_), Some(letter)) => format!("Version {letter}"),
            (Some(context), None) => studio
                .versions
                .iter()
                .position(|version| *version == context.snapshot.descriptor.key)
                .map_or_else(
                    || "Version".to_owned(),
                    |index| format!("Version {}", index + 1),
                ),
        };
        let bound = self.presenter.bound();
        let mut tip = Vec::new();
        if let Some(context) = &studio.selection {
            tip.push(format!(
                "Version {}",
                context.snapshot.descriptor.key.version.as_str()
            ));
        }
        if let Some(cell) = &bound
            && let Some(context) = &cell.context
        {
            tip.push(format!(
                "Showing {} · {}",
                context.snapshot.version.as_str(),
                crate::timeline_ui::format_timecode(cell.at, context.inputs.document().fps),
            ));
        }
        let (dot, status) = if let Some(error) = &studio.failure {
            (po0::DANGER, format!("Damaged or unavailable · {error}"))
        } else if studio.selection.is_some() && bound.is_some_and(|cell| cell.stale) {
            (po0::TEXT_3, "Loading this version…".to_owned())
        } else if studio.selection.is_some() {
            (po0::WARNING, format!("{length} · Unreviewed"))
        } else {
            (po0::TEXT_3, format!("{length} · Not published"))
        };
        (name, dot, status, tip.join("\n"))
    }
    fn studio_plate(&mut self, ui: &mut egui::Ui, narrow: bool) {
        let rect = ui.max_rect();
        let row = Rect::from_min_size(rect.min, egui::vec2(rect.width(), PLATE_HEIGHT));
        let second = row.translate(egui::vec2(0.0, PLATE_HEIGHT + space::ONE));
        let transport_row = if narrow { second } else { row };
        let transport = Rect::from_center_size(
            transport_row.center(),
            egui::vec2(32.0 + 36.0 + 32.0 + 2.0 * space::ONE, PLATE_HEIGHT),
        );
        let tools = Rect::from_min_max(
            egui::pos2(transport.right() + space::TWO, transport_row.top()),
            transport_row.max,
        );
        let (left, clock_area) = if narrow {
            let split = row.right() - 112.0;
            (
                Rect::from_min_max(row.min, egui::pos2(split - space::TWO, row.bottom())),
                Rect::from_min_max(egui::pos2(split, row.top()), row.max),
            )
        } else {
            (
                Rect::from_min_max(
                    row.min,
                    egui::pos2(transport.left() - space::TWO, row.bottom()),
                ),
                tools,
            )
        };
        at_with(ui, "studio-plate-label", left, centered_row(), |ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            self.version_pill(ui);
            let (name, dot, status, tip) = self.plate_label();
            plate_label(ui, &name, dot, &status, &tip);
        });
        at_with(ui, "studio-transport", transport, centered_row(), |ui| {
            ui.spacing_mut().item_spacing.x = space::ONE;
            if round_button(
                ui,
                Icon::StepBack,
                "Back",
                "Previous frame · Left",
                32.0,
                Round::Quiet,
                true,
            )
            .clicked()
            {
                self.seek_to(TimeCode(self.program_position().0.saturating_sub(1)));
            }
            let (icon, label) = if self.playing {
                (Icon::Pause, "Pause")
            } else {
                (Icon::Play, "Play")
            };
            if round_button(ui, icon, label, "Space", 36.0, Round::Tinted, true).clicked() {
                self.toggle_playback();
            }
            if round_button(
                ui,
                Icon::StepForward,
                "Next",
                "Next frame · Right",
                32.0,
                Round::Quiet,
                true,
            )
            .clicked()
            {
                self.seek_to(TimeCode(self.program_position().0.saturating_add(1)));
            }
        });
        let document = self.program_document();
        let (now, frames) = clock(self.program_position(), document.fps);
        let (length, length_frames) = clock(document.duration, document.fps);
        at_with(
            ui,
            "studio-plate-clock",
            clock_area,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if !narrow {
                    self.plate_tools(ui);
                    ui.add_space(space::TWO);
                }
                timecode_block(ui, &now, &frames, &format!("of {length}:{length_frames}"));
            },
        );
        if narrow {
            at_with(
                ui,
                "studio-plate-tools",
                tools,
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    self.plate_tools(ui);
                },
            );
        }
    }
    /// Point stays inert until C6; Draw starts a note on the bound picture (C3).
    fn plate_tools(&mut self, ui: &mut egui::Ui) {
        let neutral = self.focused().studio.neutral;
        if round_button(
            ui,
            Icon::Neutral,
            "Neutral",
            "Neutral surround",
            32.0,
            if neutral {
                Round::Selected
            } else {
                Round::Quiet
            },
            true,
        )
        .clicked()
        {
            self.focused_mut().studio.neutral = !neutral;
        }
        round_button(
            ui,
            Icon::Point,
            "Point",
            "Point at a person · not available yet",
            32.0,
            Round::Quiet,
            false,
        );
        let drawing = self.focused().studio.feedback.drawing;
        if round_button(
            ui,
            Icon::Draw,
            "Draw",
            "Draw on the frame",
            32.0,
            if drawing {
                Round::Selected
            } else {
                Round::Quiet
            },
            true,
        )
        .clicked()
        {
            self.focused_mut().studio.feedback.drawing = !drawing;
        }
    }
    /// The A/B/C segmented pill, with the A/B tools in a menu at its end.
    fn version_pill(&mut self, ui: &mut egui::Ui) {
        let versions = self.focused().studio.versions.clone();
        seg_track(ui, |ui| {
            for (index, version) in versions.iter().take(3).enumerate() {
                let label = ['A', 'B', 'C'][index].to_string();
                let selected = self
                    .focused()
                    .studio
                    .selection
                    .as_ref()
                    .is_some_and(|c| c.snapshot.descriptor.key == *version);
                if seg_item(ui, &label, selected, Seg::Version)
                    .on_hover_text(version.version.as_str())
                    .clicked()
                {
                    self.start_ja1(Action::StudioSelect(version.clone()));
                }
            }
            let menu = round_button(
                ui,
                Icon::ChevronDown,
                "A/B",
                "Keep stills and compare A/B",
                26.0,
                Round::Quiet,
                true,
            );
            egui::Popup::menu(&menu).show(|ui| {
                for (slot, label) in [(0, "Keep A still"), (1, "Keep B still")] {
                    if ui.button(label).clicked() {
                        self.capture_studio_still(slot, ui.ctx());
                        ui.close();
                    }
                }
                if ui.button("Compare A/B").clicked() {
                    self.focused_mut().studio.compare_open = true;
                    ui.close();
                }
                if self.focused().studio.return_to.is_some()
                    && ui.button("Return to original version").clicked()
                {
                    self.return_to_original_version();
                    ui.close();
                }
            });
        });
    }
    fn studio_composer(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().add(
            egui::Shadow {
                offset: [0, 12],
                blur: 32,
                spread: 0,
                color: Color32::from_black_alpha(82),
            }
            .as_shape(rect, 26),
        );
        ui.painter().rect(
            rect,
            26,
            po0::COMPOSER,
            egui::Stroke::new(1.0, po0::LINE),
            egui::StrokeKind::Inside,
        );
        top_sheen(ui.painter(), rect, 26.0);
        let letter = self.selected_letter();
        let fps = self.program_document().fps;
        let at = self.program_position();
        at_with(
            ui,
            "studio-composer-row",
            rect.shrink(10.0),
            centered_row(),
            |ui| {
                ui.spacing_mut().item_spacing.x = space::ONE;
                time_chip(ui, at, fps, letter);
                ui.add_space(space::ONE);
                let snapshot = self
                    .presenter
                    .bound()
                    .and_then(|cell| cell.context.map(|c| c.snapshot.clone()));
                let input_id = egui::Id::new(("studio-composer-input", self.focused().id));
                let width = (ui.available_width() - 2.0 * 32.0 - 3.0 * space::ONE).max(40.0);
                let thread = self.active_thread_mut();
                let response = ui.add_sized(
                    [width, 32.0],
                    egui::TextEdit::singleline(&mut thread.input)
                        .id(input_id)
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 8)))
                        .font(egui::FontId::proportional(13.0))
                        .text_color(po0::TEXT)
                        .hint_text(
                            egui::RichText::new("Leave a note at this moment, or ask the team…")
                                .color(po0::TEXT_3),
                        ),
                );
                if response.changed()
                    && self.focused().studio.draft_snapshot.is_none()
                    && !self.active_thread().input.is_empty()
                {
                    self.focused_mut().studio.draft_snapshot = snapshot;
                }
                if response.changed() && self.active_thread().input.is_empty() {
                    self.focused_mut().studio.draft_snapshot = None;
                }
                // C6 wires voice notes to this slot.
                round_button(
                    ui,
                    Icon::Mic,
                    "Voice note",
                    "Voice notes · not available yet",
                    32.0,
                    Round::Quiet,
                    false,
                );
                let can_send = !self.focused().ja1_read_only()
                    && !self.active_thread().input.trim().is_empty();
                if round_button(
                    ui,
                    Icon::ArrowUp,
                    "Send",
                    "Send",
                    32.0,
                    Round::Send,
                    can_send,
                )
                .clicked()
                {
                    self.start_agent_turn();
                }
            },
        );
    }
    fn studio_tabs(&mut self, ui: &mut egui::Ui, narrow: bool) {
        let tabs: Vec<(Tab, &str)> = [
            (Tab::Commentary, "Commentary"),
            (Tab::Notes, "Notes"),
            (Tab::Timeline, "Timeline"),
            (Tab::Compare, "A/B"),
        ]
        .into_iter()
        .filter(|(tab, _)| narrow || matches!(tab, Tab::Commentary | Tab::Notes))
        .collect();
        let current = self.focused().studio.tab;
        ui.horizontal(|ui| {
            if let Some(tab) = segmented(ui, &tabs, current) {
                self.focused_mut().studio.tab = tab;
            }
        });
        ui.add_space(space::THREE);
    }
    fn studio_rail_content(&mut self, ui: &mut egui::Ui) {
        if self.focused().studio.tab == Tab::Notes {
            if let Some(snapshot) = &self.focused().studio.draft_snapshot {
                ui.small(format!("Draft on {}", snapshot.version.as_str()));
                if !self.active_thread().input.is_empty() {
                    ui.label(&self.active_thread().input);
                }
            }
            self.studio_notes(ui);
            return;
        }
        let count = self.active_thread().chat.len();
        let studio = &mut self.focused_mut().studio;
        if count > studio.seen && !studio.following {
            studio.unseen += count - studio.seen;
        }
        studio.seen = count;
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("studio-commentary")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 36.0).max(40.0))
            .stick_to_bottom(studio.following);
        if std::mem::take(&mut studio.jump) {
            scroll = scroll.vertical_scroll_offset(f32::INFINITY);
        }
        let mut card_action = None;
        let frozen = self.focused().studio.selection.is_some();
        let output = scroll.show(ui, |ui| {
            let thread = self.active_thread();
            if thread.chat.is_empty() {
                ui.colored_label(po0::TEXT_3, "The conversation will appear here.");
            }
            for (index, entry) in thread.chat.iter().enumerate() {
                if frozen
                    && let crate::chat_ui::ChatEntry::EditCard {
                        summary,
                        start,
                        end,
                        ..
                    } = entry
                {
                    ui.label(format!(
                        "Current edit · {summary} (frames {}–{})",
                        start.0, end.0
                    ));
                } else {
                    crate::chat_ui::render_stream_entry(
                        ui,
                        entry,
                        index,
                        self.program_document().fps,
                        &mut card_action,
                    );
                }
            }
        });
        match card_action {
            Some(crate::chat_ui::EditCardAction::Review(cue)) => {
                self.seek_to(cue);
                self.play_program_from(self.program_position());
            }
            Some(crate::chat_ui::EditCardAction::Undo) => self.undo(),
            None => {}
        }
        let bottom =
            output.state.offset.y + output.inner_rect.height() >= output.content_size.y - 4.0;
        self.focused_mut().studio.following = bottom;
        if bottom {
            self.focused_mut().studio.unseen = 0;
        }
        let unseen = self.focused().studio.unseen;
        if unseen > 0
            && ui
                .vertical_centered(|ui| {
                    ui.add(
                        egui::Button::new(
                            egui::RichText::new(format!("{unseen} new ↓"))
                                .font(theme::semibold(12.0))
                                .color(Color32::WHITE),
                        )
                        .fill(po0::ACCENT_FILL)
                        .corner_radius(14)
                        .min_size(egui::vec2(0.0, 28.0)),
                    )
                })
                .inner
                .clicked()
        {
            self.focused_mut().studio.jump = true;
        }
    }
    /// A read-only clip-block lane under a time ruler. Clicking or dragging seeks.
    /// PO0's filmstrip, waveform, role hues and playhead restyle wait for the probe.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "frame counts map to screen points well inside f32 precision"
    )]
    fn studio_timeline(&mut self, ui: &mut egui::Ui, rect: Rect) {
        paint_well(&ui.painter_at(rect.expand(40.0)), rect);
        let painter = ui.painter_at(rect);
        let content = rect.shrink2(egui::vec2(space::FOUR, 0.0));
        let document = self.program_document();
        let duration = document.duration.0.max(1);
        let x_of = |frame: i64| {
            content.left() + content.width() * (frame.clamp(0, duration) as f32 / duration as f32)
        };
        let ruler = Rect::from_min_size(content.min, egui::vec2(content.width(), 24.0));
        paint_ruler(&painter, ruler, duration, document.fps);
        let picture_lane = Rect::from_min_size(
            egui::pos2(content.left(), ruler.bottom() + 6.0),
            egui::vec2(content.width(), 40.0),
        );
        let sound_lane = Rect::from_min_size(
            egui::pos2(content.left(), picture_lane.bottom() + 6.0),
            egui::vec2(content.width(), 22.0),
        );
        let position = self.program_position().0;
        let mut clips = 0;
        for track in &document.tracks {
            let sound = track.kind == kinewright_core::TrackKind::Audio;
            let lane = if sound { sound_lane } else { picture_lane };
            for clip in &track.clips {
                let start = clip.timeline_start.0;
                let end = start + clip.source_range.end.0 - clip.source_range.start.0;
                let block =
                    Rect::from_x_y_ranges(x_of(start) + 1.0..=x_of(end) - 1.0, lane.y_range());
                if block.width() < 1.0 {
                    continue;
                }
                clips += 1;
                let fill = if sound {
                    po0::CLIP_AUDIO
                } else if (start..end).contains(&position) {
                    po0::CLIP_CURRENT
                } else {
                    po0::CLIP
                };
                painter.rect(
                    block,
                    5,
                    fill,
                    egui::Stroke::new(1.0, Color32::from_white_alpha(18)),
                    egui::StrokeKind::Inside,
                );
                if block.width() > 28.0 {
                    let (font, ink, inset) = if sound {
                        (theme::medium(10.0), po0::TEXT_3, 4.0)
                    } else {
                        (theme::semibold(11.0), po0::CLIP_LABEL, 5.0)
                    };
                    painter.with_clip_rect(block.shrink(2.0)).text(
                        egui::pos2(block.left() + 8.0, block.top() + inset),
                        egui::Align2::LEFT_TOP,
                        clip_label(document, clip),
                        font,
                        ink,
                    );
                }
            }
        }
        if clips == 0 {
            painter.text(
                picture_lane.center(),
                egui::Align2::CENTER_CENTER,
                "Takes land here. The story of the cut grows along this line.",
                egui::FontId::proportional(12.0),
                po0::TEXT_3,
            );
        }
        let x = x_of(position);
        painter.vline(
            x,
            egui::Rangef::new(ruler.top() + 4.0, rect.bottom() - 6.0),
            egui::Stroke::new(2.0, po0::TEXT),
        );
        painter.circle_filled(egui::pos2(x, ruler.top() + 6.0), 4.0, po0::TEXT);
        let scrub = Rect::from_x_y_ranges(content.x_range(), rect.y_range());
        let response = ui.interact(
            scrub,
            ui.id().with("studio-timeline-scrub"),
            egui::Sense::click_and_drag(),
        );
        response.widget_info(|| egui::WidgetInfo::slider(true, position as f64, "Frame"));
        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let fraction = ((pointer.x - content.left()) / content.width()).clamp(0.0, 1.0);
            let last = (duration - 1).max(0);
            let frame = ((fraction * duration as f32).round() as i64).min(last);
            if frame != position {
                self.seek_to(TimeCode(frame));
            }
        }
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "dimensions come from admitted bounded integer image rasters"
    )]
    fn capture_studio_still(&mut self, slot: usize, ctx: &egui::Context) {
        let Some(cell) = self
            .presenter
            .bound()
            .filter(|cell| !cell.stale && cell.context.is_some() && !self.playing)
        else {
            self.status = "Pause on a presented version before keeping a still.".into();
            return;
        };
        let Some(context) = self.focused().studio.selection.as_ref() else {
            return;
        };
        let (width, height) = self
            .program_picture()
            .map_or((0, 0), |(_, size)| (size.x as u32, size.y as u32));
        let bytes = u64::from(width) * u64::from(height) * 8;
        let charge = match context.reserve_frame(bytes) {
            Ok(charge) => charge,
            Err(error) => {
                self.status = error.to_string();
                return;
            }
        };
        let image = if self.presenter.using_gpu() {
            let Some(display) = &self.native_display else {
                return;
            };
            let Some((stamp, at, texture)) = display
                .session
                .copy_bound(&display.renderer.device, &display.renderer.queue)
            else {
                return;
            };
            if (stamp, at) != (cell.stamp, cell.at) {
                return;
            }
            let view = texture.create_view(&eframe::wgpu::TextureViewDescriptor::default());
            let id = display.renderer.renderer.write().register_native_texture(
                &display.renderer.device,
                &view,
                eframe::wgpu::FilterMode::Linear,
            );
            StillImage::Gpu {
                id,
                size: egui::vec2(width as f32, height as f32),
                _texture: texture,
                renderer: display.renderer.clone(),
            }
        } else {
            let Some(frame) = self.focused().studio.cpu_frame.as_ref() else {
                return;
            };
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            StillImage::Cpu(ctx.load_texture(
                format!("studio-still-{slot}"),
                image,
                egui::TextureOptions::LINEAR,
            ))
        };
        self.focused_mut().studio.stills[slot] = Some(Still {
            cell,
            image,
            _charge: charge,
        });
    }
    fn studio_compare(&self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            for (slot, column) in columns.iter_mut().enumerate() {
                column.label(if slot == 0 {
                    "A · paused still"
                } else {
                    "B · paused still"
                });
                if let Some(still) = &self.focused().studio.stills[slot] {
                    column.add(
                        egui::Image::new(still.image.image()).max_width(column.available_width()),
                    );
                    let version = still
                        .cell
                        .context
                        .as_ref()
                        .map_or("Current edit", |c| c.snapshot.version.as_str());
                    column.small(format!("{version} · frame {}", still.cell.at.0));
                }
            }
        });
        ui.small("Separate timecodes · no verified unique source moment alignment");
        if let [Some(a), Some(b)] = &self.focused().studio.stills
            && let (Some(a), Some(b)) = (&a.cell.context, &b.cell.context)
        {
            let (a, b) = (a.inputs.document(), b.inputs.document());
            ui.label(format!(
                "Duration: {} → {} frames",
                a.duration.0, b.duration.0
            ));
            if a.resolution != b.resolution {
                ui.label(format!(
                    "Resolution: {:?} → {:?}",
                    a.resolution, b.resolution
                ));
            }
            if a.lut_assets != b.lut_assets {
                ui.label("Colour look resources changed");
            }
            if a.media_pool != b.media_pool {
                ui.label("Source media or interpretation changed");
            }
            if let Some(range) = crate::edit_diff::changed_project_range(a, b) {
                ui.label(format!(
                    "Changed range in B: {}–{}",
                    range.start.0, range.end.0
                ));
            }
        }
    }
}

fn at_with(
    ui: &mut egui::Ui,
    id: &'static str,
    rect: Rect,
    layout: egui::Layout,
    f: impl FnOnce(&mut egui::Ui),
) {
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(layout),
        f,
    );
}
fn centered_row() -> egui::Layout {
    egui::Layout::left_to_right(egui::Align::Center)
}
/// Studio's widget surfaces in PO0's neutral fills, with blue as the only accent.
fn po0_style(style: &mut egui::Style) {
    let visuals = &mut style.visuals;
    visuals.override_text_color = Some(po0::TEXT);
    visuals.weak_text_color = Some(po0::TEXT_2);
    visuals.selection.bg_fill = po0::YOU_TINT;
    visuals.selection.stroke = egui::Stroke::new(1.0, po0::YOU);
    visuals.hyperlink_color = po0::YOU;
    visuals.text_edit_bg_color = Some(po0::FILL);
    for (widget, fill) in [
        (&mut visuals.widgets.inactive, po0::FILL),
        (&mut visuals.widgets.hovered, po0::FILL_2),
        (&mut visuals.widgets.active, po0::FILL_3),
        (&mut visuals.widgets.open, po0::FILL_2),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = egui::Stroke::NONE;
        widget.corner_radius = egui::CornerRadius::same(14);
    }
}
/// `spec-top`: a 1 pt white edge along the top of a raised surface.
fn top_sheen(painter: &egui::Painter, rect: Rect, corner: f32) {
    painter.hline(
        egui::Rangef::new(rect.left() + corner, rect.right() - corner),
        rect.top() + 0.5,
        egui::Stroke::new(1.0, po0::SHEEN),
    );
}
fn paint_rail(painter: &egui::Painter, rect: Rect) {
    painter.add(
        egui::Shadow {
            offset: [0, 16],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(77),
        }
        .as_shape(rect, 16),
    );
    painter.rect(
        rect,
        16,
        po0::RAIL,
        egui::Stroke::new(1.0, po0::LINE),
        egui::StrokeKind::Inside,
    );
    top_sheen(painter, rect, 16.0);
}
/// The timeline surface: `well` with `depth-fill` and a top sheen.
fn paint_well(painter: &egui::Painter, rect: Rect) {
    painter.add(
        egui::Shadow {
            offset: [0, 10],
            blur: 30,
            spread: 0,
            color: Color32::from_black_alpha(71),
        }
        .as_shape(rect, 12),
    );
    painter.rect(
        rect,
        12,
        po0::WELL,
        egui::Stroke::new(1.0, Color32::from_white_alpha(9)),
        egui::StrokeKind::Inside,
    );
    top_sheen(painter, rect, 12.0);
}
/// The time ruler: `type-micro` labels in `text-3` with major and minor ticks.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "frame counts map to screen points well inside f32 precision"
)]
fn paint_ruler(
    painter: &egui::Painter,
    ruler: Rect,
    duration: i64,
    rate: kinewright_core::Rational,
) {
    let fps = crate::timeline_ui::nominal_fps(rate).max(1);
    let seconds = duration as f32 / fps as f32;
    let step = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600]
        .into_iter()
        .find(|step| ruler.width() * *step as f32 / seconds >= 84.0)
        .unwrap_or(7200);
    let minor = step as f32 / 4.0;
    let mut tick = 0.0_f32;
    while tick <= seconds {
        let x = ruler.left() + ruler.width() * tick / seconds;
        let major = (tick / step as f32).fract().abs() < 1e-3;
        painter.vline(
            x,
            egui::Rangef::new(
                ruler.bottom() - if major { 7.0 } else { 5.0 },
                ruler.bottom() - 2.0,
            ),
            egui::Stroke::new(1.0, Color32::from_white_alpha(if major { 46 } else { 26 })),
        );
        if major {
            let (label, _) = clock(TimeCode(tick as i64 * i64::from(fps)), rate);
            let (x, anchor) = if tick == 0.0 {
                (x + 8.0, egui::Align2::LEFT_TOP)
            } else {
                (x, egui::Align2::CENTER_TOP)
            };
            if x + 20.0 <= ruler.right() {
                painter.text(
                    egui::pos2(x, ruler.top() + 7.0),
                    anchor,
                    label,
                    egui::FontId::monospace(10.0),
                    po0::TEXT_3,
                );
            }
        }
        tick += minor;
    }
}
/// `mm:ss` (or `h:mm:ss`) and the frame digits, from the editor's `hh:mm:ss:ff`.
fn clock(at: TimeCode, fps: kinewright_core::Rational) -> (String, String) {
    let full = crate::timeline_ui::format_timecode(at, fps);
    let (time, frames) = full.rsplit_once(':').unwrap_or((full.as_str(), "00"));
    let time = time.strip_prefix("00:").unwrap_or(time);
    (time.to_owned(), frames.to_owned())
}
fn clip_label(document: &Document, clip: &kinewright_core::Clip) -> String {
    match &clip.content {
        kinewright_core::ClipContent::Media => document
            .media_pool
            .iter()
            .find(|asset| asset.id == clip.asset)
            .map_or_else(
                || "Clip".to_owned(),
                |asset| {
                    std::path::Path::new(&asset.name).file_stem().map_or_else(
                        || asset.name.clone(),
                        |stem| stem.to_string_lossy().into_owned(),
                    )
                },
            ),
        kinewright_core::ClipContent::Title(_) => "Title".to_owned(),
        kinewright_core::ClipContent::Freeze(_) => "Freeze frame".to_owned(),
        kinewright_core::ClipContent::Solid(_) => "Colour".to_owned(),
        kinewright_core::ClipContent::Adjustment => "Adjustment".to_owned(),
        #[allow(unreachable_patterns, reason = "future clip kinds read as plain clips")]
        _ => "Clip".to_owned(),
    }
}
fn quiet_button(ui: &mut egui::Ui, label: &str, enabled: bool, tinted: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).font(theme::medium(12.5)))
            .frame_when_inactive(tinted)
            .corner_radius(14)
            .min_size(egui::vec2(0.0, 28.0)),
    )
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seg {
    Choice,
    Version,
}
fn segmented_width(ui: &egui::Ui, labels: &[&str]) -> f32 {
    labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(
                    (*label).to_owned(),
                    theme::medium(12.0),
                    Color32::PLACEHOLDER,
                )
                .size()
                .x
                + 28.0
        })
        .sum::<f32>()
        + 6.0
}
fn seg_track<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(po0::TRACK)
        .corner_radius(16)
        .inner_margin(3)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(add).inner
        })
        .inner
}
/// A segmented control; selection reads in the person's blue (PO0 §2 Accent).
fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, items: &[(T, &str)], current: T) -> Option<T> {
    seg_track(ui, |ui| {
        let mut picked = None;
        for &(value, label) in items {
            if seg_item(ui, label, value == current, Seg::Choice).clicked() {
                picked = Some(value);
            }
        }
        picked
    })
}
fn seg_item(ui: &mut egui::Ui, label: &str, selected: bool, kind: Seg) -> egui::Response {
    let font = match kind {
        Seg::Version => theme::semibold(12.0),
        Seg::Choice => theme::medium(12.0),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let width = match kind {
        Seg::Version => 30.0,
        Seg::Choice => galley.size().x + 28.0,
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, enabled, selected, label)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hot = response.hovered();
        match (selected, kind) {
            (true, Seg::Version) => {
                let fill = if hot {
                    po0::ACCENT_FILL_HOVER
                } else {
                    po0::ACCENT_FILL
                };
                painter.rect_filled(rect, 13, fill);
            }
            (true, Seg::Choice) => {
                painter.rect(
                    rect,
                    13,
                    po0::YOU_TINT,
                    egui::Stroke::new(1.0, po0::YOU_RING),
                    egui::StrokeKind::Inside,
                );
            }
            (false, _) if hot => {
                painter.rect_filled(rect, 13, po0::FILL);
            }
            _ => {}
        }
        let ink = match (selected, kind) {
            (true, Seg::Version) => Color32::WHITE,
            (true, Seg::Choice) => po0::TEXT,
            _ if hot => po0::TEXT,
            _ => po0::TEXT_2,
        };
        painter.galley(rect.center() - galley.size() / 2.0, galley, ink);
    }
    response
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Round {
    Quiet,
    Tinted,
    Selected,
    Send,
}
/// A round icon button. `label` is the accessible name; `tip` the tooltip.
fn round_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    tip: &str,
    diameter: f32,
    kind: Round,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), sense);
    let selected = kind == Round::Selected;
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, selected, label)
    });
    if ui.is_rect_visible(rect) {
        let hot = enabled && response.hovered();
        let pressed = enabled && response.is_pointer_button_down_on();
        let fill = match kind {
            Round::Send if !enabled => po0::FILL,
            Round::Send if pressed => po0::ACCENT_FILL,
            Round::Send if hot => Color32::from_rgb(0x28, 0x91, 0xFF),
            Round::Send => po0::YOU,
            Round::Tinted if hot || pressed => po0::FILL_3,
            Round::Tinted => po0::FILL_2,
            Round::Selected => po0::YOU_TINT,
            Round::Quiet if pressed => po0::FILL_3,
            Round::Quiet if hot => po0::FILL,
            Round::Quiet => Color32::TRANSPARENT,
        };
        let radius = diameter / 2.0 - if pressed { 1.0 } else { 0.0 };
        ui.painter().circle_filled(rect.center(), radius, fill);
        if selected {
            ui.painter().circle_stroke(
                rect.center(),
                radius - 0.5,
                egui::Stroke::new(1.0, po0::YOU_RING),
            );
        }
        let ink = match kind {
            _ if !enabled => po0::TEXT_OFF,
            Round::Send => Color32::WHITE,
            Round::Quiet if !hot => po0::TEXT_2,
            _ => po0::TEXT,
        };
        let glyph = (diameter * 0.47).round().clamp(12.0, 17.0);
        icon.image(glyph).tint(ink).paint_at(
            ui,
            Rect::from_center_size(rect.center(), Vec2::splat(glyph)),
        );
    }
    response.on_hover_text(tip)
}
/// The version name in `type-plate` over a dot and its status line.
fn plate_label(ui: &mut egui::Ui, name: &str, dot: Color32, status: &str, tip: &str) {
    let painter = ui.painter();
    let name_galley = painter.layout_no_wrap(name.to_owned(), theme::semibold(15.0), po0::TEXT);
    let status_galley = painter.layout_no_wrap(
        status.to_owned(),
        egui::FontId::proportional(11.5),
        po0::TEXT_2,
    );
    let width = name_galley
        .size()
        .x
        .max(status_galley.size().x + 13.0)
        .min(ui.available_width().max(0.0));
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 36.0), egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("{name} · {status}"))
    });
    let painter = ui.painter_at(rect);
    painter.galley(rect.left_top(), name_galley, po0::TEXT);
    painter.circle_filled(egui::pos2(rect.left() + 3.5, rect.top() + 28.0), 3.5, dot);
    painter.galley(
        egui::pos2(rect.left() + 13.0, rect.top() + 20.5),
        status_galley,
        po0::TEXT_2,
    );
    if !tip.is_empty() {
        response.on_hover_text(tip);
    }
}
/// `type-timecode` with the frames in `text-3`, over "of <duration>". Tabular
/// digits come from PO0's fallback, `JetBrains Mono`, until `Inter Tabular` lands.
fn timecode_block(ui: &mut egui::Ui, time: &str, frames: &str, of: &str) {
    let mut job = egui::text::LayoutJob::default();
    for (text, color) in [
        (time.to_owned(), po0::TEXT),
        (format!(":{frames}"), po0::TEXT_3),
    ] {
        job.append(
            &text,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::monospace(14.5),
                color,
                ..Default::default()
            },
        );
    }
    let painter = ui.painter();
    let main = painter.layout_job(job);
    let under =
        painter.layout_no_wrap(of.to_owned(), egui::FontId::proportional(11.5), po0::TEXT_3);
    let width = main.size().x.max(under.size().x);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 36.0), egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Label,
            true,
            format!("{time}:{frames} {of}"),
        )
    });
    let painter = ui.painter();
    painter.galley(
        egui::pos2(rect.right() - main.size().x, rect.top()),
        main,
        po0::TEXT,
    );
    painter.galley(
        egui::pos2(rect.right() - under.size().x, rect.top() + 20.0),
        under,
        po0::TEXT_3,
    );
}
/// The composer's moment chip: "@ 02:14 on B".
fn time_chip(
    ui: &mut egui::Ui,
    at: TimeCode,
    fps: kinewright_core::Rational,
    letter: Option<char>,
) {
    let (time, _) = clock(at, fps);
    let mut job = egui::text::LayoutJob::default();
    let quiet = |text: &str| (text.to_owned(), theme::medium(12.0), po0::TEXT_2);
    let strong = |text: &str| (text.to_owned(), theme::semibold(12.0), po0::TEXT);
    let mut parts = vec![quiet("@ "), strong(&time)];
    if let Some(letter) = letter {
        parts.push(quiet(" on "));
        parts.push(strong(&letter.to_string()));
    }
    let mut spoken = String::new();
    for (text, font_id, color) in parts {
        spoken.push_str(&text);
        job.append(
            &text,
            0.0,
            egui::TextFormat {
                font_id,
                color,
                ..Default::default()
            },
        );
    }
    let galley = ui.painter().layout_job(job);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 24.0, 32.0),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &spoken));
    ui.painter().rect_filled(rect, 16, po0::TRACK);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, po0::TEXT);
    response.on_hover_text(crate::timeline_ui::format_timecode(at, fps));
}

#[cfg(test)]
#[path = "studio_tests.rs"]
pub(crate) mod tests;

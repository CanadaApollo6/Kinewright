//! Studio composition and selection. Browsing never changes the manual Core.
use crate::{
    app::KinewrightApp,
    ja1_attachment::Action,
    presenter::{DisplayCell, PresentationContext},
    theme::{self, color, space},
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
    pub(crate) manual_inputs: Option<Arc<kinewright_media::owned_io::PlaybackInputs>>,
    manual_scope: Option<(u64, kinewright_project::bundle_records::ObjectRef)>,
    versions: Vec<SnapshotKey>,
    versions_open: bool,
    tab: Tab,
    draft_snapshot: Option<SnapshotKey>,
    stills: [Option<Still>; 2],
    following: bool,
    seen: usize,
    unseen: usize,
    jump: bool,
    neutral: bool,
}

/// Opaque PO0 tokens only; effects remain in the separate PO0 rollout.
const BASE: Color32 = Color32::from_rgb(11, 11, 13);
const WELL: Color32 = Color32::from_rgb(19, 19, 22);
const RAIL: Color32 = Color32::from_rgb(24, 24, 28);

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
    pub(crate) fn new(view: Rect, editor: bool) -> Self {
        let narrow = view.width() < 1000.0;
        let margin = if narrow { space::THREE } else { space::SIX };
        let inner = view.shrink(margin);
        let header = Rect::from_min_size(inner.min, egui::vec2(inner.width(), 40.0));
        let top = header.bottom() + space::FOUR;
        let timeline_h = if editor { 240.0 } else { 144.0 };
        let timeline = Rect::from_min_max(
            egui::pos2(inner.left(), inner.bottom() - timeline_h),
            inner.max,
        );
        let width = if narrow {
            inner.width()
        } else {
            (inner.width() - 336.0 - space::SIX).max(1.0)
        };
        let plate_h = if narrow { 76.0 } else { 52.0 };
        let available_h = (timeline.top() - space::FOUR - top - plate_h - 64.0).max(120.0);
        let picture_h = (width * 9.0 / 16.0).min(available_h);
        let picture =
            Rect::from_min_size(egui::pos2(inner.left(), top), egui::vec2(width, picture_h));
        let plate = Rect::from_min_size(picture.left_bottom(), egui::vec2(width, plate_h));
        let composer_width = width.min(640.0);
        let composer = Rect::from_min_size(
            egui::pos2(
                picture.center().x - composer_width / 2.0,
                plate.bottom() + space::TWO,
            ),
            egui::vec2(composer_width, 52.0),
        );
        let rail = if narrow {
            Rect::from_min_max(
                egui::pos2(inner.left(), composer.bottom() + space::THREE),
                inner.max,
            )
        } else {
            Rect::from_min_max(
                egui::pos2(picture.right() + space::SIX, picture.top()),
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
fn fit(rect: Rect, size: Vec2) -> Rect {
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
                BASE
            }))
            .show(ui, |ui| {
                let layout = Layout::new(ui.max_rect(), self.focused().studio.mode == Mode::Editor);
                at(ui, "studio-header", layout.header, |ui| {
                    self.studio_header(ui, layout.picture.center().x, layout.narrow);
                });
                at(ui, "studio-picture", layout.picture, |ui| {
                    self.studio_picture(ui);
                });
                at(ui, "studio-plate", layout.plate, |ui| self.studio_plate(ui));
                at(ui, "studio-composer", layout.composer, |ui| {
                    self.studio_composer(ui);
                });
                if layout.narrow {
                    at(ui, "studio-narrow-tabs", layout.rail, |ui| {
                        self.studio_tabs(ui, true);
                        match self.focused().studio.tab {
                            Tab::Timeline => self.studio_timeline(ui),
                            Tab::Compare => self.studio_compare(ui),
                            _ => self.studio_rail_content(ui),
                        }
                    });
                } else {
                    at(ui, "studio-rail", layout.rail, |ui| {
                        egui::Frame::new()
                            .fill(RAIL)
                            .corner_radius(16)
                            .inner_margin(12)
                            .show(ui, |ui| {
                                ui.set_min_size(layout.rail.size() - egui::vec2(24.0, 24.0));
                                self.studio_tabs(ui, false);
                                self.studio_rail_content(ui);
                            });
                    });
                    at(ui, "studio-timeline", layout.timeline, |ui| {
                        self.studio_timeline(ui);
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
    fn studio_header(&mut self, ui: &mut egui::Ui, center: f32, narrow: bool) {
        let rect = ui.max_rect();
        let modes =
            Rect::from_center_size(egui::pos2(center, rect.center().y), egui::vec2(150.0, 32.0));
        let project = Rect::from_min_max(
            rect.min,
            egui::pos2(
                (modes.left() - space::FOUR).max(rect.left() + 80.0),
                rect.bottom(),
            ),
        );
        at(ui, "studio-title", project, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(theme::wordmark("K", color::TEXT_PRIMARY));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(self.focused().display_name())
                            .font(theme::semibold(17.0)),
                    )
                    .truncate(),
                );
            });
        });
        at(ui, "studio-mode", modes, |ui| {
            ui.horizontal_centered(|ui| {
                ui.selectable_value(&mut self.focused_mut().studio.mode, Mode::Studio, "Studio");
                ui.selectable_value(&mut self.focused_mut().studio.mode, Mode::Editor, "Editor");
            });
        });
        let actions = Rect::from_min_max(
            egui::pos2(
                if narrow {
                    rect.right() - 76.0
                } else {
                    rect.right() - 336.0
                },
                rect.top(),
            ),
            rect.max,
        );
        at(ui, "studio-actions", actions, |ui| {
            if narrow {
                ui.menu_button("Project", |ui| self.studio_actions(ui));
            } else {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.studio_actions(ui);
                });
            }
        });
    }
    fn studio_actions(&mut self, ui: &mut egui::Ui) {
        if ui.button("Versions").clicked() {
            self.focused_mut().studio.versions_open = true;
            if self.focused().ja1.is_none() {
                self.enable_ja1();
            }
            self.start_ja1(Action::StudioVersions);
        }
        if let Some(context) = &self.focused().studio.selection {
            let key = context.snapshot.descriptor.key.clone();
            if ui.button("Edit a copy").clicked() {
                self.start_ja1(Action::StudioEditCopy(key));
            }
        } else if ui.button("Save").clicked() {
            self.save_project(false);
        }
        let manual_ready = self
            .focused()
            .ja1
            .as_ref()
            .is_none_or(|attachment| !attachment.read_only());
        if ui
            .add_enabled(manual_ready, egui::Button::new("Current edit"))
            .clicked()
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
    fn studio_picture(&self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 4, Color32::BLACK);
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
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Add media to begin",
                egui::FontId::proportional(17.0),
                color::TEXT_SECONDARY,
            );
        }
    }
    fn studio_plate(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let versions = self.focused().studio.versions.clone();
            for (index, version) in versions.iter().take(3).enumerate() {
                let label = ['A', 'B', 'C'][index].to_string();
                let selected = self
                    .focused()
                    .studio
                    .selection
                    .as_ref()
                    .is_some_and(|c| c.snapshot.descriptor.key == *version);
                if ui
                    .selectable_label(selected, label)
                    .on_hover_text(version.version.as_str())
                    .clicked()
                {
                    self.start_ja1(Action::StudioSelect(version.clone()));
                }
            }
            let label =
                self.focused()
                    .studio
                    .selection
                    .as_ref()
                    .map_or("Current edit".to_owned(), |c| {
                        format!(
                            "Version {}",
                            c.snapshot
                                .descriptor
                                .key
                                .version
                                .as_str()
                                .chars()
                                .take(8)
                                .collect::<String>()
                        )
                    });
            ui.label(egui::RichText::new(label).font(theme::semibold(15.0)));
            if self.focused().studio.selection.is_some() {
                ui.small("Unreviewed");
            }
            ui.add_space(space::FOUR);
            if ui
                .button("Back")
                .on_hover_text("Previous frame · Left")
                .clicked()
            {
                self.seek_to(TimeCode(self.program_position().0.saturating_sub(1)));
            }
            if ui
                .button(if self.playing { "Pause" } else { "Play" })
                .on_hover_text("Space")
                .clicked()
            {
                self.toggle_playback();
            }
            if ui
                .button("Next")
                .on_hover_text("Next frame · Right")
                .clicked()
            {
                self.seek_to(TimeCode(self.program_position().0.saturating_add(1)));
            }
            ui.label(crate::timeline_ui::format_timecode(
                self.program_position(),
                self.program_document().fps,
            ));
            ui.add_enabled(false, egui::Button::new("Draw"));
            ui.add_enabled(false, egui::Button::new("Point"));
            ui.toggle_value(&mut self.focused_mut().studio.neutral, "Neutral")
                .on_hover_text("Neutral surround");
        });
        if let Some(cell) = self.presenter.bound()
            && let Some(context) = cell.context
        {
            ui.small(format!(
                "Showing {} · {}{}",
                context
                    .snapshot
                    .version
                    .as_str()
                    .chars()
                    .take(8)
                    .collect::<String>(),
                crate::timeline_ui::format_timecode(cell.at, context.inputs.document().fps),
                if cell.stale {
                    " · loading selected version"
                } else {
                    ""
                }
            ));
        }
        if let Some(error) = &self.focused().studio.failure {
            ui.colored_label(
                color::STATUS_DANGER,
                format!("Damaged or unavailable · {error}"),
            );
        }
    }
    fn studio_composer(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(RAIL)
            .corner_radius(26)
            .inner_margin(egui::Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let at = self.program_position();
                    ui.small(format!(
                        "@ {}",
                        crate::timeline_ui::format_timecode(at, self.program_document().fps)
                    ));
                    let snapshot = self
                        .presenter
                        .bound()
                        .and_then(|cell| cell.context.map(|c| c.snapshot.clone()));
                    let input_id = egui::Id::new(("studio-composer-input", self.focused().id));
                    let thread = self.active_thread_mut();
                    let response = ui.add_sized(
                        [ui.available_width() - 64.0, 32.0],
                        egui::TextEdit::singleline(&mut thread.input)
                            .id(input_id)
                            .hint_text("Leave a note at this moment, or ask the team…"),
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
                    let can_send = !self.focused().ja1_read_only()
                        && !self.active_thread().input.trim().is_empty();
                    if ui
                        .add_enabled(can_send, egui::Button::new("Send"))
                        .clicked()
                    {
                        self.start_agent_turn();
                    }
                });
            });
    }
    fn studio_tabs(&mut self, ui: &mut egui::Ui, narrow: bool) {
        ui.horizontal(|ui| {
            for (tab, label) in [
                (Tab::Commentary, "Commentary"),
                (Tab::Notes, "Notes"),
                (Tab::Timeline, "Timeline"),
                (Tab::Compare, "A/B"),
            ] {
                if narrow || matches!(tab, Tab::Commentary | Tab::Notes) {
                    ui.selectable_value(&mut self.focused_mut().studio.tab, tab, label);
                }
            }
        });
        ui.add_space(space::THREE);
    }
    fn studio_rail_content(&mut self, ui: &mut egui::Ui) {
        if self.focused().studio.tab == Tab::Notes {
            ui.label("Notes · newest first");
            if let Some(snapshot) = &self.focused().studio.draft_snapshot {
                ui.small(format!("Draft on {}", snapshot.version.as_str()));
            }
            if !self.active_thread().input.is_empty() {
                ui.label(&self.active_thread().input);
            }
            ui.colored_label(color::TEXT_MUTED, "No saved notes yet");
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
                ui.colored_label(color::TEXT_MUTED, "The conversation will appear here.");
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
        if unseen > 0 && ui.button(format!("{unseen} new ↓")).clicked() {
            self.focused_mut().studio.jump = true;
        }
    }
    fn studio_timeline(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(WELL)
            .corner_radius(12)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                let duration = self.program_document().duration.0.saturating_sub(1).max(0);
                let mut position = self.program_position().0;
                if ui
                    .add_sized(
                        [ui.available_width(), 22.0],
                        egui::Slider::new(&mut position, 0..=duration).text("Frame"),
                    )
                    .changed()
                {
                    self.seek_to(TimeCode(position));
                }
                egui::ScrollArea::both()
                    .max_height(32.0)
                    .id_salt("studio-timeline-text")
                    .show(ui, |ui| {
                        for track in &self.program_document().tracks {
                            ui.horizontal(|ui| {
                                ui.small(format!("{:?}", track.kind));
                                for clip in &track.clips {
                                    ui.label(format!(
                                        "{} · {}–{}",
                                        clip.id,
                                        clip.timeline_start.0,
                                        clip.timeline_start.0 + clip.source_range.end.0
                                            - clip.source_range.start.0
                                    ));
                                }
                            });
                        }
                    });
                ui.horizontal(|ui| {
                    for (slot, label) in [(0, "Keep A still"), (1, "Keep B still")] {
                        if ui.button(label).clicked() {
                            self.capture_studio_still(slot, ui.ctx());
                        }
                    }
                    if ui.button("Compare A/B").clicked() {
                        self.focused_mut().studio.compare_open = true;
                    }
                    if self.focused().studio.return_to.is_some()
                        && ui.button("Return to original version").clicked()
                    {
                        self.return_to_original_version();
                    }
                });
            });
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

#[cfg(test)]
#[path = "studio_tests.rs"]
mod tests;

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use kinewright_core::{Analysis, Document, ImportRecord, LossBound, LossCode, Rational};
use kinewright_project::interchange::{
    ImportOptions, MediaEntry, MediaStatus, PrepareControl, PrepareOptions, PreparedImport,
    Refusal, RefusalCode, ShareRoot, prepare,
};

use crate::theme::{self, color, space, type_size};

pub(crate) const OPEN_FILTER: (&str, &[&str]) = ("Timeline (OTIO, FCPXML)", &["otio", "fcpxml"]);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OpenPath {
    Project,
    Timeline { otio: bool },
}

pub(crate) fn classify_open_path(path: &Path) -> OpenPath {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return OpenPath::Project;
    };
    if ["otio", "otiod"]
        .iter()
        .any(|suffix| extension.eq_ignore_ascii_case(suffix))
    {
        OpenPath::Timeline { otio: true }
    } else if ["fcpxml", "fcpxmld"]
        .iter()
        .any(|suffix| extension.eq_ignore_ascii_case(suffix))
    {
        OpenPath::Timeline { otio: false }
    } else {
        OpenPath::Project
    }
}

enum Preparation {
    Running(mpsc::Receiver<Result<PreparedImport, Refusal>>),
    Ready(Box<PreparedImport>),
    Refused(Refusal),
}

pub(crate) enum ImportAction {
    Wait,
    Cancel,
    Open(Box<PreparedImport>),
}

pub(crate) struct ImportDialog {
    source: PathBuf,
    analysis: Arc<dyn Analysis>,
    options: ImportOptions,
    allowed: BTreeSet<ShareRoot>,
    share_choices: Vec<ShareRoot>,
    timeline_choices: Vec<String>,
    otio: bool,
    overrides: Option<(u32, u32, u32, u32)>,
    folder_picker: Option<mpsc::Receiver<Option<PathBuf>>>,
    state: Preparation,
    focus_open: bool,
    selected_media: Option<usize>,
}

impl ImportDialog {
    pub(crate) fn new(
        source: PathBuf,
        otio: bool,
        analysis: Arc<dyn Analysis>,
        ctx: &egui::Context,
    ) -> Self {
        let mut dialog = Self {
            source,
            analysis,
            options: ImportOptions::default(),
            allowed: BTreeSet::new(),
            share_choices: Vec::new(),
            timeline_choices: Vec::new(),
            otio,
            overrides: None,
            folder_picker: None,
            state: Preparation::Refused(Refusal::new(
                RefusalCode::Cancelled,
                "The import was cancelled.",
            )),
            focus_open: false,
            selected_media: None,
        };
        dialog.restart(ctx);
        dialog
    }

    fn restart(&mut self, ctx: &egui::Context) {
        self.options.control.cancel();
        self.options.control = PrepareControl::default();
        self.selected_media = None;
        let (send, receive) = mpsc::sync_channel(1);
        let options = PrepareOptions {
            import: self.options.clone(),
            allowed_shares: self.allowed.clone(),
            analysis: Arc::clone(&self.analysis),
        };
        let path = self.source.clone();
        let ctx = ctx.clone();
        self.state = match thread::Builder::new()
            .name("kinewright-interchange".into())
            .spawn(move || {
                let result = path.to_str().map_or_else(
                    || {
                        Err(Refusal::new(
                            RefusalCode::Malformed,
                            "The timeline path is not valid UTF-8.",
                        ))
                    },
                    |path| prepare(path, options),
                );
                let _ = send.send(result);
                ctx.request_repaint();
            }) {
            Ok(_) => Preparation::Running(receive),
            Err(error) => Preparation::Refused(Refusal::new(
                RefusalCode::LoweringFailed,
                format!("Could not start the import worker: {error}"),
            )),
        };
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(picker) = &self.folder_picker {
            match picker.try_recv() {
                Ok(path) => {
                    self.folder_picker = None;
                    if let Some(path) = path {
                        self.locate_folder(&path, ctx);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.folder_picker = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Preparation::Running(receiver) = &self.state {
            match receiver.try_recv() {
                Ok(result) => self.finish_preparation(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.state = Preparation::Refused(Refusal::new(
                        RefusalCode::LoweringFailed,
                        "The import worker ended before returning a result.",
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
        }
    }

    fn finish_preparation(&mut self, result: Result<PreparedImport, Refusal>) {
        match result {
            Ok(prepared) => {
                let document = prepared.document();
                if self.otio
                    && self.options.project_fps.is_none()
                    && self.options.resolution.is_none()
                {
                    self.overrides = Some((
                        document.fps.numerator(),
                        document.fps.denominator(),
                        document.resolution.0,
                        document.resolution.1,
                    ));
                }
                self.share_choices = prepared
                    .media()
                    .iter()
                    .filter_map(MediaEntry::share_root)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                self.state = Preparation::Ready(Box::new(prepared));
                self.focus_open = true;
            }
            Err(refusal) => {
                if !refusal.timelines.is_empty() {
                    self.timeline_choices.clone_from(&refusal.timelines);
                }
                if self.otio && refusal.code == RefusalCode::ProjectRateRequired {
                    self.overrides.get_or_insert((24, 1, 0, 0));
                }
                self.state = Preparation::Refused(refusal);
            }
        }
    }

    fn locate_folder(&mut self, path: &Path, ctx: &egui::Context) {
        if let Some(path) = path.to_str()
            && !self.options.media_roots.iter().any(|root| root == path)
        {
            self.options.media_roots.push(path.to_owned());
            self.restart(ctx);
        }
    }

    fn choose_folder(&mut self, ctx: &egui::Context) {
        let (send, receive) = mpsc::sync_channel(1);
        let ctx = ctx.clone();
        if thread::Builder::new()
            .name("kinewright-locate-folder".into())
            .spawn(move || {
                let _ = send.send(rfd::FileDialog::new().pick_folder());
                ctx.request_repaint();
            })
            .is_ok()
        {
            self.folder_picker = Some(receive);
        }
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context) -> ImportAction {
        self.poll(ctx);
        let mut open = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("import-timeline")).show(ctx, |ui| {
            let height = dialog_width_and_height(ui);
            ui.heading("Import timeline");
            ui.add(egui::Label::new(self.source.to_string_lossy()).truncate())
                .on_hover_text(self.source.to_string_lossy());
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(height)
                .show(ui, |ui| self.show_body(ui));
            ui.add_space(space::TWO);
            ui.horizontal(|ui| {
                let button = ui.add_enabled(
                    matches!(self.state, Preparation::Ready(_)) && self.folder_picker.is_none(),
                    primary_button("Open"),
                );
                if std::mem::take(&mut self.focus_open) {
                    button.request_focus();
                }
                open = button.clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if cancel || response.should_close() {
            self.options.control.cancel();
            ImportAction::Cancel
        } else if open {
            let Preparation::Ready(prepared) = std::mem::replace(
                &mut self.state,
                Preparation::Refused(Refusal::new(
                    RefusalCode::Cancelled,
                    "The import was closed.",
                )),
            ) else {
                unreachable!()
            };
            // The returned preparation owns this control now. Dropping the dialog must
            // not cancel it before the caller commits the document.
            self.options.control = PrepareControl::default();
            ImportAction::Open(prepared)
        } else {
            ImportAction::Wait
        }
    }

    fn show_body(&mut self, ui: &mut egui::Ui) {
        if !matches!(self.state, Preparation::Running(_)) {
            self.show_options(ui);
        }
        match &self.state {
            Preparation::Running(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    let bytes = self.options.control.hashed_bytes();
                    if bytes == 0 {
                        ui.label("Reading timeline and checking media…");
                    } else {
                        ui.label(format!("Checking media contents · {bytes} bytes read"));
                    }
                });
                return;
            }
            Preparation::Ready(prepared) => {
                let document = prepared.document();
                ui.label(format!(
                    "{}/{} fps · {} × {}",
                    document.fps.numerator(),
                    document.fps.denominator(),
                    document.resolution.0,
                    document.resolution.1,
                ));
                show_totals(ui, prepared.record());
                show_media(ui, prepared.media(), &mut self.selected_media);
                show_record_notes(ui, prepared.record());
            }
            Preparation::Refused(refusal) => show_refusal(ui, refusal),
        }
        if !self.options.media_roots.is_empty() {
            ui.collapsing("Folders searched", |ui| {
                for root in &self.options.media_roots {
                    ui.label(root);
                }
            });
        }
        if ui
            .add_enabled(
                self.options.media_roots.len() < 8 && self.folder_picker.is_none(),
                egui::Button::new("Locate folder…"),
            )
            .clicked()
        {
            self.choose_folder(ui.ctx());
        }

        if self.show_share_choices(ui) {
            self.restart(ui.ctx());
        }
    }

    fn show_share_choices(&mut self, ui: &mut egui::Ui) -> bool {
        if self.share_choices.is_empty() {
            return false;
        }
        ui.label(egui::RichText::new("Remote shares").font(theme::semibold(type_size::BODY)));
        let mut changed = false;
        egui::ScrollArea::vertical()
            .id_salt("import-share-list")
            .max_height(120.0)
            .show_rows(
                ui,
                ui.spacing().interact_size.y,
                self.share_choices.len(),
                |ui, range| {
                    for share in &self.share_choices[range] {
                        let mut allowed = self.allowed.contains(share);
                        if ui
                            .checkbox(&mut allowed, format!("Allow {}", share.as_str()))
                            .changed()
                        {
                            if allowed {
                                self.allowed.insert(share.clone());
                            } else {
                                self.allowed.remove(share);
                            }
                            changed = true;
                        }
                    }
                },
            );
        changed
    }

    fn show_options(&mut self, ui: &mut egui::Ui) {
        let mut restart = false;
        if self.timeline_choices.is_empty() {
            if let Preparation::Ready(prepared) = &self.state {
                ui.label(format!("Timeline · {}", prepared.name()));
            }
        } else {
            egui::ComboBox::from_id_salt("interchange-timeline")
                .selected_text(
                    self.options
                        .timeline
                        .as_deref()
                        .unwrap_or("Choose a timeline"),
                )
                .show_ui(ui, |ui| {
                    for name in &self.timeline_choices {
                        if ui
                            .selectable_label(self.options.timeline.as_ref() == Some(name), name)
                            .clicked()
                        {
                            self.options.timeline = Some(name.clone());
                            restart = true;
                        }
                    }
                });
        }
        if let Some((numerator, denominator, width, height)) = &mut self.overrides {
            ui.horizontal(|ui| {
                ui.label("Frames per second");
                ui.add(egui::DragValue::new(numerator).range(1..=u32::MAX));
                ui.label("/");
                ui.add(egui::DragValue::new(denominator).range(1..=u32::MAX));
            });
            ui.horizontal(|ui| {
                ui.label("Width and height");
                ui.add(egui::DragValue::new(width));
                ui.label("×");
                ui.add(egui::DragValue::new(height));
                if ui.button("Apply settings").clicked() {
                    self.options.project_fps = Rational::new(*numerator, *denominator).ok();
                    self.options.resolution =
                        (*width > 0 && *height > 0).then_some((*width, *height));
                    restart = true;
                }
            });
        }
        if restart {
            self.restart(ui.ctx());
        }
    }
}

impl Drop for ImportDialog {
    fn drop(&mut self) {
        self.options.control.cancel();
    }
}

fn dialog_width_and_height(ui: &mut egui::Ui) -> f32 {
    let window = ui.ctx().content_rect();
    ui.set_width((window.width() - 48.0).clamp(180.0, 780.0));
    (window.height() * 0.58).min(460.0)
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(text)
        .fill(color::ACCENT_WASH)
        .stroke(egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER))
}

fn technical_name(value: impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => String::new(),
    }
}

fn show_refusal(ui: &mut egui::Ui, refusal: &Refusal) {
    ui.colored_label(color::STATUS_WARNING, refusal.to_string());
    ui.small(technical_name(refusal.code));
}

pub(crate) fn import_status(timeline: &str, record: &ImportRecord) -> String {
    format!(
        "Imported {timeline}: {} exact, {} approximated, {} dropped",
        record.exact, record.approximated, record.dropped,
    )
}

fn show_totals(ui: &mut egui::Ui, record: &ImportRecord) {
    ui.label(format!(
        "{} items kept exactly · {} brought over with changes · {} could not be brought over",
        record.exact, record.approximated, record.dropped,
    ));
}

fn show_record_notes(ui: &mut egui::Ui, record: &ImportRecord) {
    for (title, items) in [
        ("Changes to items", true),
        ("Timeline and media notes", false),
    ] {
        let mut rows = record
            .rows
            .iter()
            .filter(|row| row.code.is_item() == items)
            .peekable();
        if rows.peek().is_none() {
            continue;
        }
        ui.separator();
        ui.label(egui::RichText::new(title).font(theme::semibold(type_size::BODY)));
        for row in rows {
            ui.label(format!("{} · {}", row.count, loss_description(row.code)));
            let code = technical_name(row.code);
            ui.small(&code);
            if row.unverified {
                ui.label("Unverified against the original editor");
            }
            if let Some(bound) = row.bound {
                let text = match bound {
                    LossBound::Frames(value) => format!("Largest change: {value} frames"),
                    LossBound::TenthDb(value) => {
                        format!("Largest change: {}.{} dB", value / 10, value % 10)
                    }
                    LossBound::SpeedTenthPercent(value) => {
                        format!("Largest change: {}.{}%", value / 10, value % 10)
                    }
                    LossBound::Milliseconds(value) => format!("Largest change: {value} ms"),
                };
                ui.small(text);
            }
            for name in &row.names {
                ui.small(name);
            }
            if !row.at.is_empty() {
                ui.push_id(&code, |ui| {
                    ui.collapsing("Examples", |ui| {
                        for site in &row.at {
                            ui.label(format!("Frame {} · {}", site.frame, site.source));
                        }
                    });
                });
            }
        }
    }
    if !record.omitted_codes.is_empty() {
        ui.collapsing("Additional details omitted from the saved report", |ui| {
            for &code in &record.omitted_codes {
                ui.label(loss_description(code));
                ui.small(technical_name(code));
            }
        });
    }
}

fn loss_description(code: LossCode) -> &'static str {
    match code {
        LossCode::SubframeRounded => "Timing rounded to whole frames",
        LossCode::CutSnapped => "Cut moved to a nearby frame",
        LossCode::ClipTooShort => "Clip too short to fit a frame",
        LossCode::OutsideWindow => "Item outside the part used in the timeline",
        LossCode::BeforeStartTrimmed => "Material trimmed before the timeline start",
        LossCode::NestFlattened => "Nested timeline opened as separate tracks",
        LossCode::TransitionLane => "Dissolve moved to its own track and split from the clip body",
        LossCode::LaneAdded => "Extra track added to keep overlapping items",
        LossCode::MulticamActiveAngle => "Kept the selected camera angle",
        LossCode::AuditionActivePick => "Kept the selected take",
        LossCode::SpeedRounded => "Playback speed rounded",
        LossCode::SpeedClamped => "Playback speed limited to the supported range",
        LossCode::ReverseDropped => "Reverse playback could not be brought over",
        LossCode::VariableSpeedAveraged => "Changing playback speed replaced with an average",
        LossCode::NestRetimeUnsupported => {
            "Speed change on a nested timeline could not be brought over"
        }
        LossCode::RetimedAudioMuted => "Audio omitted where playback speed changed",
        LossCode::TransitionTypeMapped => "Transition replaced with a dissolve",
        LossCode::TransitionAtNestCut => "Transition beside a nested timeline became a cut",
        LossCode::AudioCrossfadeLinear => "Audio crossfade changed to a straight fade",
        LossCode::TransitionHandlesShort => "Transition shortened to fit the available footage",
        LossCode::TransitionFadeClamped => "Fade shortened to fit the clip",
        LossCode::GainClamped => "Volume limited to the supported range",
        LossCode::GainKeysMapped => "Volume changes moved to whole frames",
        LossCode::FadeShapeLinear => "Curved fade replaced with a straight fade",
        LossCode::PanDropped => "Left and right audio balance could not be brought over",
        LossCode::ChannelMapDropped => "Audio channel routing could not be brought over",
        LossCode::AvLinkUnpaired => "No matching picture and sound pair to link",
        LossCode::MarkerDurationDropped => "Marker kept at one point in time",
        LossCode::MarkerColorAsLabel => "Marker color written into its label",
        LossCode::MarkerColorDropped => "Marker color could not be brought over",
        LossCode::ItemMarkerPinned => "Clip marker placed on the timeline",
        LossCode::MarkerOutsideItem => "Marker fell outside the part of the clip used",
        LossCode::KeywordRatingDropped => "Keywords or ratings could not be brought over",
        LossCode::EffectDropped => "Effect could not be brought over",
        LossCode::TitleStyleDefault => "Title uses the editor’s default style",
        LossCode::TitleTextTruncated => "Long title text shortened",
        LossCode::GeneratorDropped => "Generated image or graphic left a gap",
        LossCode::ImageSequenceDropped => "Image sequence could not be brought over",
        LossCode::UnknownItem => "Unsupported item could not be brought over",
        LossCode::SourceClamped => "Clip trimmed to fit the available source",
        LossCode::SourceOriginAssumed => "Start of the source was estimated",
        LossCode::StartRebased => "Timeline start moved to zero",
        LossCode::RateSnapped => "Frame rate mapped to a standard rate",
        LossCode::ResolutionInferred => "Picture size was estimated",
        LossCode::DropFrameLabels => "Original timecode labels could not be brought over",
        LossCode::ColorSpaceDefault => "Timeline uses the editor’s default color space",
        LossCode::TrackNamesDropped => "Track name shortened or omitted",
        LossCode::ClipNamesDropped => "Clip name shortened or omitted",
        LossCode::MetadataDropped => "Additional information could not be brought over",
        LossCode::TrailingGapTrimmed => "Empty time after the last clip was trimmed",
        LossCode::MediaOffline => "Media is offline",
        LossCode::MediaTimeout => "Media check took too long",
        LossCode::RemoteUrl => "Remote media is offline",
        LossCode::RelinkedByName => "Replacement media found by file name",
        LossCode::TimeWarpReading => "Reading of speed changes in an OTIO timeline",
        LossCode::HoldReading => "Reading of held frames in a Final Cut timeline",
        LossCode::AnchorRetimeReading => "Timing of attached clips when playback speed changes",
    }
}

fn show_media(ui: &mut egui::Ui, media: &[MediaEntry], selection: &mut Option<usize>) {
    if media.is_empty() {
        return;
    }
    ui.separator();
    ui.label(egui::RichText::new("Media").font(theme::semibold(type_size::BODY)));
    egui::ScrollArea::vertical()
        .id_salt("import-media-list")
        .max_height(144.0)
        .show_rows(
            ui,
            ui.spacing().interact_size.y,
            media.len(),
            |ui, range| {
                for index in range {
                    let entry = &media[index];
                    let status = match entry.status {
                        MediaStatus::Online if entry.relinked_by_name => "Found by file name",
                        MediaStatus::Online | MediaStatus::Unprobed => "Online",
                        MediaStatus::Missing => "Missing",
                        MediaStatus::Ambiguous => "Missing · several matches",
                        MediaStatus::InvalidLocator => "Invalid location",
                        MediaStatus::Unreadable => "Could not be read",
                        MediaStatus::Remote => "Remote · offline",
                        MediaStatus::TimedOut => "Timed out",
                    };
                    let text = format!("{status} · {} · {}", entry.name, entry.locator);
                    if ui
                        .add(egui::Button::selectable(*selection == Some(index), text).truncate())
                        .on_hover_text(&entry.locator)
                        .clicked()
                    {
                        *selection = Some(index);
                    }
                }
            },
        );
    if let Some(entry) = selection.and_then(|index| media.get(index)) {
        ui.label(&entry.locator);
        if let Some(path) = &entry.resolved_path {
            ui.label(format!("Found at {}", path.display()));
        }
        if let Some(matches) = &entry.matches {
            ui.label("Several files have this name. The source stays offline until you pick one.");
            egui::ScrollArea::vertical()
                .id_salt("import-match-list")
                .max_height(100.0)
                .show_rows(
                    ui,
                    ui.text_style_height(&egui::TextStyle::Body),
                    matches.len(),
                    |ui, range| {
                        for path in &matches[range] {
                            let path = path.to_string_lossy();
                            ui.add(egui::Label::new(path.as_ref()).truncate())
                                .on_hover_text(path.as_ref());
                        }
                    },
                );
        }
    }
}

pub(crate) fn show_report(ctx: &egui::Context, document: &Document) -> bool {
    let mut close = false;
    let response = egui::Modal::new(egui::Id::new("import-report")).show(ctx, |ui| {
        let height = dialog_width_and_height(ui);
        ui.heading("Import report");
        egui::ScrollArea::vertical()
            .max_height(height)
            .show(ui, |ui| {
                for (index, record) in document.imported_from.iter().enumerate().rev() {
                    ui.push_id(index, |ui| {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(&record.source_name)
                                .font(theme::semibold(type_size::BODY)),
                        );
                        show_totals(ui, record);
                        show_remote_locations(ui, document, record);
                        show_record_notes(ui, record);
                        ui.label(format!(
                            "{} media online · {} offline",
                            record.media.online, record.media.offline
                        ));
                        ui.collapsing("Source details", |ui| {
                            ui.label(format!("{:?} {}", record.format, record.format_version));
                            ui.label(format!(
                                "Start offset: {} frames",
                                record.start_offset_frames
                            ));
                            ui.small(format!("SHA-256 {}", record.source_sha256));
                        });
                    });
                }
            });
        close = ui.add(primary_button("Close")).clicked();
    });
    close || response.should_close()
}

fn show_remote_locations(ui: &mut egui::Ui, document: &Document, record: &ImportRecord) {
    if record.media.offline == 0 {
        return;
    }
    let locators: Vec<_> = document
        .media_pool
        .iter()
        .filter_map(|asset| {
            let locator = asset.remote_locator.as_ref()?;
            record
                .estimated_asset(asset.id)
                .then_some(locator.as_text())
        })
        .collect();
    if locators.is_empty() {
        return;
    }
    ui.label("Remote media is offline");
    egui::ScrollArea::vertical()
        .id_salt("report-remote-list")
        .max_height(100.0)
        .show_rows(
            ui,
            ui.text_style_height(&egui::TextStyle::Body),
            locators.len(),
            |ui, range| {
                for &locator in &locators[range] {
                    ui.add(egui::Label::new(locator).truncate())
                        .on_hover_text(locator);
                }
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ix1_tests::{ProbeHold, RecordingMedia, write_edit};
    use kinewright_core::{AssetId, RefusedMedia};
    use kinewright_media::test_support::TempDirectory;
    use kinewright_project::interchange::commit;

    fn input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        }
    }

    fn settle(dialog: &mut ImportDialog) {
        let Preparation::Running(receiver) = &dialog.state else {
            panic!("preparation was not running");
        };
        let result = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        dialog.finish_preparation(result);
    }

    fn ready(dialog: &ImportDialog) -> &PreparedImport {
        let Preparation::Ready(prepared) = &dialog.state else {
            panic!("import is not ready");
        };
        prepared
    }

    fn painted_dialog(ctx: &egui::Context, dialog: &mut ImportDialog) -> Vec<String> {
        let _ = ctx.run_ui(input(), |ui| {
            assert!(matches!(dialog.show(ui.ctx()), ImportAction::Wait));
        });
        let output = ctx.run_ui(input(), |ui| {
            assert!(matches!(dialog.show(ui.ctx()), ImportAction::Wait));
        });
        crate::theme::painted_text(&output)
    }

    fn with_key(key: egui::Key) -> egui::RawInput {
        let mut input = input();
        input.events.push(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        input
    }

    #[test]
    fn ix1_timeline_filter_and_drop_open_supported_sources_in_the_same_dialog() {
        let directory = TempDirectory::new("ix1-open");
        let file = directory.path("source.OTIO");
        let bundle = directory.path("source.otiod");
        std::fs::create_dir(&bundle).unwrap();
        let main = bundle.join("content.otio");
        write_edit(&file, "not-present.mov");
        write_edit(&main, "not-present.mov");
        assert_eq!(
            OPEN_FILTER,
            ("Timeline (OTIO, FCPXML)", &["otio", "fcpxml"][..])
        );
        assert_eq!(
            classify_open_path(Path::new("edit.fcpxml")),
            OpenPath::Timeline { otio: false }
        );
        assert_eq!(
            classify_open_path(Path::new("edit.fcpxmld")),
            OpenPath::Timeline { otio: false }
        );
        assert_eq!(
            classify_open_path(Path::new("edit.kinewright")),
            OpenPath::Project
        );

        let ctx = egui::Context::default();
        let mut dropped = input();
        dropped.dropped_files.push(egui::DroppedFile {
            path: Some(bundle.clone()),
            ..Default::default()
        });
        let _ = ctx.run_ui(dropped, |ui| {
            assert_eq!(
                crate::media_bin::dropped_paths(ui.ctx()),
                std::slice::from_ref(&bundle)
            );
        });
        for path in [file, bundle, main] {
            assert_eq!(classify_open_path(&path), OpenPath::Timeline { otio: true });
            let mut dialog = ImportDialog::new(path, true, Arc::new(RefusedMedia), &ctx);
            settle(&mut dialog);
            assert_eq!(ready(&dialog).name(), "Reel Two");
            assert_eq!(ready(&dialog).record().items, 1);
        }
    }

    #[test]
    fn ix1_escape_cancels_an_import_while_hash_bytes_are_reported() {
        let directory = TempDirectory::new("ix1-cancel");
        let file = directory.path("edit.otio");
        std::fs::write(directory.path("source.mov"), b"mock probe input").unwrap();
        write_edit(&file, "source.mov");

        let (start_tx, start_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (finish_tx, finish_rx) = mpsc::sync_channel(1);
        let backend = RecordingMedia::holding(ProbeHold::new(start_tx, release_rx, finish_tx));
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut dialog = ImportDialog::new(file, true, Arc::new(backend), &ctx);
        start_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let painted = painted_dialog(&ctx, &mut dialog);
        assert!(
            painted.iter().any(|line| line.contains("64 bytes read")),
            "{painted:?}"
        );
        let _ = ctx.run_ui(with_key(egui::Key::Escape), |ui| {
            assert!(matches!(dialog.show(ui.ctx()), ImportAction::Cancel));
        });
        assert!(dialog.options.control.is_cancelled());
        settle(&mut dialog);
        assert!(
            matches!(&dialog.state, Preparation::Refused(refusal) if refusal.code == RefusalCode::Cancelled)
        );
        release_tx.send(()).unwrap();
        assert!(finish_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }

    #[test]
    fn ix1_locating_a_folder_shows_the_unique_match_and_leaves_ambiguous_media_offline() {
        let directory = TempDirectory::new("ix1-locate");
        let source_dir = directory.path("timeline");
        let root = directory.path("media");
        std::fs::create_dir(&source_dir).unwrap();
        std::fs::create_dir(&root).unwrap();
        let file = source_dir.join("edit.otio");
        write_edit(&file, "take.mov");
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let first = root.join("take.mov");
        std::fs::write(&first, b"mock probe input").unwrap();
        let mut dialog = ImportDialog::new(file, true, Arc::new(RecordingMedia::default()), &ctx);
        settle(&mut dialog);
        assert_eq!(ready(&dialog).media()[0].status, MediaStatus::Missing);
        dialog.locate_folder(&root, &ctx);
        settle(&mut dialog);
        let unique = &ready(&dialog).media()[0];
        assert_eq!(unique.status, MediaStatus::Online);
        assert!(unique.relinked_by_name);
        assert_eq!(unique.resolved_path.as_deref(), Some(first.as_path()));
        dialog.selected_media = Some(0);
        assert!(
            painted_dialog(&ctx, &mut dialog)
                .iter()
                .any(|line| line.contains("Found by file name"))
        );

        let nested = directory.path("other-media");
        std::fs::create_dir(&nested).unwrap();
        let other = nested.join("take.mov");
        std::fs::write(&other, b"another mock probe input").unwrap();
        dialog.locate_folder(&nested, &ctx);
        settle(&mut dialog);
        let ambiguous = &ready(&dialog).media()[0];
        assert_eq!(ambiguous.status, MediaStatus::Ambiguous);
        assert!(ambiguous.resolved_path.is_none());
        let matches = ambiguous.matches.as_deref().unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0], first);
        assert_eq!(matches[1], other);
        assert_eq!(ready(&dialog).record().media.offline, 1);
        dialog.selected_media = Some(0);
        let painted = painted_dialog(&ctx, &mut dialog);
        assert!(
            painted.iter().any(|line| line.contains("several matches")),
            "{painted:?}"
        );
        for path in [&first, &other] {
            assert!(
                painted.iter().any(|line| line == &path.to_string_lossy()),
                "{painted:?}"
            );
        }
    }

    #[test]
    fn ix1_open_publishes_the_prepared_timeline_and_report_paints_the_saved_locator() {
        let directory = TempDirectory::new("ix1-report");
        let path = directory.path("first-edit.otio");
        let remote = "file://unapproved.invalid/Camera/source.mov";
        write_edit(&path, remote);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut dialog = ImportDialog::new(path, true, Arc::new(RefusedMedia), &ctx);
        settle(&mut dialog);
        let prepared = ready(&dialog);
        let expected_status = import_status(prepared.name(), prepared.record());
        let painted = painted_dialog(&ctx, &mut dialog);
        assert!(
            painted.iter().any(|line| line.contains(remote)),
            "{painted:?}"
        );
        #[cfg(not(windows))]
        assert!(!painted.iter().any(|line| line.starts_with("Allow ")));
        let mut action = ImportAction::Wait;
        let _ = ctx.run_ui(with_key(egui::Key::Enter), |ui| {
            action = dialog.show(ui.ctx());
        });
        let ImportAction::Open(prepared) = action else {
            panic!("Enter on the focused Open button did not open the timeline");
        };
        drop(dialog);
        let (name, document) = commit(*prepared, |name, doc, _| Ok::<_, ()>((name, doc))).unwrap();
        assert_eq!(name, "Reel Two");
        assert_eq!(
            document.asset(AssetId(1)).unwrap().display_location(),
            remote
        );
        assert!(expected_status.starts_with("Imported Reel Two: "));
        let _ = ctx.run_ui(input(), |ui| assert!(!show_report(ui.ctx(), &document)));
        let output = ctx.run_ui(input(), |ui| assert!(!show_report(ui.ctx(), &document)));
        let painted = crate::theme::painted_text(&output);
        for expected in ["Import report", "first-edit.otio", remote] {
            assert!(
                painted.iter().any(|line| line.contains(expected)),
                "{painted:?}"
            );
        }
    }

    #[test]
    fn ix1_a_malformed_timeline_shows_the_typed_refusal_in_the_modal() {
        let directory = TempDirectory::new("ix1-refusal");
        let input = directory.path("edit.otio");
        std::fs::write(&input, b"{invalid json").unwrap();
        let ctx = egui::Context::default();
        let mut dialog = ImportDialog::new(input, true, Arc::new(RefusedMedia), &ctx);
        settle(&mut dialog);
        let Preparation::Refused(refusal) = &dialog.state else {
            panic!("malformed source was not refused");
        };
        let message = refusal.to_string();
        assert_eq!(refusal.code, RefusalCode::Malformed);
        let painted = painted_dialog(&ctx, &mut dialog);
        assert!(
            painted.iter().any(|line| line == "malformed"),
            "{painted:?}"
        );
        assert!(painted.iter().any(|line| line == &message), "{painted:?}");
    }
}

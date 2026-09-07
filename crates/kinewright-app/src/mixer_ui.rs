//! The AU1 manual mixer: one strip per track, one per bus, one for master.
//!
//! The panel is the human half of AU1 §5.1. It reads the document's
//! `audio_mix.tracks` and the engine's `mix_peaks()` and writes exactly one
//! kind of operation, [`Operation::SetTrackMix`]. Bus and master strips are
//! read-only in this slice.
//!
//! Everything that paints is a free function taking `&mut egui::Ui`, so the
//! headless `ctx.run_ui` test pattern can assert what a strip says and that a
//! frame of painting writes nothing.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use kinewright_core::{
    AudioBus, AudioBusId, Document, MixPeaks, Operation, TRACK_MIX_GAIN_MAX, TRACK_MIX_GAIN_MIN,
    TRACK_MIX_PAN_MAX, TRACK_MIX_PAN_MIN, Track, TrackId, TrackKind, TrackMix,
};

use crate::{
    app::KinewrightApp,
    icons::Icon,
    inspector_ui::{InspectorEdits, clip_carries_audio, is_live_drag},
    theme::{self, color, radius, size, space, type_size},
    // The thresholds and the decay rate are the transport meter's, imported
    // rather than restated so the two meters cannot drift (AU1 §5.1).
    transport::{DANGER_START, DECAY_PER_SECOND, WARNING_START, peak_to_meter_level},
};

// Where the strip put each of its controls, recorded for tests only.
//
// A slider's rail is allocated by egui, so nothing in this module knows its
// rectangle until the widget has been laid out. The input-driven tests press
// real pointer buttons at real coordinates, and a guessed coordinate would
// make them pass for the wrong reason, so the strip writes down where it put
// each control as it goes. The recorder is thread-local, so parallel tests
// cannot see each other's frames; in a release build it is not compiled at
// all.
#[cfg(test)]
thread_local! {
    static STRIP_RECTS: std::cell::RefCell<Vec<(&'static str, egui::Rect)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn record_strip_rect(name: &'static str, rect: egui::Rect) {
    STRIP_RECTS.with(|rects| rects.borrow_mut().push((name, rect)));
}

#[cfg(not(test))]
#[allow(clippy::inline_always)]
#[inline(always)]
const fn record_strip_rect(_name: &'static str, _rect: egui::Rect) {}

/// The two per-track mix toggles, which share one visual rule (AU1 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MixToggle {
    Mute,
    Solo,
}

impl MixToggle {
    const fn label(self) -> &'static str {
        match self {
            Self::Mute => "M",
            Self::Solo => "S",
        }
    }

    pub(crate) const fn tooltip(self) -> &'static str {
        match self {
            Self::Mute => "Mute track",
            Self::Solo => "Solo track",
        }
    }

    /// The text colour when the toggle is on.
    ///
    /// Mute is a functional warning — the track is not being heard — and takes
    /// `STATUS_WARNING`, as the timeline's `FREE` sync-lock label does. Solo is
    /// a plain state and takes `TEXT_PRIMARY`. Neither ever uses the accent:
    /// track-header controls are not selection (DESIGN.md).
    const fn active_text(self) -> egui::Color32 {
        match self {
            Self::Mute => color::STATUS_WARNING,
            Self::Solo => color::TEXT_PRIMARY,
        }
    }

    pub(crate) const fn is_on(self, mix: TrackMix) -> bool {
        match self {
            Self::Mute => mix.mute,
            Self::Solo => mix.solo,
        }
    }
}

/// One displayed meter level per mix point, decayed frame by frame.
///
/// Peaks arrive as instantaneous chunk maxima; a bar that followed them
/// directly would flicker. The app keeps the displayed level here and lets it
/// fall at [`DECAY_PER_SECOND`], exactly as the transport's master meter does.
#[derive(Debug, Default)]
pub(crate) struct MixerMeterLevels {
    tracks: HashMap<TrackId, [f32; 2]>,
    buses: HashMap<AudioBusId, [f32; 2]>,
    master: [f32; 2],
}

impl MixerMeterLevels {
    /// Advance every displayed level towards this frame's peaks.
    ///
    /// Peaks are only read while the transport is running: a stopped engine
    /// reports nothing, so every meter falls to silence and stays there.
    fn advance(&mut self, ui: &egui::Ui, peaks: &MixPeaks, playing: bool) {
        let elapsed = ui.input(|input| input.stable_dt).clamp(0.0, 0.1);
        for level in self
            .tracks
            .values_mut()
            .chain(self.buses.values_mut())
            .chain(std::iter::once(&mut self.master))
            .flatten()
        {
            *level = decayed_level(*level, 0.0, elapsed);
        }
        if playing {
            for (track, peak) in &peaks.tracks {
                let entry = self.tracks.entry(*track).or_default();
                raise(entry, *peak);
            }
            for (bus, peak) in &peaks.buses {
                let entry = self.buses.entry(*bus).or_default();
                raise(entry, *peak);
            }
            raise(&mut self.master, peaks.master);
        }
        // A silent entry carries no information, and a removed track must not
        // leave one behind; the lookups below read silence for a missing key.
        self.tracks
            .retain(|_, level| level.iter().any(|l| *l > 0.0));
        self.buses.retain(|_, level| level.iter().any(|l| *l > 0.0));
        if self.any_lit() {
            // Decay is animation: without this the last frame of a stopped
            // transport would leave the meter frozen part-way down.
            ui.ctx().request_repaint();
        }
    }

    fn any_lit(&self) -> bool {
        self.tracks
            .values()
            .chain(self.buses.values())
            .chain(std::iter::once(&self.master))
            .flatten()
            .any(|level| *level > 0.0)
    }

    fn track(&self, track: TrackId) -> [f32; 2] {
        self.tracks.get(&track).copied().unwrap_or_default()
    }

    fn bus(&self, bus: AudioBusId) -> [f32; 2] {
        self.buses.get(&bus).copied().unwrap_or_default()
    }

    const fn master(&self) -> [f32; 2] {
        self.master
    }
}

/// Lift a decayed level to this frame's peak. The rise is instant; only the
/// fall is timed.
fn raise(level: &mut [f32; 2], peak: [f32; 2]) {
    for (display, peak) in level.iter_mut().zip(peak) {
        *display = display.max(peak_to_meter_level(peak));
    }
}

/// One frame of the meter's ballistics: rise instantly to the peak, fall at
/// [`DECAY_PER_SECOND`]. Identical to the transport meter's rule.
fn decayed_level(previous: f32, peak: f32, elapsed: f32) -> f32 {
    peak_to_meter_level(peak).max((previous - DECAY_PER_SECOND * elapsed).max(0.0))
}

impl KinewrightApp {
    /// The Mixer tab of the material strip (AU1 §5.1).
    pub(crate) fn mixer_panel(&mut self, ui: &mut egui::Ui) {
        let document = Arc::clone(&self.focused().document);
        // Peaks are telemetry from a running engine. A stopped transport has
        // no signal to report, so the meters read what is true: silence.
        let peaks = if self.playing {
            self.playback.mix_peaks()
        } else {
            MixPeaks::default()
        };
        let mut edits = InspectorEdits::default();
        mixer_strips(
            ui,
            &document,
            &peaks,
            self.playing,
            &mut self.mixer_levels,
            &mut edits,
        );
        self.submit_inspector_edits(edits);
    }
}

/// Paint the whole mixer: tracks, then buses, then master (AU1 §5.1).
pub(crate) fn mixer_strips(
    ui: &mut egui::Ui,
    document: &Document,
    peaks: &MixPeaks,
    playing: bool,
    levels: &mut MixerMeterLevels,
    edits: &mut InspectorEdits,
) {
    levels.advance(ui, peaks, playing);
    // Horizontal for the strips themselves; vertical as a fallback, so a dock
    // dragged shorter than a strip still reaches every control.
    egui::ScrollArea::both()
        .id_salt("mixer-strips")
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for (index, track) in document.tracks.iter().enumerate() {
                    track_strip(ui, document, track, index, levels, edits);
                }
                if !document.audio_mix.buses.is_empty() {
                    // One rule per boundary: with no buses the tracks and the
                    // master meet at a single separator, not two 12 px apart.
                    ui.separator();
                    for bus in &document.audio_mix.buses {
                        bus_strip(ui, document, bus, levels);
                    }
                }
                ui.separator();
                master_strip(ui, levels);
            });
        });
}

/// One track strip: caption, meters beside the fader, pan, mute and solo.
///
/// The rows are compact on purpose (AU1 §5.1 as amended): stacking the meters
/// above the fader made a 374 px strip, and the material strip cannot show one
/// that tall without scrolling past the controls that matter.
fn track_strip(
    ui: &mut egui::Ui,
    document: &Document,
    track: &Track,
    index: usize,
    levels: &MixerMeterLevels,
    edits: &mut InspectorEdits,
) {
    let mix = document.track_mix(track.id);
    let carries_audio = track_carries_audio(document, track);
    strip(ui, |ui| {
        caption_row(ui, track.kind, index, carries_audio);

        let (fader, gain_tenth_db) = meters_and_fader(ui, levels.track(track.id), mix);
        record_strip_rect("fader", fader.rect);
        if silenced_by_another_solo(document, mix) && carries_audio {
            // Directly under the meters, which are the left edge of the row
            // above: this is the reading the label explains. A track with no
            // audio-bearing clip has already said `NO AUDIO`, which is the
            // more specific reason for the same silence.
            ui.label(theme::caps_label(SOLO_MUTED_LABEL, color::TEXT_MUTED));
        }
        record_mix_edit(
            edits,
            &fader,
            mix,
            TrackMix {
                gain_tenth_db,
                ..mix
            },
        );

        let (pan, pan_percent) = pan_control(ui, mix);
        record_strip_rect("pan", pan.rect);
        record_mix_edit(edits, &pan, mix, TrackMix { pan_percent, ..mix });

        ui.horizontal(|ui| {
            for toggle in [MixToggle::Mute, MixToggle::Solo] {
                if mix_toggle_button(ui, toggle, mix) {
                    edits.push(track_mix_toggle_operation(mix, toggle));
                }
            }
        });

        reset_row(ui, mix, edits);
    });
}

/// The micro caps label a track wears when some other track's solo is what
/// silenced it (AU1 §5.1).
///
/// Eight characters, inside DESIGN.md's twelve-character cap on an uppercase
/// machine-state label, and at the house tracking narrower than the 72 px
/// strip so it sits on one line; `MUTED BY SOLO` (thirteen characters,
/// ~97 px) and `SOLO MUTED` (~77 px) both wrapped.
const SOLO_MUTED_LABEL: &str = "SILENCED";

/// The strip's `Reset` row, shown only when there is something to reset.
///
/// The rails are drag-sensing widgets: egui never reports a click, let alone a
/// double click, on a slider's rail, so the double-click reset the first build
/// carried could not fire there and wrote whatever value the second press
/// landed on instead. The strip therefore states the gesture the way the
/// inspector's audio section does — a small `Reset` button that appears once
/// the values are off neutral and pushes one discrete operation.
fn reset_row(ui: &mut egui::Ui, mix: TrackMix, edits: &mut InspectorEdits) {
    if mix.is_neutral() {
        return;
    }
    let response = ui
        .small_button("Reset")
        .on_hover_text("Return this track to unity gain, centre pan, unmuted and unsoloed.");
    record_strip_rect("reset", response.rect);
    if response.clicked() {
        edits.push(track_mix_operation(TrackMix::neutral(mix.track)));
    }
}

/// The strip's first row: caption, kind icon, and the `NO AUDIO` state.
fn caption_row(ui: &mut egui::Ui, kind: TrackKind, index: usize, carries_audio: bool) {
    let (caption, icon) = track_caption_and_icon(kind, index);
    ui.scope(|ui| {
        // A row's height is fixed when it is created, from the parent's
        // interact size. The caption row holds no interactive widget, so it
        // does not owe the strip a control's worth of height.
        ui.spacing_mut().interact_size.y = size::ICON_SM;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::HALF;
            ui.label(
                egui::RichText::new(&caption)
                    .font(egui::FontId::new(
                        type_size::CAPTION,
                        egui::FontFamily::Proportional,
                    ))
                    .color(color::TEXT_PRIMARY),
            );
            ui.add(icon.image(size::ICON_SM).tint(color::TEXT_MUTED));
        });
        if !carries_audio {
            // The controls stay enabled: a mix can be set before the media
            // arrives, and the label says why nothing is heard yet.
            //
            // The label sits under the caption rather than beside it: the
            // caption, the kind icon, and a tracked eight-character caps label
            // are 90 px of content in a 72 px strip.
            ui.label(theme::caps_label("NO AUDIO", color::TEXT_MUTED));
        }
    });
}

/// Meters on the left, fader on the right, both `MIXER_FADER_HEIGHT` tall.
///
/// The pair reads as one control surface and costs the strip one row instead
/// of two. Returns the fader's response and the gain it now shows.
fn meters_and_fader(ui: &mut egui::Ui, meters: [f32; 2], mix: TrackMix) -> (egui::Response, i32) {
    let mut gain_tenth_db = mix.gain_tenth_db;
    let response = ui
        .scope(|ui| {
            // The row's height is fixed when it is created: the readout under
            // the fader is a value, not a control surface, so the row does not
            // owe it a button's height.
            ui.spacing_mut().interact_size.y = size::ICON_SM;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = space::ONE;
                meter_pair(ui, meters);
                ui.scope(|ui| {
                    // A vertical slider takes its length from `slider_width`.
                    ui.spacing_mut().slider_width = size::MIXER_FADER_HEIGHT;
                    // The readout is set in micro with no padding of its own,
                    // so the dB figure fits the strip instead of widening the
                    // column past the meters beside it.
                    ui.style_mut().override_font_id = Some(theme::medium(type_size::MICRO));
                    ui.spacing_mut().button_padding.y = 0.0;
                    ui.add(
                        egui::Slider::new(
                            &mut gain_tenth_db,
                            TRACK_MIX_GAIN_MIN..=TRACK_MIX_GAIN_MAX,
                        )
                        .vertical()
                        .integer()
                        .custom_formatter(|value, _| format!("{:+.1} dB", value / 10.0))
                        .custom_parser(parse_gain_db)
                        .update_while_editing(false),
                    )
                })
                .inner
            })
            .inner
        })
        .inner;
    (response, gain_tenth_db)
}

/// The pan rail and its `L50` / `C` / `R50` readout.
fn pan_control(ui: &mut egui::Ui, mix: TrackMix) -> (egui::Response, i32) {
    let mut pan_percent = mix.pan_percent;
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().interact_size.y = size::ICON_SM;
            // The rail and its readout share the strip's width: a little under
            // half for the rail, the rest for the position.
            ui.spacing_mut().slider_width = size::MIXER_STRIP_WIDTH / 2.0 - space::TWO;
            ui.spacing_mut().item_spacing.x = space::HALF;
            ui.style_mut().override_font_id = Some(theme::medium(type_size::MICRO));
            ui.add(
                egui::Slider::new(&mut pan_percent, TRACK_MIX_PAN_MIN..=TRACK_MIX_PAN_MAX)
                    .integer()
                    .custom_formatter(|value, _| format_pan_value(value))
                    .custom_parser(parse_pan)
                    .update_while_editing(false),
            )
        })
        .inner;
    (response, pan_percent)
}

/// File one frame of one strip control: open a gesture on the press and write
/// the whole mix state on a change.
///
/// Live drag frames carry the track's coalesce key so the drag lands as one
/// undo entry; a release or a value typed into the readout is discrete. The
/// fader and the pan share the key because one strip runs one gesture at a
/// time. Returning to neutral is [`reset_row`]'s job, not a gesture on the
/// rail: the rail senses drags only.
fn record_mix_edit(
    edits: &mut InspectorEdits,
    response: &egui::Response,
    mix: TrackMix,
    edited: TrackMix,
) {
    if response.drag_started() {
        edits.begin_gesture();
    }
    // A readout that commits on Enter or blur reports one `changed()` frame
    // with the unchanged value; a write that changes nothing is not an edit.
    if response.changed() && edited != mix {
        let operation = track_mix_operation(edited);
        if is_live_drag(response) {
            edits.push_live(operation, track_mix_coalesce_key(mix.track));
        } else {
            edits.push(operation);
        }
    }
}

/// One read-only bus strip (AU1 §5.1). Bus editing arrives with AU2.
fn bus_strip(ui: &mut egui::Ui, document: &Document, bus: &AudioBus, levels: &MixerMeterLevels) {
    strip(ui, |ui| {
        ui.label(
            egui::RichText::new(&bus.name)
                .font(egui::FontId::new(
                    type_size::CAPTION,
                    egui::FontFamily::Proportional,
                ))
                .color(color::TEXT_PRIMARY),
        );
        ui.colored_label(
            color::TEXT_MUTED,
            format!("tracks={}", track_caption_list(document, &bus.tracks)),
        );
        if !bus.effects.is_empty() {
            let chain = bus
                .effects
                .iter()
                .map(|effect| effect.name.trim_start_matches("audio_").to_owned())
                .collect::<Vec<_>>()
                .join(" → ");
            ui.colored_label(color::TEXT_MUTED, chain);
        }
        if !bus.ducking_sidechain_tracks.is_empty() {
            ui.colored_label(
                color::TEXT_MUTED,
                format!(
                    "sidechain={}",
                    track_caption_list(document, &bus.ducking_sidechain_tracks)
                ),
            );
        }
        meter_pair(ui, levels.bus(bus.id));
        ui.colored_label(
            color::TEXT_MUTED,
            "Bus controls arrive with AU2; the agent can edit buses today.",
        );
    });
}

/// The master strip: the post-limiter meter and nothing to touch (AU1 §5.1).
fn master_strip(ui: &mut egui::Ui, levels: &MixerMeterLevels) {
    strip(ui, |ui| {
        ui.label(theme::caps_label("MASTER", color::TEXT_MUTED));
        meter_pair(ui, levels.master());
    });
}

/// Allocate one strip column and run its contents inside it.
fn strip<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.vertical(|ui| {
        ui.set_min_width(size::MIXER_STRIP_WIDTH);
        ui.set_max_width(size::MIXER_STRIP_WIDTH);
        // A strip is one control, not a stack of sections: its rows sit closer
        // together than panel content does, which is what keeps the whole
        // strip inside the dock.
        ui.spacing_mut().item_spacing.y = space::HALF;
        contents(ui)
    })
    .inner
}

/// The L/R meter pair of one mix point.
fn meter_pair(ui: &mut egui::Ui, levels: [f32; 2]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::HALF;
        for level in levels {
            draw_vertical_meter(ui, level);
        }
    });
}

/// One vertical meter bar, filling from the bottom (AU1 §5.1).
fn draw_vertical_meter(ui: &mut egui::Ui, level: f32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(size::MIXER_METER_WIDTH, size::MIXER_FADER_HEIGHT),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 1.0, color::SURFACE_ACTIVE);
    let level = level.clamp(0.0, 1.0);
    draw_meter_segment(
        ui,
        rect,
        0.0,
        level.min(WARNING_START),
        color::STATUS_SUCCESS,
    );
    draw_meter_segment(
        ui,
        rect,
        WARNING_START,
        level.min(DANGER_START),
        color::STATUS_WARNING,
    );
    draw_meter_segment(ui, rect, DANGER_START, level, color::STATUS_DANGER);
}

fn draw_meter_segment(ui: &egui::Ui, rect: egui::Rect, start: f32, end: f32, fill: egui::Color32) {
    if end <= start {
        return;
    }
    // Fill grows upwards from the bottom edge, so the fractions run from 1.0.
    let bottom = egui::lerp(rect.y_range(), 1.0 - start);
    let top = egui::lerp(rect.y_range(), 1.0 - end);
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(rect.left(), top),
            egui::pos2(rect.right(), bottom),
        ),
        0.0,
        fill,
    );
}

/// One `M` or `S` square in the mixer strip. Returns whether it was clicked.
fn mix_toggle_button(ui: &mut egui::Ui, toggle: MixToggle, mix: TrackMix) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(size::ICON_BUTTON, size::ICON_BUTTON),
        egui::Sense::click(),
    );
    let response = response.on_hover_text(toggle.tooltip());
    record_strip_rect(toggle.label(), rect);
    paint_mix_toggle(&ui.painter_at(rect), rect, toggle, toggle.is_on(mix));
    response.clicked()
}

/// Paint one mute or solo toggle (AU1 §5.1 item 5).
///
/// Shared with the timeline track header so the two surfaces cannot drift.
pub(crate) fn paint_mix_toggle(
    painter: &egui::Painter,
    rect: egui::Rect,
    toggle: MixToggle,
    active: bool,
) {
    if active {
        painter.rect_filled(rect, radius::SM, color::SURFACE_ACTIVE);
    }
    let text = if active {
        toggle.active_text()
    } else {
        color::TEXT_MUTED
    };
    theme::paint_caps(
        painter,
        rect.center(),
        egui::Align2::CENTER_CENTER,
        toggle.label(),
        text,
    );
}

/// The whole mix state of one track as an operation (AU1 §2.3).
///
/// `SetTrackMix` is an idempotent full set, so every control in the strip
/// sends all four values and changes exactly the one it owns.
pub(crate) const fn track_mix_operation(mix: TrackMix) -> Operation {
    Operation::SetTrackMix {
        track: mix.track,
        gain_tenth_db: mix.gain_tenth_db,
        pan_percent: mix.pan_percent,
        mute: mix.mute,
        solo: mix.solo,
    }
}

/// Flip one toggle and carry the other three values unchanged (AU1 §5.2).
pub(crate) const fn track_mix_toggle_operation(mix: TrackMix, toggle: MixToggle) -> Operation {
    let flipped = match toggle {
        MixToggle::Mute => TrackMix {
            mute: !mix.mute,
            ..mix
        },
        MixToggle::Solo => TrackMix {
            solo: !mix.solo,
            ..mix
        },
    };
    track_mix_operation(flipped)
}

/// Stable coalesce key for one live track-mix gesture (AU1 §5.1).
pub(crate) fn track_mix_coalesce_key(track: TrackId) -> String {
    format!("track_mix:{}", track.0)
}

/// The timeline's caption and icon for a track, so the mixer and the timeline
/// name the same track the same way.
pub(crate) fn track_caption_and_icon(kind: TrackKind, index: usize) -> (String, Icon) {
    match kind {
        TrackKind::Video => (format!("V{}", index + 1), Icon::Filmstrip),
        TrackKind::Audio => (format!("A{}", index + 1), Icon::Waveform),
    }
}

/// `A2 A3`: bus membership named the way the timeline names tracks.
fn track_caption_list(document: &Document, tracks: &[TrackId]) -> String {
    tracks
        .iter()
        .map(|track| track_caption(document, *track))
        .collect::<Vec<_>>()
        .join(" ")
}

fn track_caption(document: &Document, track: TrackId) -> String {
    document
        .tracks
        .iter()
        .position(|candidate| candidate.id == track)
        .map_or_else(
            || format!("track {track}"),
            |index| track_caption_and_icon(document.tracks[index].kind, index).0,
        )
}

/// Whether any clip on the track carries audio, the same test the inspector's
/// clip audio section makes per clip — the inspector's function, so the two
/// surfaces cannot disagree about what "no audio" means.
fn track_carries_audio(document: &Document, track: &Track) -> bool {
    track
        .clips
        .iter()
        .any(|clip| clip_carries_audio(document, clip))
}

/// Whether this track is silent only because some other track is soloed.
///
/// A track muted by its own switch says so with its own lit `M`; this label is
/// for the track that did nothing and went quiet anyway (AU1 §5.1).
fn silenced_by_another_solo(document: &Document, mix: TrackMix) -> bool {
    !mix.mute && !mix.solo && document.audio_mix.any_solo()
}

/// egui hands a formatter an `f64`; every pan percent in -100..=100 is exact
/// in `f64`, so the round trip cannot move the value.
#[allow(clippy::cast_possible_truncation)]
fn format_pan_value(value: f64) -> String {
    format_pan(value.round() as i32)
}

/// Read a value typed into the fader's readout back as decibels.
///
/// The readout is formatted in decibels but the slider's own number is tenths
/// of a decibel, so egui's default parser would read `-6.0` — exactly what the
/// field had just shown — as −0.6 dB. Every formatter needs its parser.
pub(crate) fn parse_gain_db(text: &str) -> Option<f64> {
    let text = text.trim();
    let number = text.trim_end_matches(|character: char| {
        character.is_ascii_alphabetic() || character.is_whitespace()
    });
    number.trim().parse::<f64>().ok().map(|db| db * 10.0)
}

/// Read a value typed into the pan readout back, in the spelling it shows.
///
/// `C`, `L50`, `R50`, and a bare signed percent all mean what they look like;
/// without this the default parser rejects everything the field can display.
fn parse_pan(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("c") {
        return Some(0.0);
    }
    let (sign, rest) = match text.chars().next().map(|first| first.to_ascii_uppercase()) {
        Some('L') => (-1.0, &text[1..]),
        Some('R') => (1.0, &text[1..]),
        _ => (1.0, text),
    };
    rest.trim()
        .parse::<f64>()
        .ok()
        .map(|percent| sign * percent)
}

/// `L50` / `C` / `R50`, the pan positions spelled the way a mixer spells them.
fn format_pan(pan_percent: i32) -> String {
    match pan_percent {
        0 => "C".to_owned(),
        pan if pan < 0 => format!("L{}", -pan),
        pan => format!("R{pan}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kinewright_core::{
        AssetId, AudioMix, Clip, ClipContent, ClipId, Effect, MediaAsset, MediaKind, Rational,
        TimeCode,
    };

    use super::*;

    fn asset() -> MediaAsset {
        MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("take.mp4"),
            name: "take.mp4".to_owned(),
            duration: TimeCode(120),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        }
    }

    /// Two tracks: the first carries an A/V clip, the second is empty and must
    /// therefore say `NO AUDIO`.
    fn mixer_document() -> Document {
        let asset = asset();
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![Clip {
                        id: ClipId(1),
                        asset: asset.id,
                        source_range: TimeCode(0)..TimeCode(30),
                        content: ClipContent::Media,
                        timeline_start: TimeCode(0),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
                    }],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            ],
            audio_mix: AudioMix {
                buses: vec![AudioBus {
                    id: AudioBusId(1),
                    name: "Dialogue".to_owned(),
                    tracks: vec![TrackId(2)],
                    effects: vec![Effect {
                        id: kinewright_core::EffectId(1),
                        name: "audio_gain".to_owned(),
                        parameters: std::collections::BTreeMap::new(),
                        keyframes: std::collections::BTreeMap::new(),
                    }],
                    ducking_sidechain_tracks: vec![TrackId(1)],
                }],
                tracks: Vec::new(),
            },
            media_pool: vec![asset],
            fps: Rational::new(30, 1).unwrap(),
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    const fn mix(track: u64) -> TrackMix {
        TrackMix {
            track: TrackId(track),
            gain_tenth_db: -60,
            pan_percent: 25,
            mute: false,
            solo: true,
        }
    }

    /// AU1 §7 item 22: the strip's builders emit exactly the operation the
    /// contract names, carrying every value the control does not own.
    #[test]
    fn track_mix_operation_carries_the_whole_mix_state() {
        assert_eq!(
            track_mix_operation(mix(4)),
            Operation::SetTrackMix {
                track: TrackId(4),
                gain_tenth_db: -60,
                pan_percent: 25,
                mute: false,
                solo: true,
            }
        );
        assert_eq!(
            track_mix_operation(TrackMix {
                gain_tenth_db: 0,
                ..mix(4)
            }),
            Operation::SetTrackMix {
                track: TrackId(4),
                gain_tenth_db: 0,
                pan_percent: 25,
                mute: false,
                solo: true,
            },
            "a fader change keeps pan, mute, and solo"
        );
        assert_eq!(
            track_mix_operation(TrackMix::neutral(TrackId(4))),
            Operation::SetTrackMix {
                track: TrackId(4),
                gain_tenth_db: 0,
                pan_percent: 0,
                mute: false,
                solo: false,
            },
            "the Reset row clears all four values in one operation"
        );
    }

    /// AU1 §7 item 24: a header or strip toggle flips its own flag and nothing
    /// else.
    #[test]
    fn a_toggle_flips_only_its_own_flag() {
        assert_eq!(
            track_mix_toggle_operation(mix(4), MixToggle::Mute),
            Operation::SetTrackMix {
                track: TrackId(4),
                gain_tenth_db: -60,
                pan_percent: 25,
                mute: true,
                solo: true,
            }
        );
        assert_eq!(
            track_mix_toggle_operation(mix(4), MixToggle::Solo),
            Operation::SetTrackMix {
                track: TrackId(4),
                gain_tenth_db: -60,
                pan_percent: 25,
                mute: false,
                solo: false,
            }
        );
        // Flipping twice is the identity: no toggle can drift another value.
        let once = track_mix_toggle_operation(mix(4), MixToggle::Mute);
        let Operation::SetTrackMix {
            track,
            gain_tenth_db,
            pan_percent,
            mute,
            solo,
        } = once
        else {
            panic!("the toggle builds a track mix operation");
        };
        assert_eq!(
            track_mix_toggle_operation(
                TrackMix {
                    track,
                    gain_tenth_db,
                    pan_percent,
                    mute,
                    solo
                },
                MixToggle::Mute
            ),
            track_mix_operation(mix(4))
        );
    }

    /// AU1 §7 item 22: one key per track, distinct from the clip-level keys.
    #[test]
    fn track_mix_drags_coalesce_on_one_key_per_track() {
        assert_eq!(track_mix_coalesce_key(TrackId(3)), "track_mix:3");
        assert_ne!(
            track_mix_coalesce_key(TrackId(3)),
            track_mix_coalesce_key(TrackId(4)),
            "two tracks must not merge into one undo entry"
        );
        assert_ne!(
            track_mix_coalesce_key(TrackId(3)),
            "audio_gain:3",
            "the track fader and a clip's gain are different controls"
        );
    }

    #[test]
    fn pan_reads_as_left_centre_and_right() {
        assert_eq!(format_pan(0), "C");
        assert_eq!(format_pan(-100), "L100");
        assert_eq!(format_pan(50), "R50");
    }

    /// AU1 §5.1: a readout that can be typed into has to read back what it
    /// wrote. Both fields format their value, so both need their parser;
    /// egui's default one reads the slider's raw number, which is tenths of a
    /// decibel on the fader and nothing at all on the pan.
    #[test]
    fn a_typed_readout_round_trips_through_its_own_spelling() {
        for gain_tenth_db in [0, -60, 120, -600, 7] {
            let shown = format!("{:+.1} dB", f64::from(gain_tenth_db) / 10.0);
            assert_eq!(
                parse_gain_db(&shown),
                Some(f64::from(gain_tenth_db)),
                "the fader must read `{shown}` back as the value that wrote it"
            );
        }
        assert_eq!(parse_gain_db("-6"), Some(-60.0), "the suffix is optional");
        assert_eq!(parse_gain_db(""), None);
        assert_eq!(parse_gain_db("loud"), None);

        for pan_percent in [0, -100, 100, 25, -7] {
            let shown = format_pan(pan_percent);
            assert_eq!(
                parse_pan(&shown),
                Some(f64::from(pan_percent)),
                "the pan must read `{shown}` back as the value that wrote it"
            );
        }
        assert_eq!(parse_pan("-50"), Some(-50.0), "a bare percent still works");
        assert_eq!(parse_pan("c"), Some(0.0));
        assert_eq!(parse_pan("hard left"), None);
    }

    /// The mixer's meter ballistics are the transport's: instant rise, 0.9 per
    /// second fall, never below silence.
    #[test]
    fn meters_rise_instantly_and_decay_at_the_house_rate() {
        let lit = decayed_level(0.0, 1.0, 0.0);
        assert!((lit - 1.0).abs() < 1e-6);
        let after = decayed_level(lit, 0.0, 0.1);
        assert!((after - 0.91).abs() < 1e-6, "0.9 per second for 100 ms");
        assert!(
            decayed_level(0.0, 0.0, 1.0) <= 0.0,
            "silence cannot go lower"
        );
        assert!(
            decayed_level(0.2, 0.5, 0.1) > 0.2,
            "a louder peak lifts the bar in one frame"
        );
    }

    #[test]
    fn only_another_tracks_solo_reads_as_muted_by_solo() {
        let mut document = mixer_document();
        assert!(!silenced_by_another_solo(
            &document,
            document.track_mix(TrackId(1))
        ));
        document.audio_mix.tracks = vec![TrackMix {
            solo: true,
            ..TrackMix::neutral(TrackId(2))
        }];
        assert!(
            silenced_by_another_solo(&document, document.track_mix(TrackId(1))),
            "a track nobody touched went quiet and must say so"
        );
        assert!(
            !silenced_by_another_solo(&document, document.track_mix(TrackId(2))),
            "the soloed track is the one being heard"
        );
        document.audio_mix.tracks.push(TrackMix {
            mute: true,
            ..TrackMix::neutral(TrackId(3))
        });
        assert!(
            !silenced_by_another_solo(
                &document,
                TrackMix {
                    mute: true,
                    ..TrackMix::neutral(TrackId(3))
                }
            ),
            "a track muted by its own switch says so with its own M"
        );
    }

    #[test]
    fn a_track_without_audio_bearing_media_is_reported() {
        let document = mixer_document();
        assert!(track_carries_audio(&document, &document.tracks[0]));
        assert!(!track_carries_audio(&document, &document.tracks[1]));
    }

    /// Lay one track strip out on its own and report the space it takes.
    fn measure_track_strip(document: &Document, index: usize) -> egui::Vec2 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let levels = MixerMeterLevels::default();
        let mut edits = InspectorEdits::default();
        let mut size = egui::Vec2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.vertical(|ui| {
                track_strip(
                    ui,
                    document,
                    &document.tracks[index],
                    index,
                    &levels,
                    &mut edits,
                );
                size = ui.min_rect().size();
            });
        });
        size
    }

    /// AU1 §5.1 as amended: the strip has a height budget, because the Mixer
    /// dock opens at 300 px and a strip that overran it hid the fader and the
    /// M/S toggles below the fold.
    ///
    /// The worst case is the one to measure: a track that carries audio, has
    /// been moved off neutral, and has been silenced by another track's solo
    /// wears both the `SILENCED` line and the `Reset` row.
    #[test]
    fn a_track_strip_fits_the_mixer_dock() {
        // The `mixer-dock` default of 320 px leaves about 262 px for strips
        // after the margins, tab row, and separator (AU1 §5.1).
        const BUDGET: f32 = 240.0;
        let mut worst_case = mixer_document();
        // Track 1 carries audio and is off neutral; track 2 is soloed, so
        // track 1 also reads `SILENCED`.
        worst_case.audio_mix.tracks = vec![
            TrackMix {
                gain_tenth_db: -60,
                pan_percent: 25,
                ..TrackMix::neutral(TrackId(1))
            },
            TrackMix {
                solo: true,
                ..TrackMix::neutral(TrackId(2))
            },
        ];
        for (label, document, index) in [
            ("plain", mixer_document(), 0),
            ("no audio", mixer_document(), 1),
            ("solo muted and non-neutral", worst_case, 0),
        ] {
            let size = measure_track_strip(&document, index);
            assert!(
                size.y <= BUDGET,
                "a {label} track strip is {} px tall, over the {BUDGET} px budget",
                size.y
            );
            assert!(
                (size.x - size::MIXER_STRIP_WIDTH).abs() <= 1.0,
                "a {label} strip is one column wide: {} px",
                size.x
            );
        }
    }

    /// Paint the whole mixer once and return every string it wrote.
    fn painted_mixer(document: &Document) -> Vec<String> {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let peaks = MixPeaks::default();
        let mut levels = MixerMeterLevels::default();
        let mut edits = InspectorEdits::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            mixer_strips(ui, document, &peaks, false, &mut levels, &mut edits);
        });
        assert!(
            edits.operations().is_empty(),
            "painting the mixer writes no operation"
        );
        theme::painted_text(&output)
    }

    /// A silenced track states one cause, the most specific one it has.
    #[test]
    fn a_track_without_audio_says_so_instead_of_muted_by_solo() {
        let mut document = mixer_document();
        // Track 1 carries the only audio clip; soloing it silences track 2,
        // which has no audio-bearing clip of its own.
        document.audio_mix.tracks = vec![TrackMix {
            solo: true,
            ..TrackMix::neutral(TrackId(1))
        }];
        let painted = painted_mixer(&document);
        assert!(painted.iter().any(|text| text == "NO AUDIO"));
        assert!(
            !painted.iter().any(|text| text == SOLO_MUTED_LABEL),
            "a track with nothing to hear does not blame another track's solo: {painted:?}"
        );

        // Solo the empty track instead and the one carrying audio says why it
        // went quiet.
        document.audio_mix.tracks = vec![TrackMix {
            solo: true,
            ..TrackMix::neutral(TrackId(2))
        }];
        let painted = painted_mixer(&document);
        assert!(
            painted.iter().any(|text| text == SOLO_MUTED_LABEL),
            "a track silenced by another track's solo says so: {painted:?}"
        );
    }

    /// Lay one caps label out with no wrap and return its width.
    fn caps_label_width(ctx: &egui::Context, text: &str) -> f32 {
        let mut job = theme::caps_label(text, color::TEXT_MUTED);
        job.wrap.max_width = f32::INFINITY;
        ctx.fonts_mut(|fonts| fonts.layout_job(job)).size().x
    }

    /// DESIGN.md's caps rule: a machine-state label may be uppercase only
    /// while it is twelve characters or fewer. `MUTED BY SOLO` was thirteen.
    ///
    /// The measurement is recorded here because it sets the strip's height
    /// budget: at the house tracking the label must fit one 72 px line.
    #[test]
    fn the_solo_muted_label_is_within_the_caps_cap_and_narrower_than_the_old_one() {
        assert!(
            SOLO_MUTED_LABEL.chars().count() <= 12,
            "a caps label is at most twelve characters: `{SOLO_MUTED_LABEL}`"
        );
        let ctx = egui::Context::default();
        theme::install(&ctx);
        // The theme's fonts are installed on the first pass, so lay one frame
        // out before measuring with them.
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("");
        });
        let width = caps_label_width(&ctx, SOLO_MUTED_LABEL);
        assert!(
            width < caps_label_width(&ctx, "MUTED BY SOLO"),
            "the replacement is narrower than the label it replaces: {width} px"
        );
        assert!(
            width <= size::MIXER_STRIP_WIDTH,
            "the label must fit one strip line: {width} px"
        );

        let mut job = theme::caps_label(SOLO_MUTED_LABEL, color::TEXT_MUTED);
        job.wrap.max_width = size::MIXER_STRIP_WIDTH;
        let wrapped = ctx.fonts_mut(|fonts| fonts.layout_job(job));
        assert!(
            wrapped.rows.len() == 1,
            "the label takes exactly one line in the strip"
        );
        assert!(
            wrapped.size().x <= size::MIXER_STRIP_WIDTH,
            "wrapped, the label stays inside the column: {} px",
            wrapped.size().x
        );
    }

    /// AU1 §7 item 24: painting the mixer says what the mix is and writes
    /// nothing to the document.
    #[test]
    fn painting_the_mixer_writes_nothing_and_names_every_strip() {
        let ctx = egui::Context::default();
        // The strips ask for the app's own font families, so the theme has to
        // be installed before one can be laid out.
        theme::install(&ctx);
        let document = mixer_document();
        let peaks = MixPeaks::default();
        let mut levels = MixerMeterLevels::default();
        let mut edits = InspectorEdits::default();

        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            mixer_strips(ui, &document, &peaks, false, &mut levels, &mut edits);
        });

        assert!(
            edits.operations().is_empty(),
            "painting the mixer writes no operation"
        );
        let painted = theme::painted_text(&output);
        for expected in ["V1", "A2", "MASTER", "M", "S", "NO AUDIO", "Dialogue"] {
            assert!(
                painted.iter().any(|text| text == expected),
                "the mixer paints {expected}; it painted {painted:?}"
            );
        }
        assert!(
            painted.iter().any(|text| text.contains("tracks=A2")),
            "the bus names its members the way the timeline names them: {painted:?}"
        );
        assert!(
            painted
                .iter()
                .any(|text| text.contains("Bus controls arrive with AU2")),
            "the bus strip states its AU1 limit: {painted:?}"
        );
    }

    /// A headless mixer driven by real pointer events (AU1 §7 item 22).
    ///
    /// Painting with `RawInput::default()` proves only the no-op case; the
    /// gestures this panel owes the contract — a coalesced fader drag, a
    /// discrete toggle click, a discrete `Reset` — only exist once a pointer
    /// presses a button somewhere. egui interacts against the *previous*
    /// frame's rectangles, so every test lays the strip out once before it
    /// presses anything, and reads the coordinates the strip recorded rather
    /// than guessing them.
    struct MixerHarness {
        ctx: egui::Context,
        document: Document,
        levels: MixerMeterLevels,
        time: f64,
        rects: Vec<(&'static str, egui::Rect)>,
    }

    /// 20 ms a frame: long enough that a press and the release after it are
    /// separate frames (egui does not call a same-frame press/release a drag),
    /// short enough to stay inside the click timeout.
    const FRAME_SECONDS: f64 = 0.02;

    impl MixerHarness {
        fn new(document: Document) -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            Self {
                ctx,
                document,
                levels: MixerMeterLevels::default(),
                time: 0.0,
                rects: Vec::new(),
            }
        }

        /// Run one frame with these events and return the edits it wrote.
        ///
        /// The frame's operations are applied before the next one paints, as
        /// the core actor does in the app: a control that could not see its
        /// own edit land would write it again every frame.
        fn frame(&mut self, events: Vec<egui::Event>) -> InspectorEdits {
            self.time += FRAME_SECONDS;
            #[allow(clippy::cast_possible_truncation)]
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                time: Some(self.time),
                predicted_dt: FRAME_SECONDS as f32,
                events,
                ..Default::default()
            };
            let peaks = MixPeaks::default();
            let mut edits = InspectorEdits::default();
            let levels = &mut self.levels;
            let document = &self.document;
            let _ = self.ctx.run_ui(input, |ui| {
                // A discarded pass runs the closure again, so the record is
                // cleared here rather than between frames.
                STRIP_RECTS.with(|rects| rects.borrow_mut().clear());
                mixer_strips(ui, document, &peaks, false, levels, &mut edits);
            });
            self.rects = STRIP_RECTS.with(|rects| rects.borrow().clone());
            for operation in edits.operations() {
                operation
                    .apply(&mut self.document)
                    .expect("the mixer writes operations the document accepts");
            }
            edits
        }

        /// The first strip's control of this name — the first strip is the
        /// first track in document order.
        fn rect(&self, name: &str) -> egui::Rect {
            self.rects
                .iter()
                .find(|(candidate, _)| *candidate == name)
                .unwrap_or_else(|| {
                    panic!(
                        "the strip lays out a `{name}`; it laid out {:?}",
                        self.rects
                    )
                })
                .1
        }

        fn has(&self, name: &str) -> bool {
            self.rects.iter().any(|(candidate, _)| *candidate == name)
        }
    }

    fn moved(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerMoved(pos)
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// A point on the fader's rail, `fraction` of the way down it.
    ///
    /// The fader's response is the union of the vertical rail and the numeric
    /// readout under it, so the rail is the top `MIXER_FADER_HEIGHT` of it and
    /// the readout is the rest.
    fn rail_point(fader: egui::Rect, fraction: f32) -> egui::Pos2 {
        egui::pos2(
            fader.left() + 2.0,
            fader.top() + fraction * size::MIXER_FADER_HEIGHT,
        )
    }

    /// A point on the fader's numeric readout, which sits under the rail.
    fn readout_point(fader: egui::Rect) -> egui::Pos2 {
        egui::pos2(fader.center().x, fader.bottom() - 4.0)
    }

    fn gain_of(operation: &Operation) -> i32 {
        match operation {
            Operation::SetTrackMix { gain_tenth_db, .. } => *gain_tenth_db,
            other => panic!("the mixer writes only SetTrackMix; it wrote {other:?}"),
        }
    }

    /// One non-neutral track 1, so the strip shows its `Reset` row.
    fn non_neutral_document() -> Document {
        let mut document = mixer_document();
        document.audio_mix.tracks = vec![TrackMix {
            gain_tenth_db: -60,
            pan_percent: 25,
            ..TrackMix::neutral(TrackId(1))
        }];
        document
    }

    /// AU1 §5.1 and §7 item 22: a press, a move, and a release on the fader
    /// rail write one `SetTrackMix` per frame under the track's coalesce key,
    /// and open a gesture on the press frame.
    #[test]
    fn a_fader_drag_writes_coalesced_track_mix_operations() {
        let mut harness = MixerHarness::new(mixer_document());
        let laid_out = harness.frame(Vec::new());
        assert!(
            laid_out.operations().is_empty(),
            "laying the strip out writes nothing"
        );
        let fader = harness.rect("fader");
        assert!(
            fader.height() > size::MIXER_FADER_HEIGHT,
            "the fader is a {} px rail with its readout under it; it measured {} px",
            size::MIXER_FADER_HEIGHT,
            fader.height()
        );

        let press = rail_point(fader, 0.9);
        let pressed = harness.frame(vec![moved(press), pointer(press, true)]);
        assert!(
            pressed.gesture_started(),
            "the press opens a fresh undo gesture"
        );
        assert_eq!(
            pressed.coalesce_key(),
            Some("track_mix:1"),
            "a live fader frame carries the track's key"
        );
        let first = gain_of(&pressed.operations()[0]);
        assert!(
            first < -100,
            "pressing low on the rail sets a low gain; it set {first}"
        );

        let moved_to = rail_point(fader, 0.5);
        let dragged = harness.frame(vec![moved(moved_to)]);
        assert_eq!(
            dragged.coalesce_key(),
            Some("track_mix:1"),
            "every frame of the drag joins the same undo entry"
        );
        assert!(
            !dragged.gesture_started(),
            "the gesture opens once, on the press"
        );
        let second = gain_of(&dragged.operations()[0]);
        assert!(
            second > first,
            "dragging up raises the gain: {first} then {second}"
        );

        let released = harness.frame(vec![pointer(moved_to, false)]);
        for operation in released.operations() {
            assert!(
                matches!(operation, Operation::SetTrackMix { track, .. } if *track == TrackId(1)),
                "the release writes the same track's mix; it wrote {operation:?}"
            );
        }
    }

    /// AU1 §5.1: this is the blocker the first build shipped. The rail senses
    /// drags only, so egui never reports a click on it; the double-click reset
    /// it carried could not fire, and a second press simply wrote whatever
    /// value the pointer landed on. A click on the rail must never read as
    /// "back to neutral".
    #[test]
    fn a_click_on_the_fader_rail_never_resets_the_track() {
        let mut harness = MixerHarness::new(non_neutral_document());
        let _ = harness.frame(Vec::new());
        let fader = harness.rect("fader");
        let point = rail_point(fader, 0.9);

        let mut written = Vec::new();
        for events in [
            vec![moved(point), pointer(point, true)],
            vec![pointer(point, false)],
            // A second press in the same place is what a double click is.
            vec![pointer(point, true)],
            vec![pointer(point, false)],
        ] {
            written.extend(harness.frame(events).operations().to_vec());
        }

        assert!(
            !written.is_empty(),
            "a press on the rail moves the fader, which is all a rail can do"
        );
        assert!(
            !written.contains(&track_mix_operation(TrackMix::neutral(TrackId(1)))),
            "clicking the rail must not write a neutral mix: {written:?}"
        );
        for operation in &written {
            assert!(
                matches!(
                    operation,
                    Operation::SetTrackMix {
                        track,
                        pan_percent: 25,
                        ..
                    } if *track == TrackId(1)
                ),
                "the fader carries the track's pan; it wrote {operation:?}"
            );
        }
    }

    /// AU1 §5.1: the `M` toggle is one discrete undo entry with one flag
    /// flipped and nothing coalesced.
    #[test]
    fn clicking_mute_writes_exactly_one_discrete_operation() {
        let mut harness = MixerHarness::new(mixer_document());
        let _ = harness.frame(Vec::new());
        let mute = harness.rect("M");

        let pressed = harness.frame(vec![moved(mute.center()), pointer(mute.center(), true)]);
        assert!(
            pressed.operations().is_empty(),
            "the click lands on the release, not the press"
        );
        let clicked = harness.frame(vec![pointer(mute.center(), false)]);
        assert_eq!(
            clicked.operations(),
            [Operation::SetTrackMix {
                track: TrackId(1),
                gain_tenth_db: 0,
                pan_percent: 0,
                mute: true,
                solo: false,
            }],
            "one click flips mute and carries the other three values"
        );
        assert_eq!(
            clicked.coalesce_key(),
            None,
            "a toggle is discrete: it never joins a drag's undo entry"
        );
    }

    /// AU1 §5.1 as amended: `Reset` appears only when there is something to
    /// reset, and one click on it writes one discrete neutral operation.
    #[test]
    fn the_reset_row_appears_only_off_neutral_and_writes_one_neutral_operation() {
        let mut harness = MixerHarness::new(mixer_document());
        let _ = harness.frame(Vec::new());
        assert!(
            !harness.has("reset"),
            "a neutral strip has nothing to reset: {:?}",
            harness.rects
        );

        let mut harness = MixerHarness::new(non_neutral_document());
        let _ = harness.frame(Vec::new());
        let reset = harness.rect("reset");

        let pressed = harness.frame(vec![moved(reset.center()), pointer(reset.center(), true)]);
        assert!(pressed.operations().is_empty());
        let clicked = harness.frame(vec![pointer(reset.center(), false)]);
        assert_eq!(
            clicked.operations(),
            [track_mix_operation(TrackMix::neutral(TrackId(1)))],
            "Reset clears all four values in one operation"
        );
        assert_eq!(
            clicked.coalesce_key(),
            None,
            "Reset is discrete, not the tail of a drag"
        );
        assert!(
            !clicked.gesture_started(),
            "a button press opens no drag gesture"
        );
    }

    /// AU1 §5.1: a value typed into the fader's readout is one discrete
    /// operation, not a coalesced drag frame and not a double write.
    #[test]
    fn a_value_typed_into_the_readout_is_one_discrete_operation() {
        let mut harness = MixerHarness::new(mixer_document());
        let _ = harness.frame(Vec::new());
        let readout = readout_point(harness.rect("fader"));

        // A click on the readout opens the keyboard editor.
        let _ = harness.frame(vec![moved(readout), pointer(readout, true)]);
        let _ = harness.frame(vec![pointer(readout, false)]);
        let mut written = Vec::new();
        for events in [
            vec![egui::Event::Text("-6.0".to_owned())],
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            Vec::new(),
        ] {
            let edits = harness.frame(events);
            assert_eq!(
                edits.coalesce_key(),
                None,
                "a typed value is discrete: {:?}",
                edits.operations()
            );
            written.extend(edits.operations().to_vec());
        }
        assert_eq!(
            written,
            [Operation::SetTrackMix {
                track: TrackId(1),
                gain_tenth_db: -60,
                pan_percent: 0,
                mute: false,
                solo: false,
            }],
            "typing -6.0 dB writes exactly one operation"
        );
    }
}

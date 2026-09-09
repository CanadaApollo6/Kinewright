//! The mixer: one strip per track, one per bus, one for master, and the AU2
//! chain pane beside them.
//!
//! The panel is the human half of AU1 §5.1 and AU2 §6.5-§6.8. It reads the
//! document's `audio_mix` and the engine's `mix_peaks()` and writes
//! [`Operation::SetTrackMix`], [`Operation::UpsertAudioBus`],
//! [`Operation::SetAudioMaster`], and [`Operation::SetPanLaw`].
//!
//! Bus and master controls never push an operation of their own. They mutate
//! an edited copy of their chain in [`MixerChainEdits`], and [`mixer_body`]
//! folds every touched chain into exactly one operation each after the strips
//! and the pane have painted (AU2 §6.6). Two controls of one chain moving in
//! the same frame therefore produce one whole-chain set carrying both, not two
//! that clobber each other.
//!
//! Everything that paints is a free function taking `&mut egui::Ui`, so the
//! headless `ctx.run_ui` test pattern can assert what a strip says and that a
//! frame of painting writes nothing.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use eframe::egui;
use kinewright_core::{
    AUDIO_BUS_GAIN_MAX, AUDIO_BUS_GAIN_MIN, AUDIO_MASTER_GAIN_MAX, AUDIO_MASTER_GAIN_MIN, AudioBus,
    AudioBusId, AudioChain, AudioMaster, Document, EffectId, LoudnessSnapshot, LoudnessTarget,
    MixPeaks, Operation, PanLaw, Playback, TRACK_MIX_GAIN_MAX, TRACK_MIX_GAIN_MIN,
    TRACK_MIX_PAN_MAX, TRACK_MIX_PAN_MIN, Track, TrackId, TrackKind, TrackMix,
};

use crate::{
    app::KinewrightApp,
    export_ui::export_delivery_profile,
    icons::Icon,
    inspector_ui::{InspectorEdits, clip_carries_audio, effect_display_name, is_live_drag},
    mixer_pane_ui,
    theme::{self, color, radius, size, space, type_size},
    // The thresholds and the decay rate are the transport meter's, imported
    // rather than restated so the two meters cannot drift (AU1 §5.1).
    transport::{DANGER_START, DECAY_PER_SECOND, WARNING_START, peak_to_meter_level},
};

/// Full-scale of a gain-reduction bar, in decibels (AU2 §6.7).
///
/// A dynamics node that has pushed a signal 24 dB down has done everything a
/// meter can usefully say; past that the bar is simply full.
pub(crate) const MIXER_REDUCTION_METER_RANGE_DB: f32 = 24.0;

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
    static STRIP_RECTS: std::cell::RefCell<Vec<(String, egui::Rect)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn record_strip_rect(name: &'static str, rect: egui::Rect) {
    STRIP_RECTS.with(|rects| rects.borrow_mut().push((name.to_owned(), rect)));
}

#[cfg(not(test))]
#[allow(clippy::inline_always)]
#[inline(always)]
pub(crate) const fn record_strip_rect(_name: &'static str, _rect: egui::Rect) {}

/// Record one control whose name carries an id, such as `bus_fader:1`.
///
/// The pieces are passed rather than a formatted name so a release build never
/// allocates a string it immediately discards.
#[cfg(test)]
pub(crate) fn record_keyed_rect(kind: &str, id: u64, rect: egui::Rect) {
    STRIP_RECTS.with(|rects| rects.borrow_mut().push((format!("{kind}:{id}"), rect)));
}

#[cfg(not(test))]
#[allow(clippy::inline_always)]
#[inline(always)]
pub(crate) const fn record_keyed_rect(_kind: &str, _id: u64, _rect: egui::Rect) {}

/// Record one chain-card control, named `{kind}:{effect}:{parameter}`.
#[cfg(test)]
pub(crate) fn record_param_rect(kind: &str, effect: EffectId, name: &str, rect: egui::Rect) {
    STRIP_RECTS.with(|rects| {
        rects
            .borrow_mut()
            .push((format!("{kind}:{}:{name}", effect.0), rect));
    });
}

#[cfg(not(test))]
#[allow(clippy::inline_always)]
#[inline(always)]
pub(crate) const fn record_param_rect(
    _kind: &str,
    _effect: EffectId,
    _name: &str,
    _rect: egui::Rect,
) {
}

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
    /// AU2 §6.7: one displayed gain reduction per node with a gain computer,
    /// held as the fraction of [`MIXER_REDUCTION_METER_RANGE_DB`] the bar
    /// fills so it decays on exactly the same schedule as every other meter
    /// here rather than inventing a second ballistics rule in decibels.
    reductions: HashMap<(AudioChain, EffectId), f32>,
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
            .chain(self.reductions.values_mut())
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
            for (chain, effect, reduction) in &peaks.gain_reduction {
                let entry = self.reductions.entry((*chain, *effect)).or_default();
                *entry = entry.max((reduction / MIXER_REDUCTION_METER_RANGE_DB).clamp(0.0, 1.0));
            }
        }
        // A silent entry carries no information, and a removed track must not
        // leave one behind; the lookups below read silence for a missing key.
        self.tracks
            .retain(|_, level| level.iter().any(|l| *l > 0.0));
        self.buses.retain(|_, level| level.iter().any(|l| *l > 0.0));
        self.reductions.retain(|_, level| *level > 0.0);
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
            .chain(self.reductions.values())
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

    /// The displayed gain reduction of one node, in decibels (AU2 §6.7).
    ///
    /// An absent key reads zero: a node with no gain computer, a chain that is
    /// not playing, and a node the processor has not reported yet all mean the
    /// same thing to a bar.
    pub(crate) fn reduction(&self, chain: AudioChain, effect: EffectId) -> f32 {
        self.reductions
            .get(&(chain, effect))
            .copied()
            .unwrap_or_default()
            * MIXER_REDUCTION_METER_RANGE_DB
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

/// Which chain the mixer's pane is editing (AU2 §6.6).
///
/// UI state, not document state: nothing here is written to a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MixerSelection {
    Bus(AudioBusId),
    Master,
}

impl MixerSelection {
    /// The chain key the engine's telemetry uses for this selection.
    pub(crate) const fn chain(self) -> AudioChain {
        match self {
            Self::Bus(bus) => AudioChain::Bus(bus),
            Self::Master => AudioChain::Master,
        }
    }

    /// The coalesce key of one live gesture on this chain (AU2 §6.6).
    ///
    /// One key per chain, exactly as `track_mix_coalesce_key` is one key per
    /// track: `UpsertAudioBus` and `SetAudioMaster` are whole-target
    /// idempotent sets, so two controls of one chain moving in one frame have
    /// only one key to choose and the fold's choice is unambiguous.
    pub(crate) fn coalesce_key(self) -> String {
        match self {
            Self::Bus(bus) => audio_bus_coalesce_key(bus),
            Self::Master => AUDIO_MASTER_COALESCE_KEY.to_owned(),
        }
    }
}

/// Stable coalesce key for one live bus gesture (AU2 §6.6).
pub(crate) fn audio_bus_coalesce_key(bus: AudioBusId) -> String {
    format!("audio_bus:{}", bus.0)
}

/// Stable coalesce key for one live master gesture (AU2 §6.6).
pub(crate) const AUDIO_MASTER_COALESCE_KEY: &str = "audio_master";

/// One edited copy per chain the frame touched, folded into one operation each
/// at the end of the frame (AU2 §6.6).
///
/// Controls **never** push an operation. `UpsertAudioBus` and `SetAudioMaster`
/// are whole-chain sets, so a fader that pushed its own value and a card
/// parameter that pushed its own in the same frame would each carry the other
/// control's *old* value and the second would clobber the first. They mutate a
/// copy instead and the fold writes one set carrying both.
#[derive(Debug, Default)]
pub(crate) struct MixerChainEdits {
    buses: BTreeMap<AudioBusId, AudioBus>,
    master: Option<AudioMaster>,
    /// The pan law is document-level rather than part of any chain, but its
    /// control lives on the master pane and obeys the same "never push" rule.
    pan_law: Option<PanLaw>,
    /// True when any edit this frame came from a live drag.
    live: bool,
    /// True when any control opened a drag gesture this frame.
    gesture_started: bool,
}

impl MixerChainEdits {
    /// The edited copy of one bus, cloned in on first touch.
    pub(crate) fn bus(&mut self, bus: &AudioBus) -> &mut AudioBus {
        self.buses.entry(bus.id).or_insert_with(|| bus.clone())
    }

    /// The edited copy of the master chain, cloned in on first touch.
    pub(crate) fn master(&mut self, master: &AudioMaster) -> &mut AudioMaster {
        self.master.get_or_insert_with(|| master.clone())
    }

    /// Add a bus the document does not have yet (`+ Bus`, AU2 §6.5).
    fn create_bus(&mut self, bus: AudioBus) {
        self.buses.insert(bus.id, bus);
    }

    pub(crate) const fn set_pan_law(&mut self, law: PanLaw) {
        self.pan_law = Some(law);
    }

    pub(crate) const fn mark_live(&mut self, live: bool) {
        if live {
            self.live = true;
        }
    }

    pub(crate) const fn begin_gesture(&mut self) {
        self.gesture_started = true;
    }

    /// Fold every touched chain into one operation each (AU2 §6.6).
    ///
    /// Runs after both the strips and the pane have painted, because
    /// `InspectorEdits::push_live` sets the coalesce key only on the first
    /// operation of the frame and `submit_inspector_edits` reads exactly one
    /// key per frame. A copy equal to what the document already holds is
    /// dropped: a control that reports `changed()` with the value it was given
    /// is not an edit.
    fn drain_into(self, document: &Document, edits: &mut InspectorEdits) {
        if self.gesture_started {
            edits.begin_gesture();
        }
        let live = self.live;
        for (id, bus) in self.buses {
            if document.audio_mix.bus(id) == Some(&bus) {
                continue;
            }
            file_chain_edit(
                edits,
                live,
                Operation::UpsertAudioBus { bus },
                MixerSelection::Bus(id).coalesce_key(),
            );
        }
        if let Some(master) = self.master
            && master != document.audio_mix.master
        {
            file_chain_edit(
                edits,
                live,
                Operation::SetAudioMaster { master },
                MixerSelection::Master.coalesce_key(),
            );
        }
        if let Some(law) = self.pan_law
            && law != document.audio_mix.pan_law
        {
            // The law is chosen with a click, never a drag, so this is always
            // the discrete branch in practice; it follows the frame's rule
            // rather than asserting that, so it can never drop a live key.
            file_chain_edit(
                edits,
                live,
                Operation::SetPanLaw { law },
                MixerSelection::Master.coalesce_key(),
            );
        }
    }
}

fn file_chain_edit(
    edits: &mut InspectorEdits,
    live: bool,
    operation: Operation,
    coalesce_key: String,
) {
    if live {
        edits.push_live(operation, coalesce_key);
    } else {
        edits.push(operation);
    }
}

/// What the Mixer reads from the engine each frame it is visible.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MixerTelemetry {
    /// Per-point peaks; silence while the transport is stopped.
    pub(crate) peaks: MixPeaks,
    /// The live loudness meter (AU3 §3.9), read playing or paused.
    pub(crate) loudness: LoudnessSnapshot,
}

/// Read the Mixer's telemetry for one frame (AU1 §5.1, AU3 §4.4).
///
/// Peaks are telemetry from a running engine: a stopped transport has no
/// signal to report, so the meters read what is true, silence. The loudness
/// snapshot is different in kind — it describes what has been heard since the
/// last reset, and the meter freezes it on pause (AU3 §3.9, F15) — so it is
/// read every frame, playing or paused, and the frozen figures stay on
/// screen.
pub(crate) fn mixer_telemetry(playback: &dyn Playback, playing: bool) -> MixerTelemetry {
    MixerTelemetry {
        peaks: if playing {
            playback.mix_peaks()
        } else {
            MixPeaks::default()
        },
        loudness: playback.loudness(),
    }
}

/// What one frame of [`mixer_body`] hands back to its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MixerFrame {
    /// The selection the frame ends with; see [`mixer_body`].
    pub(crate) selection: Option<MixerSelection>,
    /// The `LOUDNESS` section's `Reset` was clicked (AU3 §4.4). Answered with
    /// `Playback::reset_loudness`, never with an operation.
    pub(crate) reset_loudness: bool,
}

impl KinewrightApp {
    /// The Mixer tab of the material strip (AU1 §5.1, AU2 §6.6, AU3 §4.4).
    pub(crate) fn mixer_panel(&mut self, ui: &mut egui::Ui) {
        let document = Arc::clone(&self.focused().document);
        let telemetry = mixer_telemetry(self.playback.as_ref(), self.playing);
        // The bars measure against the export dialog's current profile target,
        // whatever the dialog's other settings say (AU3 §4.4, F12/F20).
        let target = export_delivery_profile(self.export_dialog.delivery_aspect).loudness_target();
        let mut edits = InspectorEdits::default();
        let frame = mixer_body(
            ui,
            &document,
            self.mixer_selection,
            &telemetry,
            target,
            self.playing,
            &mut self.mixer_levels,
            &mut edits,
        );
        self.mixer_selection = frame.selection;
        if frame.reset_loudness {
            self.playback.reset_loudness();
        }
        self.submit_inspector_edits(edits);
    }
}

/// The whole Mixer tab: the strips and, when one chain is selected, the pane
/// beside them (AU2 §6.6).
///
/// A free function rather than a method so the headless harness can drive both
/// surfaces: the harness builds an `egui::Context`, a `Document`, and
/// `MixerMeterLevels`, and cannot construct a `KinewrightApp`.
///
/// Returns the selection the frame ends with — an `Edit` toggle is a control
/// like any other and has to be able to change it, and a selection naming a
/// bus the document no longer has is cleared here rather than left to paint
/// an empty pane — and whether the loudness `Reset` was clicked (AU3 §4.4).
#[allow(clippy::too_many_arguments)]
pub(crate) fn mixer_body(
    ui: &mut egui::Ui,
    document: &Document,
    selection: Option<MixerSelection>,
    telemetry: &MixerTelemetry,
    target: LoudnessTarget,
    playing: bool,
    levels: &mut MixerMeterLevels,
    edits: &mut InspectorEdits,
) -> MixerFrame {
    levels.advance(ui, &telemetry.peaks, playing);
    let selection = match selection {
        Some(MixerSelection::Bus(bus)) if document.audio_mix.bus(bus).is_none() => None,
        other => other,
    };
    let mut requested = selection;
    let mut reset_loudness = false;
    let mut chain = MixerChainEdits::default();
    ui.horizontal_top(|ui| {
        // The pane owns a fixed column on the right; the strips take what is
        // left and scroll horizontally inside it, as they do with no pane.
        let reserved = if selection.is_some() {
            size::MIXER_CHAIN_PANE_WIDTH + space::THREE
        } else {
            0.0
        };
        let strips_width = (ui.available_width() - reserved).max(size::MIXER_STRIP_WIDTH);
        let strips = egui::vec2(strips_width, ui.available_height());
        ui.allocate_ui_with_layout(strips, egui::Layout::top_down(egui::Align::Min), |ui| {
            mixer_strips(
                ui,
                document,
                selection,
                levels,
                telemetry.loudness,
                &mut requested,
                &mut chain,
                edits,
            );
        });
        if let Some(current) = selection {
            ui.separator();
            reset_loudness = mixer_pane_ui::chain_pane(
                ui,
                document,
                current,
                levels,
                telemetry.loudness,
                target,
                &mut chain,
            );
        }
    });
    chain.drain_into(document, edits);
    MixerFrame {
        selection: requested,
        reset_loudness,
    }
}

/// Paint the strips: tracks, then buses, then master (AU1 §5.1).
#[allow(clippy::too_many_arguments)]
pub(crate) fn mixer_strips(
    ui: &mut egui::Ui,
    document: &Document,
    selection: Option<MixerSelection>,
    levels: &MixerMeterLevels,
    loudness: LoudnessSnapshot,
    requested: &mut Option<MixerSelection>,
    chain: &mut MixerChainEdits,
    edits: &mut InspectorEdits,
) {
    // Horizontal for the strips themselves; vertical as a fallback, so a dock
    // dragged shorter than a strip still reaches every control.
    egui::ScrollArea::both()
        .id_salt("mixer-strips")
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for (index, track) in document.tracks.iter().enumerate() {
                    track_strip(ui, document, track, index, levels, chain, edits);
                }
                if !document.audio_mix.buses.is_empty() {
                    // One rule per boundary: with no buses the tracks and the
                    // master meet at a single separator, not two 12 px apart.
                    ui.separator();
                    for bus in &document.audio_mix.buses {
                        bus_strip(ui, document, bus, selection, levels, requested, chain);
                    }
                }
                ui.separator();
                master_strip(ui, document, selection, levels, loudness, requested, chain);
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
    chain: &mut MixerChainEdits,
    edits: &mut InspectorEdits,
) {
    let mix = document.track_mix(track.id);
    let carries_audio = track_carries_audio(document, track);
    strip(ui, |ui| {
        caption_row(ui, track.kind, index, carries_audio);

        let (fader, gain_tenth_db) = meters_and_fader(
            ui,
            levels.track(track.id),
            mix.gain_tenth_db,
            TRACK_MIX_GAIN_MIN..=TRACK_MIX_GAIN_MAX,
        );
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

        // `Reset` and `+ Bus` share a row. Both are conditional in different
        // ways — `Reset` appears only off neutral, `+ Bus` is disabled rather
        // than hidden — and giving each its own row put the tallest track
        // strip 16 px over the 240 px dock budget (AU2 §6.5).
        ui.scope(|ui| {
            // Two small buttons, not a section: the row does not owe them a
            // control's height, and a 26 px row put the `NO AUDIO` strip over
            // the 240 px dock budget. Both are set in micro with the strip's
            // own padding so the pair fits one 72 px line rather than
            // wrapping to two (AU2 §6.5).
            ui.spacing_mut().interact_size.y = size::ICON_SM;
            ui.spacing_mut().button_padding = egui::vec2(space::HALF, 0.0);
            ui.spacing_mut().item_spacing.x = space::HALF;
            ui.style_mut().override_font_id = Some(theme::medium(type_size::MICRO));
            ui.horizontal(|ui| {
                reset_row(ui, mix, edits);
                add_bus_button(ui, document, track, index, carries_audio, chain);
            });
        });
    });
}

/// The `+ Bus` button (AU2 §6.5): create a bus around the strip you are
/// pointing at, which is what every other mixer control does.
fn add_bus_button(
    ui: &mut egui::Ui,
    document: &Document,
    track: &Track,
    index: usize,
    carries_audio: bool,
    chain: &mut MixerChainEdits,
) {
    let reason = add_bus_block(document, track, carries_audio);
    let response = ui.add_enabled(reason.is_none(), egui::Button::new("+ Bus").small());
    record_keyed_rect("add_bus", track.id.0, response.rect);
    let response = match reason {
        Some(reason) => response.on_disabled_hover_text(reason),
        None => response.on_hover_text("Create a bus carrying this track."),
    };
    if response.clicked() {
        chain.create_bus(AudioBus {
            id: document.audio_mix.next_bus_id(),
            name: track_caption_and_icon(track.kind, index).0,
            tracks: vec![track.id],
            gain_tenth_db: 0,
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
        });
    }
}

/// Why `+ Bus` is refused on this track, if it is (AU2 §6.5).
///
/// "No audio" is the more specific of the two, so a silent track that is
/// somehow already routed says the thing that is actually stopping it.
pub(crate) fn add_bus_block(
    document: &Document,
    track: &Track,
    carries_audio: bool,
) -> Option<&'static str> {
    if !carries_audio {
        return Some(NO_AUDIO_REASON);
    }
    document
        .audio_mix
        .bus_for_track(track.id)
        .map(|_| ALREADY_ROUTED_REASON)
}

/// Why `+ Bus` is disabled on a track with no audio-bearing clip (AU2 §6.5).
pub(crate) const NO_AUDIO_REASON: &str = "no audio";
/// Why `+ Bus` is disabled on a track that already routes to a bus.
pub(crate) const ALREADY_ROUTED_REASON: &str = "already routed";

/// One bus strip: name, members, node count, meter-and-fader row, `Edit`.
fn bus_strip(
    ui: &mut egui::Ui,
    document: &Document,
    bus: &AudioBus,
    selection: Option<MixerSelection>,
    levels: &MixerMeterLevels,
    requested: &mut Option<MixerSelection>,
    chain: &mut MixerChainEdits,
) {
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
        // A `→`-joined chain wraps to roughly eight lines at 13 pt in a 72 px
        // column, so the strip states the count and the tooltip states the
        // chain; the sidechain line moved into the pane (AU2 §6.5).
        ui.colored_label(color::TEXT_MUTED, node_count_label(bus.effects.len()))
            .on_hover_text(chain_tooltip(&bus.effects));

        let (fader, gain_tenth_db) = meters_and_fader(
            ui,
            levels.bus(bus.id),
            bus.gain_tenth_db,
            AUDIO_BUS_GAIN_MIN..=AUDIO_BUS_GAIN_MAX,
        );
        record_keyed_rect("bus_fader", bus.id.0, fader.rect);
        if fader.drag_started() {
            chain.begin_gesture();
        }
        if fader.changed() && gain_tenth_db != bus.gain_tenth_db {
            chain.bus(bus).gain_tenth_db = gain_tenth_db;
            chain.mark_live(is_live_drag(&fader));
        }

        let selected = selection == Some(MixerSelection::Bus(bus.id));
        let toggle = edit_toggle(ui, selected, MixerSelection::Bus(bus.id), requested);
        record_keyed_rect("bus_edit", bus.id.0, toggle);
    });
}

/// The master strip: the post-limiter meter, the master fader, the integrated
/// loudness line (AU3 §4.5), and `Edit`.
fn master_strip(
    ui: &mut egui::Ui,
    document: &Document,
    selection: Option<MixerSelection>,
    levels: &MixerMeterLevels,
    loudness: LoudnessSnapshot,
    requested: &mut Option<MixerSelection>,
    chain: &mut MixerChainEdits,
) {
    let master = &document.audio_mix.master;
    strip(ui, |ui| {
        ui.label(theme::caps_label("MASTER", color::TEXT_MUTED));
        ui.colored_label(color::TEXT_MUTED, node_count_label(master.effects.len()))
            .on_hover_text(chain_tooltip(&master.effects));

        let (fader, gain_tenth_db) = meters_and_fader(
            ui,
            levels.master(),
            master.gain_tenth_db,
            AUDIO_MASTER_GAIN_MIN..=AUDIO_MASTER_GAIN_MAX,
        );
        record_strip_rect("master_fader", fader.rect);
        if fader.drag_started() {
            chain.begin_gesture();
        }
        if fader.changed() && gain_tenth_db != master.gain_tenth_db {
            chain.master(master).gain_tenth_db = gain_tenth_db;
            chain.mark_live(is_live_drag(&fader));
        }

        // The integrated figure, heard so far; `I —` until the meter has a
        // complete gating block (AU3 §4.5). The strip is 72 px wide, so the
        // full readout line is the tooltip and the pane's section.
        ui.label(
            egui::RichText::new(mixer_pane_ui::integrated_strip_line(loudness))
                .font(theme::medium(type_size::MICRO))
                .color(color::TEXT_MUTED),
        )
        .on_hover_text(mixer_pane_ui::loudness_readout(loudness));

        let selected = selection == Some(MixerSelection::Master);
        let toggle = edit_toggle(ui, selected, MixerSelection::Master, requested);
        record_strip_rect("master_edit", toggle);
    });
}

/// The strip's `Edit` toggle, which opens or closes the chain pane (AU2 §6.5).
fn edit_toggle(
    ui: &mut egui::Ui,
    selected: bool,
    target: MixerSelection,
    requested: &mut Option<MixerSelection>,
) -> egui::Rect {
    let response = ui.selectable_label(selected, "Edit");
    if response.clicked() {
        *requested = if selected { None } else { Some(target) };
    }
    response.rect
}

/// `4 nodes`, `1 node`, `0 nodes`: one line, never wrapped (AU2 §6.5).
pub(crate) fn node_count_label(nodes: usize) -> String {
    if nodes == 1 {
        "1 node".to_owned()
    } else {
        format!("{nodes} nodes")
    }
}

/// The full chain, in order, as the node-count label's hover tooltip.
pub(crate) fn chain_tooltip(effects: &[kinewright_core::Effect]) -> String {
    if effects.is_empty() {
        return "No effects on this chain.".to_owned();
    }
    effects
        .iter()
        .map(|effect| effect_display_name(&effect.name))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// The micro caps label a track wears when some other track's solo is what
/// silenced it (AU1 §5.1).
///
/// Eight characters, inside DESIGN.md's twelve-character cap on an uppercase
/// machine-state label, and at the house tracking narrower than the 72 px
/// strip so it sits on one line; `MUTED BY SOLO` (thirteen characters,
/// ~97 px) and `SOLO MUTED` (~77 px) both wrapped.
const SOLO_MUTED_LABEL: &str = "SILENCED";

/// The strip's `Reset` button, shown only when there is something to reset.
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
///
/// AU2 §6.5 generalises the row off `TrackMix`: the bus and master strips
/// carry the same construction over their own gain range, and stacking their
/// meters above their fader would repeat AU1's 374 px mistake. Every one of
/// the three reads in decibels, so the formatter and parser are the row's own
/// rather than another argument that could only ever take one value.
fn meters_and_fader(
    ui: &mut egui::Ui,
    meters: [f32; 2],
    value: i32,
    range: std::ops::RangeInclusive<i32>,
) -> (egui::Response, i32) {
    let mut gain_tenth_db = value;
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
                        egui::Slider::new(&mut gain_tenth_db, range)
                            .vertical()
                            .integer()
                            .custom_formatter(|value, _| format_gain_db(value))
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

pub(crate) fn track_caption(document: &Document, track: TrackId) -> String {
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
pub(crate) fn track_carries_audio(document: &Document, track: &Track) -> bool {
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

/// Every fader in the mixer reads in decibels, whatever its own unit is.
pub(crate) fn format_gain_db(tenth_db: f64) -> String {
    format!("{:+.1} dB", tenth_db / 10.0)
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

    use std::sync::atomic::{AtomicUsize, Ordering};

    use kinewright_core::{
        AssetId, AudioMix, Clip, ClipContent, ClipId, DeliveryAspect, EBU_R128_PROGRAMME_TARGET,
        Effect, MediaAsset, MediaKind, ParamValue, Rational, STREAMING_PLATFORM_TARGET, TimeCode,
    };

    use super::*;
    use crate::mixer_pane_ui::{self, LOUDNESS_MONITORING_NOTE, MixerChain, MixerUnit};

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
                    gain_tenth_db: 0,
                    effects: vec![Effect {
                        id: kinewright_core::EffectId(1),
                        name: "audio_gain".to_owned(),
                        parameters: std::collections::BTreeMap::new(),
                        keyframes: std::collections::BTreeMap::new(),
                    }],
                    ducking_sidechain_tracks: vec![TrackId(1)],
                }],
                tracks: Vec::new(),
                ..AudioMix::default()
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
        let mut chain = MixerChainEdits::default();
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
                    &mut chain,
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
        painted_mixer_with(document, None)
    }

    /// Paint the mixer with a chain selected, so the pane paints too.
    fn painted_mixer_with(document: &Document, selection: Option<MixerSelection>) -> Vec<String> {
        painted_mixer_with_loudness(document, selection, LoudnessSnapshot::default())
    }

    fn painted_mixer_with_loudness(
        document: &Document,
        selection: Option<MixerSelection>,
        loudness: LoudnessSnapshot,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let telemetry = MixerTelemetry {
            peaks: MixPeaks::default(),
            loudness,
        };
        let mut levels = MixerMeterLevels::default();
        let mut edits = InspectorEdits::default();
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1_200.0, 800.0),
                )),
                ..Default::default()
            },
            |ui| {
                let frame = mixer_body(
                    ui,
                    document,
                    selection,
                    &telemetry,
                    STREAMING_PLATFORM_TARGET,
                    false,
                    &mut levels,
                    &mut edits,
                );
                assert!(!frame.reset_loudness, "nothing was clicked");
            },
        );
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

    /// AU1 §7 item 24 and AU2 §7 B19: painting the mixer says what the mix is
    /// and writes nothing to the document.
    #[test]
    fn painting_the_mixer_writes_nothing_and_names_every_strip() {
        let painted = painted_mixer(&mixer_document());
        for expected in [
            "V1", "A2", "MASTER", "M", "S", "NO AUDIO", "Dialogue", "+ Bus", "Edit", "1 node",
        ] {
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
            !painted
                .iter()
                .any(|text| text.contains("Bus controls arrive with AU2")),
            "AU2 delivers the controls the AU1 strip apologised for: {painted:?}"
        );
        assert!(
            !painted.iter().any(|text| text.contains("sidechain=")),
            "the sidechain moved into the chain pane (AU2 §6.5): {painted:?}"
        );
    }

    /// AU2 §7 B19: with a chain selected the pane paints beside the strips —
    /// its title, its routing, its cards, and its `+ Effect` menu — and still
    /// writes nothing.
    #[test]
    fn painting_the_chain_pane_writes_nothing_and_names_every_card() {
        let document = chain_document();
        let painted = painted_mixer_with(&document, Some(MixerSelection::Bus(AudioBusId(1))));
        for expected in [
            "Bus: Dialogue",
            "Tracks",
            "Sidechain",
            "+ Effect",
            "Gain",
            "Parametric EQ",
            "Compressor",
            "Gate",
            "Ducking",
            "True-peak limiter",
            "Bypass",
            "Remove",
            "6 nodes",
        ] {
            assert!(
                painted.iter().any(|text| text == expected),
                "the chain pane paints {expected}; it painted {painted:?}"
            );
        }

        let painted = painted_mixer_with(&document, Some(MixerSelection::Master));
        for expected in ["Master", "Pan law", "Balance", "Constant power"] {
            assert!(
                painted.iter().any(|text| text == expected),
                "the master pane paints {expected}; it painted {painted:?}"
            );
        }
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
        /// AU2 §6.6: the chain the pane is editing. `mixer_body` returns the
        /// selection the frame ended with, so the `Edit` toggles drive this
        /// exactly as they drive the app's own field.
        selection: Option<MixerSelection>,
        peaks: MixPeaks,
        /// AU3 §4.4: the snapshot the pane's `LOUDNESS` section reads.
        loudness: LoudnessSnapshot,
        playing: bool,
        levels: MixerMeterLevels,
        time: f64,
        rects: Vec<(String, egui::Rect)>,
        /// Whether the last frame's `Reset` was clicked (AU3 §4.4).
        reset_loudness: bool,
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
                selection: None,
                peaks: MixPeaks::default(),
                loudness: LoudnessSnapshot::default(),
                playing: false,
                levels: MixerMeterLevels::default(),
                time: 0.0,
                rects: Vec::new(),
                reset_loudness: false,
            }
        }

        /// Open the pane on one chain, as its `Edit` toggle would.
        fn editing(mut self, selection: MixerSelection) -> Self {
            self.selection = Some(selection);
            self
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
                // Wide enough that the strips and the 400 px chain pane both
                // sit inside the viewport: a control scrolled out of view
                // cannot be pressed.
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1_200.0, 800.0),
                )),
                time: Some(self.time),
                predicted_dt: FRAME_SECONDS as f32,
                events,
                ..Default::default()
            };
            let telemetry = MixerTelemetry {
                peaks: self.peaks.clone(),
                loudness: self.loudness,
            };
            let playing = self.playing;
            let selection = self.selection;
            let mut ended_with = MixerFrame {
                selection,
                reset_loudness: false,
            };
            let mut edits = InspectorEdits::default();
            let levels = &mut self.levels;
            let document = &self.document;
            let _ = self.ctx.run_ui(input, |ui| {
                // A discarded pass runs the closure again, so the record is
                // cleared here rather than between frames.
                STRIP_RECTS.with(|rects| rects.borrow_mut().clear());
                ended_with = mixer_body(
                    ui,
                    document,
                    selection,
                    &telemetry,
                    STREAMING_PLATFORM_TARGET,
                    playing,
                    levels,
                    &mut edits,
                );
            });
            self.selection = ended_with.selection;
            self.reset_loudness = ended_with.reset_loudness;
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
            other => panic!("the strip fader writes SetTrackMix; it wrote {other:?}"),
        }
    }

    fn upserted_bus(operation: &Operation) -> &AudioBus {
        match operation {
            Operation::UpsertAudioBus { bus } => bus,
            other => panic!("expected one UpsertAudioBus; got {other:?}"),
        }
    }

    fn set_master(operation: &Operation) -> &AudioMaster {
        match operation {
            Operation::SetAudioMaster { master } => master,
            other => panic!("expected one SetAudioMaster; got {other:?}"),
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

    // -----------------------------------------------------------------
    // AU2 Part B
    // -----------------------------------------------------------------

    /// One chain built from the descriptor table, exactly as `+ Effect` does.
    fn chain_effects(names: &[&str]) -> Vec<Effect> {
        let mut effects = Vec::new();
        for name in names {
            mixer_pane_ui::insert_audio_effect(&mut effects, name);
        }
        effects
    }

    /// The worst case R10 names: a bus with six nodes, a sidechain, and a
    /// non-zero gain.
    fn chain_document() -> Document {
        let mut document = mixer_document();
        let bus = &mut document.audio_mix.buses[0];
        bus.gain_tenth_db = -30;
        bus.effects = chain_effects(&[
            "audio_gain",
            "audio_parametric_eq",
            "audio_compressor",
            "audio_gate",
            "audio_ducking",
            "audio_true_peak_limiter",
        ]);
        document
    }

    /// The same worst case with a master chain to edit as well.
    fn chain_document_with_master() -> Document {
        let mut document = chain_document();
        document.audio_mix.master = AudioMaster {
            gain_tenth_db: -20,
            effects: chain_effects(&["audio_gain"]),
        };
        document
    }

    /// A snapshot with every figure present, as a meter mid-programme reports.
    fn measured_loudness() -> LoudnessSnapshot {
        LoudnessSnapshot {
            momentary_lufs_hundredths: Some(-1_830),
            short_term_lufs_hundredths: Some(-1_705),
            integrated_lufs_hundredths: Some(-1_600),
            loudness_range_lu_hundredths: Some(620),
            true_peak_dbtp_hundredths: Some(-130),
            programme_seconds: 42,
        }
    }

    /// A playback double that reports one fixed snapshot and counts the
    /// resets and peak reads it receives (AU3 §4.4). The engine's meter is
    /// media's to prove; the app only reads the snapshot and asks for resets.
    struct FixedLoudnessPlayback {
        snapshot: LoudnessSnapshot,
        resets: AtomicUsize,
        peak_reads: AtomicUsize,
    }

    impl FixedLoudnessPlayback {
        fn new(snapshot: LoudnessSnapshot) -> Self {
            Self {
                snapshot,
                resets: AtomicUsize::new(0),
                peak_reads: AtomicUsize::new(0),
            }
        }
    }

    impl Playback for FixedLoudnessPlayback {
        fn set_document(&self, _document: Arc<Document>) {}
        fn request_frame(&self, _at: TimeCode) {}
        fn frames(&self) -> crossbeam_channel::Receiver<(TimeCode, kinewright_core::FrameTexture)> {
            crossbeam_channel::bounded(0).1
        }
        fn events(&self) -> crossbeam_channel::Receiver<kinewright_core::MediaEvent> {
            crossbeam_channel::bounded(0).1
        }
        fn play(&self, _from: TimeCode) {}
        fn pause(&self) {}
        fn seek(&self, _to: TimeCode) {}
        fn position(&self) -> TimeCode {
            TimeCode::ZERO
        }
        fn output_peaks(&self) -> [f32; 2] {
            [0.0, 0.0]
        }
        fn mix_peaks(&self) -> MixPeaks {
            self.peak_reads.fetch_add(1, Ordering::SeqCst);
            MixPeaks {
                master: [0.5, 0.25],
                ..MixPeaks::default()
            }
        }
        fn loudness(&self) -> LoudnessSnapshot {
            self.snapshot
        }
        fn reset_loudness(&self) {
            self.resets.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Track 1 routed to the bus, so `+ Bus` has something to refuse.
    fn routed_document() -> Document {
        let mut document = mixer_document();
        document.audio_mix.buses[0].tracks = vec![TrackId(1), TrackId(2)];
        document
    }

    fn measure_bus_strip(document: &Document, index: usize) -> egui::Vec2 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let levels = MixerMeterLevels::default();
        let mut requested = None;
        let mut chain = MixerChainEdits::default();
        let mut measured = egui::Vec2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.vertical(|ui| {
                bus_strip(
                    ui,
                    document,
                    &document.audio_mix.buses[index],
                    None,
                    &levels,
                    &mut requested,
                    &mut chain,
                );
                measured = ui.min_rect().size();
            });
        });
        measured
    }

    fn measure_master_strip(document: &Document, loudness: LoudnessSnapshot) -> egui::Vec2 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let levels = MixerMeterLevels::default();
        let mut requested = None;
        let mut chain = MixerChainEdits::default();
        let mut measured = egui::Vec2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.vertical(|ui| {
                master_strip(
                    ui,
                    document,
                    None,
                    &levels,
                    loudness,
                    &mut requested,
                    &mut chain,
                );
                measured = ui.min_rect().size();
            });
        });
        measured
    }

    fn measure_chain_card(document: &Document, index: usize) -> egui::Vec2 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let levels = MixerMeterLevels::default();
        let mut edits = MixerChainEdits::default();
        let bus = &document.audio_mix.buses[0];
        let mut measured = egui::Vec2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
                mixer_pane_ui::chain_card(
                    ui,
                    MixerChain::Bus(bus),
                    index,
                    false,
                    &levels,
                    &mut edits,
                );
                measured = ui.min_rect().size();
            });
        });
        measured
    }

    /// AU2 §7 B19 and §6.5's arithmetic: the bus and master strips fit the
    /// same 240 px dock budget the track strip does, in their worst case.
    ///
    /// Measured on this build: a bus strip is 215 px whether it carries one
    /// node or six, because the node count is one line either way, and the
    /// master strip is 210 px with AU3 §4.5's integrated line under the fader
    /// (196 before it), whether that line reads `I —` or a figure. The track
    /// strip, which gained the `+ Bus` button, is 218 px plain and 232 px in
    /// both of its loaded states.
    #[test]
    fn a_bus_and_master_strip_fit_the_mixer_dock() {
        const BUDGET: f32 = 240.0;
        // DESIGN.md's `210 for the master`, held to a pixel so the document's
        // number and the real layout cannot drift apart silently.
        const MASTER: f32 = 210.0;
        // DESIGN.md's `215 for a bus strip`, held the same way.
        const BUS: f32 = 215.0;
        let mut worst_case = chain_document();
        worst_case.audio_mix.master = AudioMaster {
            gain_tenth_db: -30,
            effects: chain_effects(&["audio_gain", "audio_true_peak_limiter"]),
        };
        for (label, measured, expected) in [
            (
                "plain bus",
                measure_bus_strip(&mixer_document(), 0),
                Some(BUS),
            ),
            ("six-node bus", measure_bus_strip(&worst_case, 0), Some(BUS)),
            (
                "plain master",
                measure_master_strip(&mixer_document(), LoudnessSnapshot::default()),
                Some(MASTER),
            ),
            (
                "loaded master",
                measure_master_strip(&worst_case, measured_loudness()),
                Some(MASTER),
            ),
        ] {
            assert!(
                measured.y <= BUDGET,
                "a {label} strip is {} px tall, over the {BUDGET} px budget",
                measured.y
            );
            if let Some(expected) = expected {
                assert!(
                    (measured.y - expected).abs() <= 1.0,
                    "DESIGN.md says `{expected}` for this strip; a {label} strip measured {} px",
                    measured.y
                );
            }
            assert!(
                (measured.x - size::MIXER_STRIP_WIDTH).abs() <= 1.0,
                "a {label} strip is one column wide: {} px",
                measured.x
            );
        }
    }

    /// Lay the pane out on its own in a wide viewport and report its size.
    fn measure_chain_pane(document: &Document, selection: MixerSelection) -> egui::Vec2 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let levels = MixerMeterLevels::default();
        let mut edits = MixerChainEdits::default();
        let mut measured = egui::Vec2::ZERO;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1_200.0, 800.0),
                )),
                ..Default::default()
            },
            |ui| {
                ui.horizontal_top(|ui| {
                    mixer_pane_ui::chain_pane(
                        ui,
                        document,
                        selection,
                        &levels,
                        measured_loudness(),
                        STREAMING_PLATFORM_TARGET,
                        &mut edits,
                    );
                    measured = ui.min_rect().size();
                });
            },
        );
        measured
    }

    /// AU2 §6.6: the pane is one `MIXER_CHAIN_PANE_WIDTH` column however much
    /// room it is offered.
    ///
    /// This is a regression pin, not a formality: a card's wrapped control row
    /// is a stack of nested `Ui`s, and while they were allocated at an unknown
    /// width `horizontal_wrapped` never wrapped — a parametric EQ's eighteen
    /// controls ran in one line to the far edge of the window and dragged the
    /// card's frame, the EQ well, and the reduction bar out with them.
    #[test]
    fn the_chain_pane_is_one_column_however_wide_the_dock_is() {
        for (label, selection) in [
            ("bus", MixerSelection::Bus(AudioBusId(1))),
            ("master", MixerSelection::Master),
        ] {
            let measured = measure_chain_pane(&chain_document(), selection);
            assert!(
                (measured.x - size::MIXER_CHAIN_PANE_WIDTH).abs() <= 1.0,
                "the {label} pane is {} px wide in a 1 200 px viewport",
                measured.x
            );
        }
    }

    /// AU2 §7 B19: a collapsed card is one row, no taller than an icon
    /// button, which is what makes "one expanded at a time" affordable in a
    /// dock that leaves about 262 px. Measured at 18 px on this build.
    #[test]
    fn a_collapsed_chain_card_is_one_icon_button_row() {
        let document = chain_document();
        for index in 0..document.audio_mix.buses[0].effects.len() {
            let measured = measure_chain_card(&document, index);
            assert!(
                measured.y <= size::ICON_BUTTON,
                "collapsed card {index} is {} px tall, over the {} px row",
                measured.y,
                size::ICON_BUTTON
            );
        }
    }

    /// AU2 §7 B20: a drag on a bus fader writes one coalesced
    /// `UpsertAudioBus` per frame under the bus's own key, carrying every
    /// other field of the bus unchanged.
    #[test]
    fn a_bus_fader_drag_writes_coalesced_upsert_audio_bus() {
        let mut harness = MixerHarness::new(mixer_document());
        let laid_out = harness.frame(Vec::new());
        assert!(
            laid_out.operations().is_empty(),
            "laying the strips out writes nothing"
        );
        let fader = harness.rect("bus_fader:1");

        let press = rail_point(fader, 0.9);
        let pressed = harness.frame(vec![moved(press), pointer(press, true)]);
        assert!(pressed.gesture_started(), "the press opens an undo gesture");
        assert_eq!(
            pressed.coalesce_key(),
            Some("audio_bus:1"),
            "a live bus frame carries the bus's key"
        );
        let bus = upserted_bus(&pressed.operations()[0]);
        assert_eq!(bus.id, AudioBusId(1));
        assert_eq!(bus.name, "Dialogue");
        assert_eq!(bus.tracks, vec![TrackId(2)]);
        assert_eq!(bus.ducking_sidechain_tracks, vec![TrackId(1)]);
        assert_eq!(bus.effects.len(), 1, "a fader does not touch the chain");
        let low = bus.gain_tenth_db;
        assert!(
            low < -100,
            "pressing low on the rail sets a low gain: {low}"
        );

        let up = rail_point(fader, 0.4);
        let dragged = harness.frame(vec![moved(up)]);
        assert_eq!(dragged.coalesce_key(), Some("audio_bus:1"));
        assert!(!dragged.gesture_started(), "the gesture opens once");
        assert!(
            upserted_bus(&dragged.operations()[0]).gain_tenth_db > low,
            "dragging up raises the bus gain"
        );

        let released = harness.frame(vec![pointer(up, false)]);
        for operation in released.operations() {
            assert!(matches!(operation, Operation::UpsertAudioBus { .. }));
        }
        assert_eq!(
            harness.document.audio_mix.buses[0].gain_tenth_db,
            upserted_bus(&dragged.operations()[0]).gain_tenth_db,
            "the drag landed in the document"
        );
    }

    /// AU2 §7 B20: the master fader writes `SetAudioMaster` under the master
    /// key, and the pan-law choice writes `SetPanLaw`.
    #[test]
    fn the_master_fader_and_the_pan_law_write_their_own_operations() {
        let mut harness = MixerHarness::new(mixer_document());
        let _ = harness.frame(Vec::new());
        let fader = harness.rect("master_fader");

        let press = rail_point(fader, 0.9);
        let pressed = harness.frame(vec![moved(press), pointer(press, true)]);
        assert_eq!(pressed.coalesce_key(), Some("audio_master"));
        assert!(pressed.gesture_started());
        let master = set_master(&pressed.operations()[0]);
        assert!(
            master.gain_tenth_db < -100,
            "the master fader wrote {}",
            master.gain_tenth_db
        );
        assert!(
            master.effects.is_empty(),
            "a fader does not touch the chain"
        );
        let _ = harness.frame(vec![pointer(press, false)]);

        let mut harness = MixerHarness::new(mixer_document()).editing(MixerSelection::Master);
        let _ = harness.frame(Vec::new());
        let constant = harness.rect("pan_law:constant_power");
        let _ = harness.frame(vec![
            moved(constant.center()),
            pointer(constant.center(), true),
        ]);
        let clicked = harness.frame(vec![pointer(constant.center(), false)]);
        assert_eq!(
            clicked.operations(),
            [Operation::SetPanLaw {
                law: PanLaw::ConstantPower
            }],
            "the law is one discrete document-level choice"
        );
        assert_eq!(
            clicked.coalesce_key(),
            None,
            "a radio choice is never the tail of a drag"
        );
    }

    /// AU2 §7 B18, the fold pin: a frame in which a bus's strip fader and one
    /// of its pane controls both change produces exactly one
    /// `UpsertAudioBus` carrying both changes.
    ///
    /// The two changes land in one frame because the pane control is a
    /// `DragValue` in text-edit mode: the frame that presses the strip fader
    /// also carries the `Enter` that commits the readout, and a `DragValue`
    /// with `update_while_editing(false)` applies its value on the frame it
    /// loses focus. Two controls of one chain pushing their own operations
    /// would each carry the other's old value, which is exactly the clobber
    /// R9 forbids.
    #[test]
    fn one_frame_of_a_strip_fader_and_a_pane_control_writes_one_upsert() {
        // What the same press writes on its own. Asserting the folded bus
        // carries exactly this is what makes the test a fold pin: `< 0` would
        // pass on the fixture's own -30 even if the press were swallowed.
        let expected_gain = {
            let mut control =
                MixerHarness::new(chain_document()).editing(MixerSelection::Bus(AudioBusId(1)));
            let _ = control.frame(Vec::new());
            let fader = control.rect("bus_fader:1");
            let press = rail_point(fader, 0.8);
            let pressed = control.frame(vec![moved(press), pointer(press, true)]);
            upserted_bus(&pressed.operations()[0]).gain_tenth_db
        };
        assert_ne!(
            expected_gain,
            chain_document().audio_mix.buses[0].gain_tenth_db,
            "the press must actually move the fader"
        );

        let mut harness =
            MixerHarness::new(chain_document()).editing(MixerSelection::Bus(AudioBusId(1)));
        let _ = harness.frame(Vec::new());
        let parameter = harness.rect("param:1:gain_tenth_db");
        let fader = harness.rect("bus_fader:1");

        // Open the pane control's readout and type a value into it.
        let point = parameter.center();
        let _ = harness.frame(vec![moved(point), pointer(point, true)]);
        let _ = harness.frame(vec![pointer(point, false)]);
        let _ = harness.frame(vec![egui::Event::Text("-4.0".to_owned())]);

        // One frame: the press moves the bus gain and the `Enter` commits the
        // pane readout.
        let press = rail_point(fader, 0.8);
        let folded = harness.frame(vec![
            moved(press),
            pointer(press, true),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        assert_eq!(
            folded.operations().len(),
            1,
            "two controls of one chain fold into one whole-chain set: {:?}",
            folded.operations()
        );
        assert_eq!(folded.coalesce_key(), Some("audio_bus:1"));
        let bus = upserted_bus(&folded.operations()[0]);
        assert_eq!(
            bus.gain_tenth_db, expected_gain,
            "the strip fader's own change is in the folded bus"
        );
        assert_eq!(
            mixer_pane_ui::parameter_value(&bus.effects[0], "gain_tenth_db"),
            -40,
            "the pane control's change is in the same folded bus: {bus:?}"
        );
    }

    /// AU2 §7 B18, through `mixer_body`: a frame that moves a bus's strip
    /// fader and commits a control on the *master* pane writes exactly one
    /// `UpsertAudioBus` and one `SetAudioMaster`.
    #[test]
    fn one_frame_touching_a_bus_and_the_master_writes_one_of_each_through_the_body() {
        let mut harness =
            MixerHarness::new(chain_document_with_master()).editing(MixerSelection::Master);
        let _ = harness.frame(Vec::new());
        // The master pane shows the master's own chain, so this readout is the
        // master's gain node, not the bus's.
        let parameter = harness.rect("param:1:gain_tenth_db");
        let fader = harness.rect("bus_fader:1");

        let point = parameter.center();
        let _ = harness.frame(vec![moved(point), pointer(point, true)]);
        let _ = harness.frame(vec![pointer(point, false)]);
        let _ = harness.frame(vec![egui::Event::Text("-2.5".to_owned())]);

        let press = rail_point(fader, 0.8);
        let folded = harness.frame(vec![
            moved(press),
            pointer(press, true),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        assert_eq!(
            folded.operations().len(),
            2,
            "one operation per touched chain: {:?}",
            folded.operations()
        );
        let bus = upserted_bus(&folded.operations()[0]);
        assert_eq!(bus.id, AudioBusId(1));
        assert!(
            bus.gain_tenth_db < -100,
            "the strip fader moved the bus: {}",
            bus.gain_tenth_db
        );
        let master = set_master(&folded.operations()[1]);
        assert_eq!(
            master.gain_tenth_db, -20,
            "the master fader was not touched"
        );
        assert_eq!(
            mixer_pane_ui::parameter_value(&master.effects[0], "gain_tenth_db"),
            -25,
            "the master pane's control is in the master's own operation"
        );
        assert_eq!(
            folded.coalesce_key(),
            Some("audio_bus:1"),
            "one key per frame; the fold's first operation names it"
        );
    }

    /// AU2 §7 B20, through `mixer_body`: a pointer drag on a pane control is a
    /// live gesture coalesced under its chain's key, exactly as a strip fader
    /// drag is.
    #[test]
    fn a_pane_control_drag_is_live_and_coalesced_under_the_chains_key() {
        let mut harness =
            MixerHarness::new(chain_document()).editing(MixerSelection::Bus(AudioBusId(1)));
        let _ = harness.frame(Vec::new());
        let parameter = harness.rect("param:1:gain_tenth_db");
        let start = parameter.center();

        let pressed = harness.frame(vec![moved(start), pointer(start, true)]);
        assert!(
            pressed.operations().is_empty(),
            "a press that has not moved is not an edit: {:?}",
            pressed.operations()
        );
        // A `DragValue` is not being dragged until the pointer clears egui's
        // drag threshold, so the gesture opens on the first moved frame.
        let moved_to = egui::pos2(start.x + 24.0, start.y);
        let dragged = harness.frame(vec![moved(moved_to)]);
        assert!(
            dragged.gesture_started(),
            "the drag opens a fresh undo gesture"
        );
        assert_eq!(
            dragged.operations().len(),
            1,
            "one whole-chain set per dragged frame: {:?}",
            dragged.operations()
        );
        assert_eq!(
            dragged.coalesce_key(),
            Some("audio_bus:1"),
            "a live pane frame carries the chain's key"
        );
        let bus = upserted_bus(&dragged.operations()[0]);
        assert!(
            mixer_pane_ui::parameter_value(&bus.effects[0], "gain_tenth_db") > 0,
            "dragging right raises the value: {bus:?}"
        );
        assert_eq!(bus.gain_tenth_db, -30, "the bus fader was not touched");

        // `is_live_drag` includes `drag_stopped()` so a value that changes on
        // the release frame stays inside the gesture. A `DragValue` release
        // carries no delta, so either nothing is written or it is written
        // under the same key — never a second, discrete undo entry.
        let released = harness.frame(vec![pointer(moved_to, false)]);
        assert!(
            released.operations().is_empty() || released.coalesce_key() == Some("audio_bus:1"),
            "the release frame must not open a second undo entry: {:?}",
            released.operations()
        );
    }

    /// AU2 §7 B20, through `mixer_body`: opening the `+ Effect` menu and
    /// clicking an entry appends that node at its descriptor neutrals as one
    /// discrete operation, and a gated entry writes nothing.
    #[test]
    fn the_effect_menu_inserts_through_the_body_and_its_gates_hold() {
        let mut harness =
            MixerHarness::new(chain_document()).editing(MixerSelection::Bus(AudioBusId(1)));
        let _ = harness.frame(Vec::new());
        let menu = harness.rect("add_effect");
        let _ = harness.frame(vec![moved(menu.center()), pointer(menu.center(), true)]);
        let _ = harness.frame(vec![pointer(menu.center(), false)]);
        // The menu's entries only exist once it is open.
        let _ = harness.frame(Vec::new());
        let entry = harness.rect("insert:0:audio_compressor");

        let _ = harness.frame(vec![moved(entry.center()), pointer(entry.center(), true)]);
        let clicked = harness.frame(vec![pointer(entry.center(), false)]);
        assert_eq!(
            clicked.coalesce_key(),
            None,
            "an insertion is discrete, not the tail of a drag"
        );
        let bus = upserted_bus(&clicked.operations()[0]);
        assert_eq!(bus.effects.len(), 7);
        let inserted = bus.effects.last().expect("the node was appended");
        assert_eq!(inserted.name, "audio_compressor");
        assert_eq!(
            inserted.id,
            EffectId(7),
            "one past the highest id in this chain"
        );
        let descriptor =
            kinewright_core::effect_descriptor("audio_compressor").expect("registered");
        assert_eq!(inserted.parameters.len(), descriptor.parameters.len());
        for parameter in descriptor.parameters {
            assert_eq!(
                inserted.parameters.get(parameter.name),
                Some(&ParamValue::Integer(parameter.neutral))
            );
        }

        // The master has no sidechain, so Ducking is disabled there and a
        // press on it writes nothing.
        let mut harness =
            MixerHarness::new(chain_document_with_master()).editing(MixerSelection::Master);
        let _ = harness.frame(Vec::new());
        let menu = harness.rect("add_effect");
        let _ = harness.frame(vec![moved(menu.center()), pointer(menu.center(), true)]);
        let _ = harness.frame(vec![pointer(menu.center(), false)]);
        let _ = harness.frame(Vec::new());
        let entry = harness.rect("insert:0:audio_ducking");
        let _ = harness.frame(vec![moved(entry.center()), pointer(entry.center(), true)]);
        let clicked = harness.frame(vec![pointer(entry.center(), false)]);
        assert!(
            clicked.operations().is_empty(),
            "Ducking has no sidechain on the master: {:?}",
            clicked.operations()
        );
    }

    /// AU2 §7 B18: the fold itself, at the seam — one operation per touched
    /// chain, in bus-then-master order, and a copy equal to the document is
    /// not an edit.
    #[test]
    fn a_frame_touching_a_bus_and_the_master_writes_one_of_each() {
        let document = chain_document();
        let mut edits = InspectorEdits::default();
        let mut chain = MixerChainEdits::default();
        chain.bus(&document.audio_mix.buses[0]).gain_tenth_db = -55;
        chain.master(&document.audio_mix.master).gain_tenth_db = -20;
        chain.mark_live(true);
        chain.drain_into(&document, &mut edits);

        assert_eq!(edits.operations().len(), 2);
        assert_eq!(upserted_bus(&edits.operations()[0]).gain_tenth_db, -55);
        assert_eq!(set_master(&edits.operations()[1]).gain_tenth_db, -20);
        assert_eq!(
            edits.coalesce_key(),
            Some("audio_bus:1"),
            "one key per frame; the fold's first operation names it"
        );

        // A copy equal to what the document holds is not an edit.
        let mut edits = InspectorEdits::default();
        let mut chain = MixerChainEdits::default();
        let _ = chain.bus(&document.audio_mix.buses[0]);
        let _ = chain.master(&document.audio_mix.master);
        chain.set_pan_law(PanLaw::Balance);
        chain.drain_into(&document, &mut edits);
        assert!(
            edits.operations().is_empty(),
            "an untouched copy writes nothing: {:?}",
            edits.operations()
        );
    }

    /// AU2 §7 B20: `+ Bus` builds the bus §6.5 names, and states why it
    /// cannot when the track has no audio or is already routed.
    #[test]
    fn add_bus_creates_the_contracts_bus_and_states_why_it_cannot() {
        let document = mixer_document();
        assert_eq!(
            add_bus_block(&document, &document.tracks[0], true),
            None,
            "V1 carries audio and is not routed"
        );
        assert_eq!(
            add_bus_block(&document, &document.tracks[1], false),
            Some(NO_AUDIO_REASON)
        );
        let routed = routed_document();
        assert_eq!(
            add_bus_block(&routed, &routed.tracks[0], true),
            Some(ALREADY_ROUTED_REASON)
        );

        let mut harness = MixerHarness::new(mixer_document());
        let _ = harness.frame(Vec::new());
        let add = harness.rect("add_bus:1");
        let _ = harness.frame(vec![moved(add.center()), pointer(add.center(), true)]);
        let clicked = harness.frame(vec![pointer(add.center(), false)]);
        assert_eq!(
            clicked.operations(),
            [Operation::UpsertAudioBus {
                bus: AudioBus {
                    id: AudioBusId(2),
                    name: "V1".to_owned(),
                    tracks: vec![TrackId(1)],
                    gain_tenth_db: 0,
                    effects: Vec::new(),
                    ducking_sidechain_tracks: Vec::new(),
                }
            }],
            "the bus is created around the strip you are pointing at"
        );
        assert_eq!(clicked.coalesce_key(), None, "creating a bus is discrete");

        // The disabled button writes nothing however hard it is pressed.
        let mut harness = MixerHarness::new(mixer_document());
        let _ = harness.frame(Vec::new());
        let add = harness.rect("add_bus:2");
        let _ = harness.frame(vec![moved(add.center()), pointer(add.center(), true)]);
        let clicked = harness.frame(vec![pointer(add.center(), false)]);
        assert!(
            clicked.operations().is_empty(),
            "a track with no audio cannot be given a bus: {:?}",
            clicked.operations()
        );
    }

    /// AU2 §7 B20: the six offered nodes, their eight display names, and the
    /// two gates.
    #[test]
    fn the_effect_menu_offers_six_nodes_and_gates_the_two_it_must() {
        assert_eq!(
            mixer_pane_ui::INSERTABLE_AUDIO_EFFECTS,
            [
                "audio_gain",
                "audio_parametric_eq",
                "audio_compressor",
                "audio_gate",
                "audio_ducking",
                "audio_true_peak_limiter",
            ]
        );
        for legacy in ["audio_eq", "audio_limiter"] {
            assert!(
                !mixer_pane_ui::is_audio_effect_insertable(legacy),
                "{legacy} is retained and processed but never offered"
            );
            assert!(
                kinewright_core::is_audio_effect(legacy),
                "{legacy} is still a valid chain node"
            );
        }
        for name in mixer_pane_ui::INSERTABLE_AUDIO_EFFECTS {
            assert!(mixer_pane_ui::is_audio_effect_insertable(name));
            assert!(
                !crate::inspector_ui::is_effect_insertable(name),
                "the clip inspector still refuses every audio node"
            );
        }
        for (name, display) in [
            ("audio_gain", "Gain"),
            ("audio_eq", "EQ (legacy)"),
            ("audio_compressor", "Compressor"),
            ("audio_ducking", "Ducking"),
            ("audio_limiter", "Limiter (legacy)"),
            ("audio_parametric_eq", "Parametric EQ"),
            ("audio_gate", "Gate"),
            ("audio_true_peak_limiter", "True-peak limiter"),
        ] {
            assert_eq!(effect_display_name(name), display);
        }

        // Ducking needs a sidechain; the master never has one.
        assert_eq!(
            mixer_pane_ui::insertion_block(&[], &[], "audio_ducking"),
            Some(mixer_pane_ui::DUCKING_NEEDS_SIDECHAIN)
        );
        assert_eq!(
            mixer_pane_ui::insertion_block(&[], &[TrackId(1)], "audio_ducking"),
            None
        );
        // Two true-peak limiters and a 10 ms compressor exhaust the budget.
        let mut spent = chain_effects(&["audio_compressor"]);
        spent[0]
            .parameters
            .insert("lookahead_milliseconds".to_owned(), ParamValue::Integer(10));
        mixer_pane_ui::insert_audio_effect(&mut spent, "audio_true_peak_limiter");
        spent[1]
            .parameters
            .insert("lookahead_milliseconds".to_owned(), ParamValue::Integer(10));
        assert_eq!(
            mixer_pane_ui::insertion_block(&spent, &[TrackId(1)], "audio_true_peak_limiter"),
            Some(mixer_pane_ui::LOOKAHEAD_BUDGET_SPENT)
        );
        assert_eq!(
            mixer_pane_ui::insertion_block(&spent, &[TrackId(1)], "audio_gain"),
            None,
            "a node that declares no lookahead is never over budget"
        );
    }

    /// AU2 §7 B20: an insertion writes every descriptor neutral explicitly and
    /// takes `max + 1` within its own chain.
    #[test]
    fn an_insertion_writes_every_neutral_and_allocates_max_plus_one() {
        let mut effects = Vec::new();
        mixer_pane_ui::insert_audio_effect(&mut effects, "audio_true_peak_limiter");
        assert_eq!(effects[0].id, kinewright_core::EffectId(1));
        assert!(effects[0].keyframes.is_empty());
        let descriptor = kinewright_core::effect_descriptor("audio_true_peak_limiter")
            .expect("the node is registered");
        assert_eq!(effects[0].parameters.len(), descriptor.parameters.len());
        for parameter in descriptor.parameters {
            assert_eq!(
                effects[0].parameters.get(parameter.name),
                Some(&ParamValue::Integer(parameter.neutral)),
                "{} inserts at its descriptor neutral",
                parameter.name
            );
        }
        // The two the contract calls out as audible on insertion.
        assert_eq!(
            mixer_pane_ui::parameter_value(&effects[0], "ceiling_tenth_db"),
            0
        );
        assert_eq!(
            mixer_pane_ui::insertion_lookahead_milliseconds("audio_true_peak_limiter"),
            5
        );

        effects[0].id = kinewright_core::EffectId(9);
        mixer_pane_ui::insert_audio_effect(&mut effects, "audio_gain");
        assert_eq!(
            effects[1].id,
            kinewright_core::EffectId(10),
            "the chain allocates its own ids, one past its own highest"
        );
    }

    /// AU2 §7 B20: a routing or sidechain checkbox that would be rejected is
    /// disabled with the reason, and writes nothing when pressed anyway.
    #[test]
    fn routing_and_sidechain_checkboxes_are_disabled_with_their_reasons() {
        let document = chain_document();
        let bus = &document.audio_mix.buses[0];
        assert_eq!(
            mixer_pane_ui::routing_block(&document, bus, TrackId(2)).as_deref(),
            Some(mixer_pane_ui::LAST_TRACK_REASON),
            "unchecking the last track would be InvalidAudioBus"
        );
        assert_eq!(
            mixer_pane_ui::routing_block(&document, bus, TrackId(1)),
            None,
            "an unrouted track can join"
        );
        assert_eq!(
            mixer_pane_ui::sidechain_block(bus, TrackId(1)),
            Some(mixer_pane_ui::LAST_SIDECHAIN_REASON),
            "this bus ducks, so its last sidechain cannot go"
        );
        assert_eq!(
            mixer_pane_ui::sidechain_block(&document.audio_mix.buses[0], TrackId(2)),
            None
        );
        let mut two_buses = chain_document();
        two_buses.audio_mix.buses.push(AudioBus {
            id: AudioBusId(2),
            name: "Music".to_owned(),
            tracks: vec![TrackId(1)],
            gain_tenth_db: 0,
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
        });
        assert_eq!(
            mixer_pane_ui::routing_block(&two_buses, &two_buses.audio_mix.buses[0], TrackId(1))
                .as_deref(),
            Some("already on Music"),
            "a track claimed elsewhere names the bus that has it"
        );

        // Pressing the disabled last-track box writes nothing; pressing an
        // enabled one folds into the bus.
        let mut harness =
            MixerHarness::new(chain_document()).editing(MixerSelection::Bus(AudioBusId(1)));
        let _ = harness.frame(Vec::new());
        let last = harness.rect("route:2");
        let _ = harness.frame(vec![moved(last.center()), pointer(last.center(), true)]);
        let clicked = harness.frame(vec![pointer(last.center(), false)]);
        assert!(
            clicked.operations().is_empty(),
            "the last track cannot be unrouted: {:?}",
            clicked.operations()
        );

        let join = harness.rect("route:1");
        let _ = harness.frame(vec![moved(join.center()), pointer(join.center(), true)]);
        let clicked = harness.frame(vec![pointer(join.center(), false)]);
        assert_eq!(
            upserted_bus(&clicked.operations()[0]).tracks,
            vec![TrackId(2), TrackId(1)],
            "checking a track routes it to this bus"
        );
    }

    /// AU2 §7 B20: every card control produces the folded operation the
    /// contract names — a bypass flip, a move, and a remove.
    #[test]
    fn a_bypass_a_move_and_a_remove_each_fold_into_one_upsert() {
        let mut harness =
            MixerHarness::new(chain_document()).editing(MixerSelection::Bus(AudioBusId(1)));
        let _ = harness.frame(Vec::new());

        let bypass = harness.rect("bypass:1");
        let _ = harness.frame(vec![moved(bypass.center()), pointer(bypass.center(), true)]);
        let clicked = harness.frame(vec![pointer(bypass.center(), false)]);
        let bus = upserted_bus(&clicked.operations()[0]);
        assert_eq!(mixer_pane_ui::parameter_value(&bus.effects[0], "bypass"), 1);
        assert_eq!(bus.effects.len(), 6, "a bypass moves nothing else");

        let _ = harness.frame(Vec::new());
        let down = harness.rect("down:1");
        let _ = harness.frame(vec![moved(down.center()), pointer(down.center(), true)]);
        let clicked = harness.frame(vec![pointer(down.center(), false)]);
        let names = upserted_bus(&clicked.operations()[0])
            .effects
            .iter()
            .map(|effect| effect.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(names[0], "audio_parametric_eq");
        assert_eq!(names[1], "audio_gain");

        let _ = harness.frame(Vec::new());
        let remove = harness.rect("remove:1");
        let _ = harness.frame(vec![moved(remove.center()), pointer(remove.center(), true)]);
        let clicked = harness.frame(vec![pointer(remove.center(), false)]);
        let bus = upserted_bus(&clicked.operations()[0]);
        assert_eq!(bus.effects.len(), 5);
        assert!(
            bus.effects.iter().all(|effect| effect.name != "audio_gain"),
            "Remove took the node it named: {bus:?}"
        );
    }

    /// AU2 §7 B21: the gain-reduction bar fills from the right, grows
    /// leftward as reduction rises, and reads `MixPeaks.gain_reduction` by
    /// `(chain, effect id)`.
    #[test]
    fn the_gain_reduction_bar_fills_from_the_right() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 4.0), egui::vec2(200.0, 6.0));
        let quiet = mixer_pane_ui::reduction_fill_rect(rect, 0.0);
        assert!(quiet.width() <= 0.0, "no reduction paints no bar");
        let some = mixer_pane_ui::reduction_fill_rect(rect, 6.0);
        let more = mixer_pane_ui::reduction_fill_rect(rect, 12.0);
        for fill in [some, more] {
            assert!(
                (fill.right() - rect.right()).abs() < 1e-3,
                "the bar is anchored to the right edge"
            );
            assert!((fill.top() - rect.top()).abs() < 1e-3);
        }
        assert!(
            more.left() < some.left(),
            "more reduction grows leftward: {} then {}",
            some.left(),
            more.left()
        );
        assert!(
            (more.width() - rect.width() * 0.5).abs() < 1e-3,
            "12 dB of a {MIXER_REDUCTION_METER_RANGE_DB} dB range is half the bar"
        );
        assert!(
            (mixer_pane_ui::reduction_fill_rect(rect, 90.0).width() - rect.width()).abs() < 1e-3,
            "the bar clamps full"
        );
        assert_eq!(mixer_pane_ui::reduction_readout(3.25), "-3.2 dB");

        // The level comes from the peaks table, keyed by chain and effect id.
        let mut levels = MixerMeterLevels::default();
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let peaks = MixPeaks {
            gain_reduction: vec![(AudioChain::Bus(AudioBusId(1)), EffectId(3), 12.0)],
            ..MixPeaks::default()
        };
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            levels.advance(ui, &peaks, true);
        });
        assert!(
            (levels.reduction(AudioChain::Bus(AudioBusId(1)), EffectId(3)) - 12.0).abs() < 1e-3
        );
        assert!(
            levels
                .reduction(AudioChain::Bus(AudioBusId(1)), EffectId(4))
                .abs()
                < 1e-6,
            "an absent key reads zero"
        );
        assert!(
            levels.reduction(AudioChain::Master, EffectId(3)).abs() < 1e-6,
            "the master chain is a different key"
        );

        // Nothing playing means nothing to report.
        let mut stopped = MixerMeterLevels::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            stopped.advance(ui, &peaks, false);
        });
        assert!(stopped.reduction(AudioChain::Bus(AudioBusId(1)), EffectId(3)) <= 0.0);
    }

    /// AU2 §7 B21: the EQ well samples the node's own magnitude function at
    /// 96 log-spaced points, and a boosted band paints above the 0 dB line.
    #[test]
    fn the_eq_well_samples_the_nodes_own_magnitude() {
        let mut effects = Vec::new();
        mixer_pane_ui::insert_audio_effect(&mut effects, "audio_parametric_eq");
        let mut boosted = effects[0].clone();
        boosted
            .parameters
            .insert("band1_gain_tenth_db".to_owned(), ParamValue::Integer(60));
        boosted
            .parameters
            .insert("band1_hertz".to_owned(), ParamValue::Integer(1_000));

        let sampled = mixer_pane_ui::eq_well_magnitudes(&boosted);
        assert_eq!(sampled.len(), mixer_pane_ui::EQ_WELL_SAMPLES);
        for (index, measured) in sampled.iter().enumerate() {
            let expected = kinewright_media::parametric_eq_magnitude_db(
                &boosted,
                TimeCode::ZERO,
                mixer_pane_ui::eq_well_hertz(index),
                mixer_pane_ui::EQ_WELL_SAMPLE_RATE,
            );
            assert!(
                (measured - expected).abs() < 1e-6,
                "the well plots the node's own response at point {index}"
            );
        }
        assert!((mixer_pane_ui::eq_well_hertz(0) - 20.0).abs() < 1e-9);
        assert!(
            (mixer_pane_ui::eq_well_hertz(mixer_pane_ui::EQ_WELL_SAMPLES - 1) - 20_000.0).abs()
                < 1e-6
        );

        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 96.0));
        // 1 kHz is `log10(50) / log10(1000)` of the way across.
        let expected_x = rect.left() + rect.width() * (50.0_f32.log10() / 1_000.0_f32.log10());
        assert!(
            (mixer_pane_ui::eq_well_x(rect, 1_000.0) - expected_x).abs() < 1e-3,
            "1 kHz lands at {expected_x}, not {}",
            mixer_pane_ui::eq_well_x(rect, 1_000.0)
        );
        let zero = mixer_pane_ui::eq_well_y(rect, 0.0);
        assert!((zero - rect.center().y).abs() < 1e-3, "0 dB is the middle");
        assert!(mixer_pane_ui::eq_well_y(rect, 24.0) <= rect.top() + 1e-3);
        assert!(mixer_pane_ui::eq_well_y(rect, -24.0) >= rect.bottom() - 1e-3);

        let points = mixer_pane_ui::eq_well_points(&boosted, rect);
        assert_eq!(points.len(), mixer_pane_ui::EQ_WELL_SAMPLES);
        assert!(
            points.iter().any(|point| point.y < zero - 1.0),
            "a +6 dB band paints above the 0 dB line"
        );
        let flat = mixer_pane_ui::eq_well_points(&effects[0], rect);
        assert!(
            flat.iter().all(|point| (point.y - zero).abs() < 0.5),
            "an all-neutral parametric EQ is a flat line on the 0 dB grid"
        );
    }

    /// AU2 §6.7: a readout whose display rounds must never write on its way
    /// out.
    ///
    /// `1.2 kHz` is every frequency from 1 150 to 1 249 and `4.0:1` is every
    /// ratio from 395 to 404, and egui re-parses the *displayed* text whenever
    /// a readout loses focus. Before the value-aware parser, clicking a band
    /// frequency of 1 250 to read it and clicking away wrote 1 200 to the
    /// document and opened an undo entry.
    #[test]
    fn a_rounded_readout_writes_nothing_on_the_way_out() {
        // The two lossy units, and the exact edit each one still accepts.
        for (parameter, stored, typed, expected) in [
            ("band1_hertz", 1_250_i64, "1.3 kHz", 1_300_i64),
            ("ratio_hundredths", 405, "4.5:1", 450),
            // `0.57 * 100.0` is `56.999999999999993`, which egui truncates to
            // 56 on its way into the `i64`.
            ("band1_q_hundredths", 71, "Q 0.57", 57),
        ] {
            let mut document = chain_document();
            let bus = &mut document.audio_mix.buses[0];
            let effect = bus
                .effects
                .iter_mut()
                .find(|effect| {
                    kinewright_core::effect_descriptor(&effect.name)
                        .is_some_and(|descriptor| descriptor.parameter(parameter).is_some())
                })
                .expect("the worst-case chain carries this parameter");
            effect
                .parameters
                .insert(parameter.to_owned(), ParamValue::Integer(stored));
            let effect_id = effect.id;
            // The card that holds it has to be the expanded one.
            let effects = bus.effects.clone();
            let position = effects
                .iter()
                .position(|candidate| candidate.id == effect_id)
                .expect("the node is in the chain");
            bus.effects.swap(0, position);

            let mut harness =
                MixerHarness::new(document).editing(MixerSelection::Bus(AudioBusId(1)));
            let _ = harness.frame(Vec::new());
            let readout = harness.rect(&format!("param:{}:{parameter}", effect_id.0));
            let point = readout.center();

            // Click in, click away: the readout says exactly what it said.
            let _ = harness.frame(vec![moved(point), pointer(point, true)]);
            let _ = harness.frame(vec![pointer(point, false)]);
            let elsewhere = egui::pos2(readout.right() + 200.0, readout.center().y);
            let _ = harness.frame(vec![moved(elsewhere), pointer(elsewhere, true)]);
            let away = harness.frame(vec![pointer(elsewhere, false)]);
            assert!(
                away.operations().is_empty(),
                "reading `{parameter}` = {stored} and clicking away is not an edit: {:?}",
                away.operations()
            );
            assert_eq!(
                mixer_pane_ui::parameter_value(
                    &harness.document.audio_mix.buses[0].effects[0],
                    parameter
                ),
                stored,
                "`{parameter}` still holds the value it was showing"
            );

            // A different spelling is a real edit, and a discrete one.
            let _ = harness.frame(vec![moved(point), pointer(point, true)]);
            let _ = harness.frame(vec![pointer(point, false)]);
            let _ = harness.frame(vec![egui::Event::Text(typed.to_owned())]);
            let committed = harness.frame(vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            assert_eq!(
                committed.coalesce_key(),
                None,
                "a typed value is discrete: {:?}",
                committed.operations()
            );
            let bus = upserted_bus(&committed.operations()[0]);
            assert_eq!(
                mixer_pane_ui::parameter_value(&bus.effects[0], parameter),
                expected,
                "typing `{typed}` writes {expected}"
            );
        }
    }

    /// AU2 §6.7: the value-aware parser covers every unit, not just the two
    /// that round — so a formatter that becomes lossy later cannot reopen the
    /// hole silently.
    #[test]
    fn every_readout_reads_its_own_rendering_back_unchanged() {
        for name in mixer_pane_ui::INSERTABLE_AUDIO_EFFECTS {
            let descriptor = kinewright_core::effect_descriptor(name).expect("registered");
            for parameter in descriptor.parameters {
                if parameter.name == "bypass" {
                    continue;
                }
                let unit = mixer_pane_ui::mixer_unit(parameter.name);
                if unit == MixerUnit::Flag {
                    continue;
                }
                // Every value in the range, not a sample: only 137 of the
                // 1 991 Q values and 17 of the 191 one-decimal ratios scale to
                // a product below their integer, so a sparse sweep misses the
                // class entirely. A stride of 7 on the wide frequency ranges
                // keeps the walk quick and is coprime with 10 and 100, so it
                // still visits every decimal pattern.
                let stride = if parameter.max - parameter.min > 4_000 {
                    7
                } else {
                    1
                };
                for value in (parameter.min..=parameter.max).step_by(stride) {
                    #[allow(clippy::cast_precision_loss)]
                    let shown = mixer_pane_ui::format_mixer_unit(unit, value as f64);
                    #[allow(clippy::cast_precision_loss)]
                    let expected = value as f64;
                    assert_eq!(
                        mixer_pane_ui::parse_mixer_unit_from(unit, &shown, value),
                        Some(expected),
                        "{name}.{} shows `{shown}` at {value} and must read it back as itself",
                        parameter.name
                    );
                    // The plain parser cannot recover a rounded rendering —
                    // `1.0 kHz` means 1 000 whatever it was rendered from —
                    // but whatever it returns must land exactly on an integer,
                    // because egui truncates it into the `i64`.
                    let plain = mixer_pane_ui::parse_mixer_unit(unit, &shown)
                        .expect("the control's own rendering parses");
                    assert!(
                        (plain.fract()).abs() < f64::EPSILON,
                        "{name}.{} parsed `{shown}` as {plain}, which truncates one unit low",
                        parameter.name
                    );
                }
            }
        }
        // Only the two lossy units need it; the others are exact either way.
        assert_eq!(
            mixer_pane_ui::parse_mixer_unit(MixerUnit::Hertz, "1.2 kHz"),
            Some(1_200.0),
            "the plain parser still reads the spelling at face value"
        );
        assert_eq!(
            mixer_pane_ui::parse_mixer_unit_from(MixerUnit::Hertz, "1.2 kHz", 1_250),
            Some(1_250.0)
        );
        assert_eq!(
            mixer_pane_ui::parse_mixer_unit_from(MixerUnit::Hertz, "1.2 kHz", 900),
            Some(1_200.0),
            "text that is not what the control is showing is a real edit"
        );
        assert_eq!(
            mixer_pane_ui::parse_mixer_unit_from(MixerUnit::Decibels, "loud", 35),
            None,
            "a bad entry still leaves the value alone"
        );
    }

    /// AU2 §7 B21 and §6.7's unit table: every chain readout reads back what
    /// it wrote, in the spelling it wrote it.
    #[test]
    fn a_typed_chain_readout_round_trips_in_its_own_unit() {
        assert_eq!(
            mixer_pane_ui::format_mixer_unit(MixerUnit::Hertz, 1_200.0),
            "1.2 kHz"
        );
        assert_eq!(
            mixer_pane_ui::format_mixer_unit(MixerUnit::Hertz, 250.0),
            "250 Hz"
        );
        assert_eq!(
            mixer_pane_ui::format_mixer_unit(MixerUnit::Decibels, 35.0),
            "+3.5 dB"
        );
        assert_eq!(
            mixer_pane_ui::format_mixer_unit(MixerUnit::Milliseconds, 12.0),
            "12 ms"
        );
        assert_eq!(
            mixer_pane_ui::format_mixer_unit(MixerUnit::Ratio, 400.0),
            "4.0:1"
        );
        assert_eq!(
            mixer_pane_ui::format_mixer_unit(MixerUnit::Q, 71.0),
            "Q 0.71"
        );

        for (unit, text, expected) in [
            (MixerUnit::Hertz, "1.2 kHz", 1_200.0),
            (MixerUnit::Hertz, "1.2k", 1_200.0),
            (MixerUnit::Hertz, "1200 hz", 1_200.0),
            (MixerUnit::Hertz, "1200", 1_200.0),
            (MixerUnit::Hertz, "250 Hz", 250.0),
            (MixerUnit::Decibels, "+3.5 dB", 35.0),
            (MixerUnit::Decibels, "-6", -60.0),
            (MixerUnit::Milliseconds, "12 ms", 12.0),
            (MixerUnit::Milliseconds, "12", 12.0),
            (MixerUnit::Ratio, "4.0:1", 400.0),
            (MixerUnit::Ratio, "4:1", 400.0),
            (MixerUnit::Ratio, "4", 400.0),
            (MixerUnit::Q, "Q 0.71", 71.0),
            (MixerUnit::Q, "q 0.71", 71.0),
            (MixerUnit::Q, "0.71", 71.0),
            // The scaling that lands these on their integer is not exact in
            // binary — `0.57 * 100.0` is `56.999999999999993` — and egui
            // writes an `i64` by truncating, so each of these stored one unit
            // low before the parser rounded.
            (MixerUnit::Q, "Q 0.57", 57.0),
            (MixerUnit::Ratio, "2.3:1", 230.0),
            (MixerUnit::Hertz, "2.01 kHz", 2_010.0),
        ] {
            // Exactly: egui writes the parsed `f64` into an `i64` by
            // truncating, so a result a hair under the integer is a value one
            // unit low in the document.
            assert_eq!(
                mixer_pane_ui::parse_mixer_unit(unit, text),
                Some(expected),
                "{unit:?} must read `{text}` back as exactly {expected}"
            );
        }
        for unit in [
            MixerUnit::Hertz,
            MixerUnit::Milliseconds,
            MixerUnit::Ratio,
            MixerUnit::Q,
        ] {
            assert_eq!(mixer_pane_ui::parse_mixer_unit(unit, "loud"), None);
            assert_eq!(mixer_pane_ui::parse_mixer_unit(unit, ""), None);
        }

        // Round trip every displayed value of every audio parameter.
        for name in [
            "audio_gain",
            "audio_parametric_eq",
            "audio_compressor",
            "audio_gate",
            "audio_ducking",
            "audio_true_peak_limiter",
        ] {
            let descriptor = kinewright_core::effect_descriptor(name).expect("registered");
            for parameter in descriptor.parameters {
                if parameter.name == "bypass" {
                    // The header's checkbox, not a readout (AU2 §6.7).
                    continue;
                }
                let unit = mixer_pane_ui::mixer_unit(parameter.name);
                if unit == MixerUnit::Flag {
                    continue;
                }
                assert_ne!(
                    unit,
                    MixerUnit::Plain,
                    "{name}.{} has no unit",
                    parameter.name
                );
                for value in [parameter.min, parameter.neutral, parameter.max] {
                    #[allow(clippy::cast_precision_loss)]
                    let shown = mixer_pane_ui::format_mixer_unit(unit, value as f64);
                    #[allow(clippy::cast_precision_loss)]
                    let expected = value as f64;
                    // Exactly, not within half a unit: egui writes the parsed
                    // `f64` into an `i64` by truncating, so a result half a
                    // unit low is a value one unit low in the document.
                    assert_eq!(
                        mixer_pane_ui::parse_mixer_unit(unit, &shown),
                        Some(expected),
                        "{name}.{} shows `{shown}` and must read it back exactly",
                        parameter.name
                    );
                }
            }
        }
    }

    /// AU2 §6.7: one card is expanded at a time, the first in the chain by
    /// default, and a remembered card that has left the chain falls back.
    #[test]
    fn one_card_is_expanded_at_a_time() {
        let document = chain_document();
        let effects = &document.audio_mix.buses[0].effects;
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(
                mixer_pane_ui::expanded_card(ui, MixerSelection::Bus(AudioBusId(1)), effects),
                Some(EffectId(1)),
                "the first card of the chain is expanded by default"
            );
            assert_eq!(
                mixer_pane_ui::expanded_card(ui, MixerSelection::Bus(AudioBusId(1)), &[]),
                None,
                "an empty chain expands nothing"
            );
        });

        let mut harness =
            MixerHarness::new(chain_document()).editing(MixerSelection::Bus(AudioBusId(1)));
        let _ = harness.frame(Vec::new());
        assert!(
            harness.has("param:1:gain_tenth_db"),
            "the first card is open: {:?}",
            harness.rects
        );
        assert!(
            !harness.has("param:3:threshold_tenth_db"),
            "the others are collapsed"
        );
        let header = harness.rect("card:3");
        let _ = harness.frame(vec![moved(header.center()), pointer(header.center(), true)]);
        let clicked = harness.frame(vec![pointer(header.center(), false)]);
        assert!(
            clicked.operations().is_empty(),
            "expanding a card is not a document edit"
        );
        let _ = harness.frame(Vec::new());
        assert!(
            harness.has("param:3:threshold_tenth_db"),
            "clicking a collapsed header expands it: {:?}",
            harness.rects
        );
        assert!(
            !harness.has("param:1:gain_tenth_db"),
            "and collapses the one that was open"
        );
    }

    /// AU2 §6.5 and §6.6: the `Edit` toggles drive the selection, and a
    /// selection naming a bus the document has lost is cleared.
    #[test]
    fn the_edit_toggles_open_and_close_the_chain_pane() {
        let mut harness = MixerHarness::new(mixer_document());
        let _ = harness.frame(Vec::new());
        assert_eq!(harness.selection, None);

        let edit = harness.rect("bus_edit:1");
        let _ = harness.frame(vec![moved(edit.center()), pointer(edit.center(), true)]);
        let _ = harness.frame(vec![pointer(edit.center(), false)]);
        assert_eq!(harness.selection, Some(MixerSelection::Bus(AudioBusId(1))));

        let master = harness.rect("master_edit");
        let _ = harness.frame(vec![moved(master.center()), pointer(master.center(), true)]);
        let _ = harness.frame(vec![pointer(master.center(), false)]);
        assert_eq!(
            harness.selection,
            Some(MixerSelection::Master),
            "selecting a second chain replaces the selection"
        );

        // The bus the pane names leaves the document.
        let mut harness =
            MixerHarness::new(mixer_document()).editing(MixerSelection::Bus(AudioBusId(1)));
        let _ = harness.frame(Vec::new());
        assert!(harness.selection.is_some());
        harness.document.audio_mix.buses.clear();
        let _ = harness.frame(Vec::new());
        assert_eq!(
            harness.selection, None,
            "a selection whose bus is gone is cleared"
        );
    }

    /// AU2 §6.6: the coalesce keys are exactly the two the contract names,
    /// and neither collides with AU1's per-track key.
    #[test]
    fn the_chain_coalesce_keys_are_one_per_chain() {
        assert_eq!(audio_bus_coalesce_key(AudioBusId(1)), "audio_bus:1");
        assert_eq!(AUDIO_MASTER_COALESCE_KEY, "audio_master");
        assert_eq!(
            MixerSelection::Bus(AudioBusId(7)).coalesce_key(),
            "audio_bus:7"
        );
        assert_eq!(MixerSelection::Master.coalesce_key(), "audio_master");
        assert_ne!(
            audio_bus_coalesce_key(AudioBusId(3)),
            track_mix_coalesce_key(TrackId(3)),
            "a bus and a track of the same number are different undo entries"
        );
        assert_eq!(
            MixerSelection::Bus(AudioBusId(2)).chain(),
            AudioChain::Bus(AudioBusId(2))
        );
        assert_eq!(MixerSelection::Master.chain(), AudioChain::Master);
    }

    /// AU2 §6.5: the strip states the node count and keeps the chain in its
    /// tooltip, because a `→`-joined chain wraps a 72 px column to ribbons.
    #[test]
    fn the_node_count_replaces_the_chain_text() {
        assert_eq!(node_count_label(0), "0 nodes");
        assert_eq!(node_count_label(1), "1 node");
        assert_eq!(node_count_label(4), "4 nodes");
        assert_eq!(chain_tooltip(&[]), "No effects on this chain.");
        assert_eq!(
            chain_tooltip(&chain_effects(&["audio_gain", "audio_true_peak_limiter"])),
            "Gain → True-peak limiter"
        );
    }

    /// AU2 §7 B21: DESIGN.md's Mixer section states the AU2 rules and no
    /// longer describes the bus strips as read-only.
    ///
    /// The section still contains the words "read-only" — §6.9's own
    /// replacement paragraph calls the EQ well one — so the pin is on the
    /// sentence that was removed, not on the phrase.
    #[test]
    fn the_design_note_states_the_new_mixer_rules() {
        const DESIGN: &str = include_str!("../../../docs/DESIGN.md");
        let mixer = DESIGN
            .split_once("### Mixer")
            .expect("DESIGN.md has a Mixer section")
            .1
            .split_once("\n### ")
            .expect("the Mixer section ends at the next heading")
            .0;
        // The file is hard-wrapped, so each phrase is one that fits a line.
        for expected in [
            "a node count with the full chain as its",
            "`+ Bus` button, disabled with its",
            "`Edit` opens the chain pane beside the strips",
            "One card is expanded at a time",
            "read-only magnitude well on a parametric EQ",
            "in the product that fills from the right",
        ] {
            assert!(
                mixer.contains(expected),
                "DESIGN.md's Mixer section must state: {expected}"
            );
        }
        for token in [
            "size-mixer-chain-pane-width",
            "size-mixer-eq-curve-height",
            "size-mixer-reduction-meter-height",
        ] {
            assert!(
                DESIGN.contains(token),
                "DESIGN.md must list the token `{token}`"
            );
        }
        assert!(
            !mixer.contains("Bus strips are read-only"),
            "AU2 delivers the controls that sentence apologised for"
        );
        assert!(
            !mixer.contains("Bus controls arrive with AU2"),
            "the strip no longer says so either"
        );

        // AU3 §4.6 and §7 A19: the LOUDNESS paragraph, the measured master
        // figure, and the new token.
        for expected in [
            "The master pane opens with a `LOUDNESS` section",
            "horizontal bars over −40…0 LUFS",
            "a `Reset` that",
            "restarts integration",
            "what has been heard, not what has",
            "success inside the",
            "target's tolerance, warning above it",
            "is never a failure",
            "integrated figure as one micro",
            "Monitoring is not delivery: playback is",
            "never normalised",
            "and 210 for the master",
        ] {
            assert!(
                mixer.contains(expected),
                "DESIGN.md's Mixer section must state: {expected}"
            );
        }
        assert!(
            !mixer.contains("196 for the master"),
            "the master figure is the measured one, with the integrated line"
        );
        assert!(
            DESIGN.contains("size-mixer-loudness-bar-height"),
            "DESIGN.md must list the token `size-mixer-loudness-bar-height`"
        );
    }

    /// AU3 §4.4 F12/F20: the bars and the export read one table. The Mixer
    /// builds its target from `export_delivery_profile(delivery_aspect)`, so
    /// that mapping is pinned here rather than assumed by every test that
    /// passes a target by hand.
    #[test]
    fn the_mixer_target_follows_the_export_dialog_aspect() {
        assert_eq!(
            export_delivery_profile(None).loudness_target(),
            EBU_R128_PROGRAMME_TARGET,
            "a master export monitors against the programme target"
        );
        for aspect in DeliveryAspect::ALL {
            assert_eq!(
                export_delivery_profile(Some(aspect)).loudness_target(),
                STREAMING_PLATFORM_TARGET,
                "{aspect:?} delivers to a platform, so the bars follow the platform target"
            );
        }
    }

    /// AU3 §7 A19: the docs that describe the Part A surface say what it does.
    /// Pinned as hard-wrapped phrases, as the DESIGN.md test does.
    #[test]
    fn the_au3_part_a_docs_describe_the_mixer_loudness_section() {
        const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
        const MEDIA_POLICY: &str = include_str!("../../../docs/MEDIA-POLICY.md");
        const AU1: &str = include_str!("../../../docs/AU1-MANUAL-MIX.md");
        const AU2: &str = include_str!("../../../docs/AU2-EQ-AND-DYNAMICS.md");
        const M36: &str = include_str!("../../../docs/M36-AGENT-RUNTIME-EFFICIENCY.md");
        assert!(
            CHANGELOG.contains("The Mixer's master pane shows a `LOUDNESS` section fed by"),
            "CHANGELOG.md names the LOUDNESS section"
        );
        assert!(
            CHANGELOG.contains("published by audible position"),
            "CHANGELOG.md says the live meter publishes by audible position"
        );
        assert!(
            MEDIA_POLICY.contains("hands each chunk to an observer"),
            "MEDIA-POLICY.md carries the observer paragraph"
        );
        assert!(
            AU1.contains("docs/AU3-LOUDNESS-AND-DELIVERY.md"),
            "AU1's deferral list points at the AU3 contract"
        );
        assert!(
            AU2.contains("(AU3 §3.6 resolved this by pinning"),
            "AU2's true-peak deferral points at its AU3 resolution"
        );
        assert!(
            AU2.contains("(AU3 §3.8 delivered it for"),
            "AU2's per-chunk accumulator deferral points at its AU3 delivery"
        );
        assert!(
            M36.contains("(2026-09-09, after AU3 Part A)"),
            "M36 carries the Part A capability-budget rows"
        );
        assert!(
            M36.contains("| 130 |"),
            "M36's internal registry row still counts 130 capabilities"
        );
    }

    /// AU2 §6.7: the move buttons are `▲` and `▼`, so the installed font
    /// stack has to carry them — a missing glyph would ship two tofu boxes in
    /// the one row a card is allowed.
    #[test]
    fn the_card_move_buttons_have_glyphs_in_the_house_fonts() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        // Fonts land on the first pass.
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("");
        });
        let font = egui::FontId::new(type_size::BODY, egui::FontFamily::Proportional);
        for glyph in ["▲", "▼"] {
            assert!(
                ctx.fonts_mut(|fonts| fonts.has_glyphs(&font, glyph)),
                "the proportional family must carry `{glyph}`"
            );
        }
    }

    /// AU2 §6.10: the limits the UI itself has to state, it states.
    #[test]
    fn the_mixer_states_the_limits_it_must() {
        assert_eq!(
            mixer_pane_ui::insertion_warning("audio_ducking"),
            "Inserts a working 12 dB duck below -30 dBFS.",
            "Ducking's neutrals are a working duck, not an identity"
        );
        assert!(
            mixer_pane_ui::insertion_warning("audio_true_peak_limiter")
                .contains("stops playback once"),
            "a node that declares lookahead takes the stop-and-re-cue path"
        );
        for identity in [
            "audio_gain",
            "audio_parametric_eq",
            "audio_compressor",
            "audio_gate",
        ] {
            assert!(
                mixer_pane_ui::insertion_warning(identity).contains("exact identity"),
                "{identity} inserts as an identity and plays through"
            );
        }
        assert_eq!(mixer_pane_ui::EQ_WELL_TOOLTIP, "Magnitude at 48 kHz.");
        assert_eq!(mixer_pane_ui::EQ_WELL_SAMPLE_RATE, 48_000);
    }

    /// AU2 §6.9: the three new tokens resolve, and the pane is wide enough to
    /// hold a wrapped row of controls beside a 72 px strip.
    #[test]
    fn the_new_mixer_tokens_resolve() {
        assert!((size::MIXER_CHAIN_PANE_WIDTH - 400.0).abs() < f32::EPSILON);
        assert!((size::MIXER_EQ_CURVE_HEIGHT - 96.0).abs() < f32::EPSILON);
        assert!((size::MIXER_REDUCTION_METER_HEIGHT - 6.0).abs() < f32::EPSILON);
        // AU3 §7 A19.
        assert!((size::MIXER_LOUDNESS_BAR_HEIGHT - 6.0).abs() < f32::EPSILON);
        assert!((MIXER_REDUCTION_METER_RANGE_DB - 24.0).abs() < f32::EPSILON);
        const {
            assert!(size::MIXER_CHAIN_PANE_WIDTH > size::MIXER_STRIP_WIDTH * 4.0);
        }
    }

    /// AU3 §4.4 and §7 A18: the snapshot is polled while paused. Peaks read
    /// silence on a stopped transport, as AU1 §5.1 says, but the loudness
    /// figures are what has been heard and stay on screen frozen (F15).
    #[test]
    fn the_loudness_snapshot_is_polled_while_paused() {
        let playback = FixedLoudnessPlayback::new(measured_loudness());

        let paused = mixer_telemetry(&playback, false);
        assert_eq!(
            paused.peaks,
            MixPeaks::default(),
            "a paused transport is silent"
        );
        assert_eq!(
            paused.loudness,
            measured_loudness(),
            "the frozen figures show"
        );
        assert_eq!(
            playback.peak_reads.load(Ordering::SeqCst),
            0,
            "peaks are not read while paused"
        );

        let playing = mixer_telemetry(&playback, true);
        assert!(
            playing
                .peaks
                .master
                .iter()
                .zip([0.5, 0.25])
                .all(|(read, fed)| (read - fed).abs() < f32::EPSILON),
            "a playing transport reads the engine's peaks: {:?}",
            playing.peaks.master
        );
        assert_eq!(playing.loudness, measured_loudness());
        assert_eq!(playback.peak_reads.load(Ordering::SeqCst), 1);
        assert_eq!(
            playback.resets.load(Ordering::SeqCst),
            0,
            "reading never resets"
        );
    }

    /// AU3 §4.4 and §7 A18: the master pane paints `LOUDNESS`, `M`, `S`,
    /// `Reset`, the readout line, and the Part A F21 sentence; the master strip
    /// beside it carries the integrated figure.
    #[test]
    fn the_master_pane_paints_the_loudness_section() {
        let painted = painted_mixer_with_loudness(
            &chain_document_with_master(),
            Some(MixerSelection::Master),
            measured_loudness(),
        );
        for expected in [
            "LOUDNESS",
            "M",
            "S",
            "-18.3",
            "-17.1",
            "I -16.0 LUFS · LRA 6.2 LU · TP -1.3 dBTP · 0:42",
            "Reset",
            LOUDNESS_MONITORING_NOTE,
            "I -16.0",
        ] {
            assert!(
                painted.iter().any(|text| text == expected),
                "the master pane paints {expected:?}; it painted {painted:?}"
            );
        }
        assert!(
            !LOUDNESS_MONITORING_NOTE.contains("export step"),
            "Part A carries only the monitoring half of the F21 sentence"
        );

        let bus_pane = painted_mixer_with_loudness(
            &chain_document_with_master(),
            Some(MixerSelection::Bus(AudioBusId(1))),
            measured_loudness(),
        );
        assert!(
            !bus_pane.iter().any(|text| text == "LOUDNESS"),
            "a bus pane has no LOUDNESS section"
        );
    }

    /// AU3 §4.5 and §7 A18: the master strip paints `I —` before the meter has
    /// an integrated figure, and the figure once it has.
    #[test]
    fn the_master_strip_carries_the_integrated_line() {
        let painted = painted_mixer_with(&mixer_document(), None);
        assert!(
            painted.iter().any(|text| text == "I —"),
            "the strip says `I —` with nothing measured; it painted {painted:?}"
        );
        assert!(
            !painted.iter().any(|text| text == "LOUDNESS"),
            "the section itself lives in the pane, not the strip"
        );
        let painted = painted_mixer_with_loudness(&mixer_document(), None, measured_loudness());
        assert!(
            painted.iter().any(|text| text == "I -16.0"),
            "the strip carries the integrated figure; it painted {painted:?}"
        );
    }

    /// AU3 §4.4: `Reset` asks the engine to restart integration and pushes no
    /// operation — the meter is telemetry, not document state — and leaves the
    /// selection where it was.
    #[test]
    fn the_loudness_reset_reaches_the_engine_and_writes_nothing() {
        let mut harness =
            MixerHarness::new(chain_document_with_master()).editing(MixerSelection::Master);
        harness.loudness = measured_loudness();
        let _ = harness.frame(Vec::new());
        assert!(!harness.reset_loudness, "nothing was clicked yet");
        let before = harness.document.clone();

        let reset = harness.rect("reset_loudness");
        let _ = harness.frame(vec![moved(reset.center()), pointer(reset.center(), true)]);
        let edits = harness.frame(vec![pointer(reset.center(), false)]);
        assert!(
            harness.reset_loudness,
            "the click is reported to the caller"
        );
        assert!(
            edits.operations().is_empty(),
            "a loudness reset writes no operation: {:?}",
            edits.operations()
        );
        assert_eq!(harness.document, before, "and the document is untouched");
        assert_eq!(harness.selection, Some(MixerSelection::Master));

        // `mixer_panel` turns this flag into one `Playback::reset_loudness()`;
        // that two-line seam is not reachable from a test, because
        // `KinewrightApp::new` needs a live GPU media engine. What is proved
        // here is that the flag rises exactly on the click and falls the next
        // frame, and that no `Operation` is emitted on any of the three frames.

        let _ = harness.frame(Vec::new());
        assert!(
            !harness.reset_loudness,
            "the flag is one frame's answer, not a latch"
        );
    }
}

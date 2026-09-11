//! The mixer's chain pane: routing, cards, and the `+ Effect` menu (AU2 §6.6).
//!
//! No floating window. DESIGN.md's dock rule still applies, so the pane is a
//! fixed column inside the Mixer tab beside the strips, with its own vertical
//! scroll area.
//!
//! Nothing here pushes an operation. Every control mutates the edited copy of
//! its chain in [`MixerChainEdits`], which [`crate::mixer_ui::mixer_body`]
//! folds into exactly one `UpsertAudioBus` per touched bus and at most one
//! `SetAudioMaster` after the whole frame has painted (AU2 §6.6).

use std::collections::BTreeMap;

use eframe::egui;
use kinewright_core::{
    AUDIO_BUS_GAIN_MAX, AUDIO_BUS_GAIN_MIN, AUDIO_MASTER_GAIN_MAX, AUDIO_MASTER_GAIN_MIN, AudioBus,
    AudioChain, AudioMaster, AutomationCurve, CHAIN_LOOKAHEAD_MILLISECONDS, Document, Effect,
    EffectId, LoudnessSnapshot, LoudnessTarget, NOISE_PROFILE_BAND_COUNT,
    NOISE_PROFILE_PARAMETER_NAMES, PROFILE_BAND_NEUTRAL_TENTH_DB, PanLaw, ParamValue,
    TRACK_AUTOMATION_PARAMETERS, TRACK_MIX_GAIN_MAX, TRACK_MIX_GAIN_MIN, TRACK_MIX_PAN_MAX,
    TRACK_MIX_PAN_MIN, TimeCode, Track, TrackId, TrackMix, chain_lookahead_milliseconds,
    effect_descriptor, has_gain_computer, is_hold_only_parameter, is_noise_profile_parameter,
    is_static_audio_parameter,
};

use crate::{
    inspector_ui::{
        CurveWrite, apply_keyframe_row_action, effect_display_name, is_live_drag, keyframe_row,
        upsert_keyframe,
    },
    mixer_ui::{
        MIXER_REDUCTION_METER_RANGE_DB, MixerChainEdits, MixerMeterLevels, MixerSelection,
        record_keyed_rect, record_param_rect, record_strip_rect, track_caption,
        track_carries_audio,
    },
    theme::{self, color, radius, size, space, type_size},
};

/// The chain a pane control is editing: the selection plus the chain's current
/// stored value, which is what a control needs to clone a copy in.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MixerChain<'a> {
    Bus(&'a AudioBus),
    Master(&'a AudioMaster),
    Track(&'a Track, &'a TrackMix),
}

impl<'a> MixerChain<'a> {
    pub(crate) const fn selection(self) -> MixerSelection {
        match self {
            Self::Bus(bus) => MixerSelection::Bus(bus.id),
            Self::Master(_) => MixerSelection::Master,
            Self::Track(track, _) => MixerSelection::Track(track.id),
        }
    }

    const fn effects(self) -> &'a [Effect] {
        match self {
            Self::Bus(bus) => bus.effects.as_slice(),
            Self::Master(master) => master.effects.as_slice(),
            Self::Track(_, _) => &[],
        }
    }

    /// The sidechain sources this chain can duck from. The master has none,
    /// which is why `audio_ducking` is rejected there outright.
    const fn sidechain(self) -> &'a [TrackId] {
        match self {
            Self::Bus(bus) => bus.ducking_sidechain_tracks.as_slice(),
            Self::Master(_) | Self::Track(_, _) => &[],
        }
    }

    /// AU4 §5.4 rule 114: this chain's fader curve, if it carries one.
    const fn gain_curve(self) -> Option<&'a AutomationCurve> {
        match self {
            Self::Bus(bus) => bus.gain_curve.as_ref(),
            Self::Master(master) => master.gain_curve.as_ref(),
            Self::Track(_, mix) => mix.gain_curve.as_ref(),
        }
    }

    /// The chain fader's parked value.
    const fn gain_tenth_db(self) -> i32 {
        match self {
            Self::Bus(bus) => bus.gain_tenth_db,
            Self::Master(master) => master.gain_tenth_db,
            Self::Track(_, mix) => mix.gain_tenth_db,
        }
    }

    /// The chain fader's legal range, which is what an automation key is
    /// clamped to before it is written.
    const fn gain_range(self) -> std::ops::RangeInclusive<i64> {
        match self {
            Self::Bus(_) => AUDIO_BUS_GAIN_MIN as i64..=AUDIO_BUS_GAIN_MAX as i64,
            Self::Master(_) => AUDIO_MASTER_GAIN_MIN as i64..=AUDIO_MASTER_GAIN_MAX as i64,
            Self::Track(_, _) => TRACK_MIX_GAIN_MIN as i64..=TRACK_MIX_GAIN_MAX as i64,
        }
    }

    /// Write this chain's fader curve into the edited copy. `None` clears it.
    fn set_gain_curve(self, edits: &mut MixerChainEdits, curve: Option<AutomationCurve>) {
        match self {
            Self::Bus(bus) => edits.bus(bus).gain_curve = curve,
            Self::Master(master) => edits.master(master).gain_curve = curve,
            Self::Track(_, mix) => edits.track(mix).gain_curve = curve,
        }
    }

    /// The edited copy of this chain's effect list.
    fn effects_mut(self, edits: &mut MixerChainEdits) -> &mut Vec<Effect> {
        match self {
            Self::Bus(bus) => &mut edits.bus(bus).effects,
            Self::Master(master) => &mut edits.master(master).effects,
            Self::Track(_, _) => {
                unreachable!("a track chain has no effect list (AU6 §6.2)")
            }
        }
    }

    /// The edited copy of one node of this chain, addressed by id rather than
    /// position so a reorder earlier in the same frame cannot retarget it.
    fn effect_mut(self, edits: &mut MixerChainEdits, effect: EffectId) -> Option<&mut Effect> {
        self.effects_mut(edits)
            .iter_mut()
            .find(|candidate| candidate.id == effect)
    }
}

/// The chain pane beside the strips (AU2 §6.6).
///
/// `snapshot` and `target` feed the master pane's `LOUDNESS` section (AU3
/// §4.4); a bus pane ignores them. Returns `true` when that section's `Reset`
/// was clicked, which the caller answers with `Playback::reset_loudness` —
/// telemetry, not an operation, so it does not travel through `edits`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn chain_pane(
    ui: &mut egui::Ui,
    document: &Document,
    selection: MixerSelection,
    levels: &MixerMeterLevels,
    snapshot: LoudnessSnapshot,
    position: TimeCode,
    target: LoudnessTarget,
    edits: &mut MixerChainEdits,
    learn: &mut NoiseLearn<'_>,
) -> bool {
    // The pane owns exactly one column. `set_max_width` alone does not hold a
    // `ScrollArea`, which sizes its viewport from the room it is offered, so
    // the column is allocated at the token's width and the scroll area is
    // given that and no more; without this the cards' wrapped control rows
    // stretch to the far edge of the window.
    let column = egui::vec2(size::MIXER_CHAIN_PANE_WIDTH, ui.available_height());
    ui.allocate_ui_with_layout(column, egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.set_min_width(size::MIXER_CHAIN_PANE_WIDTH);
        ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
        let scrolled = egui::ScrollArea::vertical()
            .id_salt("mixer-chain-pane")
            .max_width(size::MIXER_CHAIN_PANE_WIDTH)
            .show(ui, |ui| {
                ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
                match selection {
                    MixerSelection::Bus(id) => {
                        if let Some(bus) = document.audio_mix.bus(id) {
                            bus_pane(ui, document, bus, levels, position, edits, learn);
                        }
                        false
                    }
                    MixerSelection::Master => master_pane(
                        ui, document, levels, snapshot, position, target, edits, learn,
                    ),
                    MixerSelection::Track(id) => {
                        if let Some(track) = document.tracks.iter().find(|track| track.id == id) {
                            let mix = document.audio_mix.track(id);
                            track_pane(
                                ui,
                                document,
                                track,
                                &mix,
                                position,
                                document.duration,
                                edits,
                            );
                        }
                        false
                    }
                }
            });
        // The scroll is the pane's whole answer to a short dock, so its two
        // measurements are recorded for the test that proves the content
        // really does overflow a 260 px viewport instead of stretching it.
        record_strip_rect("chain_pane_viewport", scrolled.inner_rect);
        record_strip_rect(
            "chain_pane_content",
            egui::Rect::from_min_size(scrolled.inner_rect.min, scrolled.content_size),
        );
        scrolled.inner
    })
    .inner
}

#[allow(clippy::too_many_arguments)]
fn bus_pane(
    ui: &mut egui::Ui,
    document: &Document,
    bus: &AudioBus,
    levels: &MixerMeterLevels,
    position: TimeCode,
    edits: &mut MixerChainEdits,
    learn: &mut NoiseLearn<'_>,
) {
    let chain = MixerChain::Bus(bus);
    pane_title(ui, &format!("Bus: {}", bus.name));
    // AU4 §5.4 rule 112: at the top of a bus pane, below `LOUDNESS` on the
    // master pane.
    automation_section(ui, chain, position, document.duration, edits);
    routing_rows(ui, document, bus, edits);
    sidechain_rows(ui, document, bus, edits);
    ui.separator();
    chain_cards(ui, chain, levels, position, edits, learn);
    add_effect_menu(ui, chain, edits);
}

fn track_pane(
    ui: &mut egui::Ui,
    document: &Document,
    track: &Track,
    mix: &TrackMix,
    position: TimeCode,
    duration: TimeCode,
    edits: &mut MixerChainEdits,
) {
    let chain = MixerChain::Track(track, mix);
    pane_title(ui, &format!("Track: {}", track_caption(document, track.id)));
    automation_section(ui, chain, position, duration, edits);
}

/// The master pane: the `LOUDNESS` section first, then the pan law and the
/// chain (AU3 §4.4). Returns whether `Reset` was clicked.
#[allow(clippy::too_many_arguments)]
fn master_pane(
    ui: &mut egui::Ui,
    document: &Document,
    levels: &MixerMeterLevels,
    snapshot: LoudnessSnapshot,
    position: TimeCode,
    target: LoudnessTarget,
    edits: &mut MixerChainEdits,
    learn: &mut NoiseLearn<'_>,
) -> bool {
    let master = &document.audio_mix.master;
    let chain = MixerChain::Master(master);
    pane_title(ui, "Master");
    let reset_loudness = loudness_section(ui, snapshot, target);
    automation_section(ui, chain, position, document.duration, edits);
    pan_law_rows(ui, document.audio_mix.pan_law, edits);
    ui.separator();
    chain_cards(ui, chain, levels, position, edits, learn);
    add_effect_menu(ui, chain, edits);
    reset_loudness
}

fn pane_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .font(theme::semibold(type_size::BODY))
            .color(color::TEXT_PRIMARY),
    );
}

/// Why a routing checkbox refuses to clear the last track of a bus
/// (`OpError::InvalidAudioBus`).
/// The wording is the app's, not core's: `OpError::InvalidAudioBus` tells a
/// caller "remove the bus instead", and the pane carries no bus-removal
/// control, so a human reading that would be at a dead end. The chat pane is
/// the route a human has, and it is on screen beside the mixer.
pub(crate) const LAST_TRACK_REASON: &str =
    "a bus keeps at least one track; ask the agent to remove the bus instead";
/// Why a sidechain checkbox refuses to clear the last sidechain of a bus that
/// carries `audio_ducking` (`OpError::AudioBusDuckingWithoutSidechain`).
pub(crate) const LAST_SIDECHAIN_REASON: &str = "this bus's ducking node needs a sidechain track";

/// Why one routing checkbox refuses to move, if it does (AU2 §6.8).
///
/// A toggle that would produce a rejected operation is disabled with the
/// reason rather than allowed to fail.
pub(crate) fn routing_block(document: &Document, bus: &AudioBus, track: TrackId) -> Option<String> {
    if let Some(other) = document
        .audio_mix
        .bus_for_track(track)
        .filter(|id| *id != bus.id)
    {
        return Some(document.audio_mix.bus(other).map_or_else(
            || format!("already on bus {other}"),
            |other| format!("already on {}", other.name),
        ));
    }
    if bus.tracks.contains(&track) && bus.tracks.len() == 1 {
        return Some(LAST_TRACK_REASON.to_owned());
    }
    None
}

/// Why one sidechain checkbox refuses to move, if it does (AU2 §6.8).
pub(crate) fn sidechain_block(bus: &AudioBus, track: TrackId) -> Option<&'static str> {
    let ducks = bus
        .effects
        .iter()
        .any(|effect| effect.name == "audio_ducking");
    let last =
        bus.ducking_sidechain_tracks.len() == 1 && bus.ducking_sidechain_tracks.contains(&track);
    (ducks && last).then_some(LAST_SIDECHAIN_REASON)
}

/// One routing checkbox per audio-bearing document track, plus any track
/// already routed here so it can be moved off (AU2 §6.6).
fn routing_rows(
    ui: &mut egui::Ui,
    document: &Document,
    bus: &AudioBus,
    edits: &mut MixerChainEdits,
) {
    ui.label(
        egui::RichText::new("Tracks")
            .font(theme::medium(type_size::MICRO))
            .color(color::TEXT_MUTED),
    );
    ui.horizontal_wrapped(|ui| {
        for track in document
            .tracks
            .iter()
            .filter(|track| track_carries_audio(document, track) || bus.tracks.contains(&track.id))
        {
            routing_checkbox(ui, document, bus, track.id, edits);
        }
    });
}

fn routing_checkbox(
    ui: &mut egui::Ui,
    document: &Document,
    bus: &AudioBus,
    track: TrackId,
    edits: &mut MixerChainEdits,
) {
    let on_this_bus = bus.tracks.contains(&track);
    let elsewhere = document
        .audio_mix
        .bus_for_track(track)
        .is_some_and(|id| id != bus.id);
    let reason = routing_block(document, bus, track);
    // A track claimed by another bus is shown checked and disabled with that
    // bus's name: it is routed, just not here.
    let mut checked = on_this_bus || elsewhere;
    let response = ui.add_enabled(
        reason.is_none(),
        egui::Checkbox::new(&mut checked, track_caption(document, track)),
    );
    record_keyed_rect("route", track.0, response.rect);
    if let Some(reason) = reason {
        response.on_disabled_hover_text(reason);
        return;
    }
    if response.changed() {
        let tracks = &mut edits.bus(bus).tracks;
        if checked {
            tracks.push(track);
        } else {
            tracks.retain(|candidate| *candidate != track);
        }
    }
}

/// One sidechain checkbox per document track (AU2 §6.6).
///
/// Every track, not only the audio-bearing ones: a sidechain source is often a
/// track whose media has not arrived yet, and the tap is set up in advance.
fn sidechain_rows(
    ui: &mut egui::Ui,
    document: &Document,
    bus: &AudioBus,
    edits: &mut MixerChainEdits,
) {
    ui.label(
        egui::RichText::new("Sidechain")
            .font(theme::medium(type_size::MICRO))
            .color(color::TEXT_MUTED),
    );
    ui.horizontal_wrapped(|ui| {
        for track in &document.tracks {
            sidechain_checkbox(ui, document, bus, track.id, edits);
        }
    });
}

fn sidechain_checkbox(
    ui: &mut egui::Ui,
    document: &Document,
    bus: &AudioBus,
    track: TrackId,
    edits: &mut MixerChainEdits,
) {
    let mut checked = bus.ducking_sidechain_tracks.contains(&track);
    let reason = sidechain_block(bus, track);
    let response = ui.add_enabled(
        reason.is_none(),
        egui::Checkbox::new(&mut checked, track_caption(document, track)),
    );
    record_keyed_rect("sidechain", track.0, response.rect);
    if let Some(reason) = reason {
        response.on_disabled_hover_text(reason);
        return;
    }
    if response.changed() {
        let sidechain = &mut edits.bus(bus).ducking_sidechain_tracks;
        if checked {
            sidechain.push(track);
        } else {
            sidechain.retain(|candidate| *candidate != track);
        }
    }
}

/// The document-level pan law, on the master pane (AU2 §6.6).
fn pan_law_rows(ui: &mut egui::Ui, law: PanLaw, edits: &mut MixerChainEdits) {
    ui.label(
        egui::RichText::new("Pan law")
            .font(theme::medium(type_size::MICRO))
            .color(color::TEXT_MUTED),
    );
    ui.horizontal(|ui| {
        for (candidate, label, name) in [
            (PanLaw::Balance, "Balance", "pan_law:balance"),
            (
                PanLaw::ConstantPower,
                "Constant power",
                "pan_law:constant_power",
            ),
        ] {
            let response = ui.selectable_label(law == candidate, label);
            record_strip_rect(name, response.rect);
            if response.clicked() && law != candidate {
                edits.set_pan_law(candidate);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Loudness (AU3 §4.4)
// ---------------------------------------------------------------------------

/// The lower edge of the momentary and short-term bars, in LUFS hundredths.
/// The bars run −40…0 LUFS; anything quieter reads as empty.
const LOUDNESS_BAR_FLOOR_HUNDREDTHS: i32 = -4_000;
/// Width of the `M` / `S` label at the left of a loudness bar row.
const LOUDNESS_BAR_LABEL_WIDTH: f32 = 16.0;
/// Width of the numeric readout at the right of a loudness bar row.
const LOUDNESS_BAR_READOUT_WIDTH: f32 = 56.0;
/// The dash a readout shows for a value the meter has not measured yet:
/// momentary under 400 ms, short-term under 3 s, integrated on silence, range
/// under two gated windows, true peak on digital silence.
pub(crate) const LOUDNESS_NONE: &str = "—";
/// The muted sentence that closes the section (AU3 §4.4, F14/F21).
///
/// Both halves since Part B: the operator hears the master as they mixed it,
/// and the file is brought to the target by the export step's own checkbox —
/// so a bar sitting away from the target is never a reason to touch the mix.
pub(crate) const LOUDNESS_MONITORING_NOTE: &str = "monitoring is not delivery: playback is never \
     normalised; the export step normalises the file";

/// The master pane's `LOUDNESS` section (AU3 §4.4): momentary and short-term
/// as bars against the export dialog's current profile target, then one line
/// of integrated, range, true peak, and programme time, then `Reset`.
///
/// Returns `true` when `Reset` was clicked. The caller answers with
/// `Playback::reset_loudness`; no operation is pushed, because the meter is
/// telemetry and not document state.
pub(crate) fn loudness_section(
    ui: &mut egui::Ui,
    snapshot: LoudnessSnapshot,
    target: LoudnessTarget,
) -> bool {
    ui.scope(|ui| {
        // The section is one readout, not a stack of controls: its rows sit
        // as close as a strip's do, and the readout row is as tall as its
        // small button rather than a full control height. This is what keeps
        // the section inside its 80 px budget (AU3 §4.4).
        ui.spacing_mut().item_spacing.y = space::HALF;
        ui.spacing_mut().interact_size.y = size::ICON_SM;
        ui.label(theme::caps_label("LOUDNESS", color::TEXT_MUTED));
        loudness_bar_row(ui, "M", snapshot.momentary_lufs_hundredths, target);
        loudness_bar_row(ui, "S", snapshot.short_term_lufs_hundredths, target);
        let reset = ui
            .horizontal(|ui| {
                loudness_readout_label(ui, snapshot, target);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let response = ui.small_button("Reset");
                    record_strip_rect("reset_loudness", response.rect);
                    response.clicked()
                })
                .inner
            })
            .inner;
        ui.add(
            egui::Label::new(
                egui::RichText::new(LOUDNESS_MONITORING_NOTE)
                    .font(theme::medium(type_size::MICRO))
                    .color(color::TEXT_MUTED),
            )
            .wrap(),
        );
        reset
    })
    .inner
}

/// One bar row: a `MICRO` label, the bar, and a `MICRO` readout, painted into
/// one allocation exactly a text row tall.
fn loudness_bar_row(ui: &mut egui::Ui, label: &str, lufs: Option<i32>, target: LoudnessTarget) {
    let font = theme::medium(type_size::MICRO);
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(row);
    painter.text(
        row.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        font.clone(),
        color::TEXT_MUTED,
    );
    let bar_left = row.left() + LOUDNESS_BAR_LABEL_WIDTH;
    let bar_right = (row.right() - LOUDNESS_BAR_READOUT_WIDTH - space::ONE).max(bar_left);
    let bar_rect = egui::Rect::from_center_size(
        egui::pos2(f32::midpoint(bar_left, bar_right), row.center().y),
        egui::vec2(bar_right - bar_left, size::MIXER_LOUDNESS_BAR_HEIGHT),
    );
    painter.rect_filled(bar_rect, 1.0, color::SURFACE_ACTIVE);
    if let Some(fill_color) = loudness_bar_color(lufs, target) {
        let fill = egui::Rect::from_min_size(
            bar_rect.min,
            egui::vec2(
                bar_rect.width() * loudness_bar_fill(lufs),
                bar_rect.height(),
            ),
        );
        if fill.width() > 0.0 {
            // A status colour against the target, never the accent
            // (DESIGN.md): quiet is `text-secondary`, not a failure.
            painter.rect_filled(fill, 1.0, fill_color);
        }
    }
    painter.text(
        row.right_center(),
        egui::Align2::RIGHT_CENTER,
        loudness_value(lufs),
        font,
        color::TEXT_SECONDARY,
    );
}

/// The `I … · LRA … · TP … · m:ss` line, with `TP` in status-danger above the
/// target's ceiling.
fn loudness_readout_label(ui: &mut egui::Ui, snapshot: LoudnessSnapshot, target: LoudnessTarget) {
    let font = theme::medium(type_size::MICRO);
    let [before, true_peak, after] = loudness_readout_parts(snapshot);
    let true_peak_color = true_peak_color(snapshot.true_peak_dbtp_hundredths, target);
    let mut job = egui::text::LayoutJob::default();
    for (text, text_color) in [
        (before, color::TEXT_SECONDARY),
        (true_peak, true_peak_color),
        (after, color::TEXT_SECONDARY),
    ] {
        job.append(
            &text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: text_color,
                ..Default::default()
            },
        );
    }
    ui.label(job);
}

/// How full a momentary or short-term bar is over −40…0 LUFS: `0` for `None`,
/// clamped at both ends.
#[must_use]
pub(crate) fn loudness_bar_fill(lufs: Option<i32>) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    lufs.map_or(0.0, |lufs| {
        (lufs.saturating_sub(LOUDNESS_BAR_FLOOR_HUNDREDTHS) as f32
            / -(LOUDNESS_BAR_FLOOR_HUNDREDTHS as f32))
            .clamp(0.0, 1.0)
    })
}

/// The colour of a loudness bar's fill against the target: `text-secondary`
/// below `target − tolerance`, status-success inside the tolerance,
/// status-warning above it, and no fill at all for `None`. Never the accent.
#[must_use]
pub(crate) fn loudness_bar_color(
    lufs: Option<i32>,
    target: LoudnessTarget,
) -> Option<egui::Color32> {
    let lufs = lufs?;
    let low = target.integrated_lufs_hundredths - target.tolerance_lu_hundredths;
    let high = target.integrated_lufs_hundredths + target.tolerance_lu_hundredths;
    Some(if lufs < low {
        color::TEXT_SECONDARY
    } else if lufs > high {
        color::STATUS_WARNING
    } else {
        color::STATUS_SUCCESS
    })
}

/// The colour the `TP` figure is painted in: status-danger strictly above the
/// target's true-peak ceiling, `text-secondary` at or below it and for a peak
/// the meter has not measured. A peak exactly at the ceiling conforms.
#[must_use]
pub(crate) fn true_peak_color(dbtp: Option<i32>, target: LoudnessTarget) -> egui::Color32 {
    if dbtp.is_some_and(|dbtp| dbtp > target.true_peak_ceiling_dbtp_hundredths) {
        color::STATUS_DANGER
    } else {
        color::TEXT_SECONDARY
    }
}

/// The section's readout line: `I −16.0 LUFS · LRA 6.2 LU · TP −1.3 dBTP ·
/// 0:42`, with [`LOUDNESS_NONE`] in place of each value the meter has not
/// measured.
#[must_use]
pub(crate) fn loudness_readout(snapshot: LoudnessSnapshot) -> String {
    loudness_readout_parts(snapshot).concat()
}

/// The readout line in three pieces so the true-peak figure can carry its
/// own colour: everything before it, the figure itself, and the rest.
fn loudness_readout_parts(snapshot: LoudnessSnapshot) -> [String; 3] {
    [
        format!(
            "I {} LUFS · LRA {} LU · TP ",
            loudness_value(snapshot.integrated_lufs_hundredths),
            loudness_value(snapshot.loudness_range_lu_hundredths),
        ),
        loudness_value(snapshot.true_peak_dbtp_hundredths),
        format!(" dBTP · {}", programme_clock(snapshot.programme_seconds)),
    ]
}

/// The master strip's one-line integrated readout: `I −16.0`, or `I —`
/// (AU3 §4.5).
#[must_use]
pub(crate) fn integrated_strip_line(snapshot: LoudnessSnapshot) -> String {
    format!("I {}", loudness_value(snapshot.integrated_lufs_hundredths))
}

/// One loudness figure to a tenth (`-18.3`), or [`LOUDNESS_NONE`].
///
/// The sign comes from the whole value, as the export dialog's `decibels`
/// does, so `-5` hundredths reads `-0.1` and not `0.1`; the tenth is rounded
/// half away from zero.
#[must_use]
pub(crate) fn loudness_value(hundredths: Option<i32>) -> String {
    let Some(hundredths) = hundredths else {
        return LOUDNESS_NONE.to_owned();
    };
    let sign = if hundredths < 0 { "-" } else { "" };
    let tenths = (hundredths.unsigned_abs() + 5) / 10;
    format!("{sign}{}.{}", tenths / 10, tenths % 10)
}

/// Whole seconds as `m:ss`; the minutes are not capped.
#[must_use]
pub(crate) fn programme_clock(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

// ---------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------

/// The chain's cards, one expanded at a time (AU2 §6.7).
fn chain_cards(
    ui: &mut egui::Ui,
    chain: MixerChain,
    levels: &MixerMeterLevels,
    position: TimeCode,
    edits: &mut MixerChainEdits,
    learn: &mut NoiseLearn<'_>,
) {
    let effects = chain.effects();
    if effects.is_empty() {
        ui.colored_label(color::TEXT_MUTED, "No effects on this chain yet.");
        return;
    }
    let expanded = expanded_card(ui, chain.selection(), effects);
    for (index, effect) in effects.iter().enumerate() {
        chain_card(
            ui,
            chain,
            index,
            expanded == Some(effect.id),
            levels,
            position,
            edits,
            learn,
        );
    }
}

/// The id of the expanded-card memory slot for one chain.
fn expanded_memory_id(selection: MixerSelection) -> egui::Id {
    match selection {
        MixerSelection::Bus(bus) => egui::Id::new(("mixer-expanded-card", "bus", bus.0)),
        MixerSelection::Master => egui::Id::new(("mixer-expanded-card", "master", 0_u64)),
        MixerSelection::Track(track) => egui::Id::new(("mixer-expanded-card", "track", track.0)),
    }
}

/// Which card of this chain is expanded: the remembered one while it is still
/// in the chain, otherwise the first (AU2 §6.7).
pub(crate) fn expanded_card(
    ui: &egui::Ui,
    selection: MixerSelection,
    effects: &[Effect],
) -> Option<EffectId> {
    let stored: Option<EffectId> = ui.data(|data| data.get_temp(expanded_memory_id(selection)));
    stored
        .filter(|id| effects.iter().any(|effect| effect.id == *id))
        .or_else(|| effects.first().map(|effect| effect.id))
}

fn expand_card(ui: &egui::Ui, selection: MixerSelection, effect: EffectId) {
    ui.data_mut(|data| data.insert_temp(expanded_memory_id(selection), effect));
}

/// One chain card: a header row, and a body when it is the expanded one.
///
/// A collapsed card carries no frame, because the collapsed row's whole budget
/// is `ICON_BUTTON` and a `ui.group`'s margins and stroke cost 14 px on their
/// own (AU2 §6.7). Fourteen, not the 8 rule 131's arithmetic assumed: the
/// group and the header together measure 31.5 px (AU5 §0 R120).
///
/// The expanded card takes the frame, where the grouping is what tells a
/// wrapped row of controls from the row below it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn chain_card(
    ui: &mut egui::Ui,
    chain: MixerChain,
    index: usize,
    expanded: bool,
    levels: &MixerMeterLevels,
    position: TimeCode,
    edits: &mut MixerChainEdits,
    learn: &mut NoiseLearn<'_>,
) {
    if expanded {
        ui.group(|ui| {
            card_header(ui, chain, index, expanded, levels, edits);
            card_body(ui, chain, index, levels, position, edits, learn);
        });
    } else {
        card_header(ui, chain, index, expanded, levels, edits);
    }
}

fn card_header(
    ui: &mut egui::Ui,
    chain: MixerChain,
    index: usize,
    expanded: bool,
    levels: &MixerMeterLevels,
    edits: &mut MixerChainEdits,
) {
    let effects = chain.effects();
    let effect = &effects[index];
    let last = effects.len().saturating_sub(1);
    ui.scope(|ui| {
        // The header is one row of a stack of cards, not a section: it does
        // not owe its buttons a control's worth of padding.
        ui.spacing_mut().interact_size.y = size::ICON_SM;
        ui.spacing_mut().button_padding.y = 0.0;
        ui.spacing_mut().item_spacing.x = space::ONE;
        ui.horizontal(|ui| {
            // The tooltip says what the label cannot: the registered node
            // name and the id every operation and error message uses.
            let name = ui
                .selectable_label(expanded, effect_display_name(&effect.name))
                .on_hover_text(format!("{} · node {}", effect.name, effect.id.0));
            record_keyed_rect("card", effect.id.0, name.rect);
            if name.clicked() && !expanded {
                expand_card(ui, chain.selection(), effect.id);
            }

            // `bypass` is hold-only, so the static read and the resolved
            // read are the same value; the static one says so.
            let mut bypassed = parameter_value(effect, "bypass", TimeCode::ZERO) == 1;
            let bypass = ui.checkbox(&mut bypassed, "Bypass");
            record_keyed_rect("bypass", effect.id.0, bypass.rect);
            if bypass.changed()
                && let Some(target) = chain.effect_mut(edits, effect.id)
            {
                target.parameters.insert(
                    "bypass".to_owned(),
                    ParamValue::Integer(i64::from(bypassed)),
                );
            }

            if has_gain_computer(&effect.name) {
                let reduction = levels.reduction(
                    chain
                        .selection()
                        .chain()
                        .expect("gain reduction is painted on a bus or the master"),
                    effect.id,
                );
                ui.label(
                    egui::RichText::new(reduction_readout(reduction))
                        .font(theme::medium(type_size::MICRO))
                        .color(color::TEXT_MUTED),
                );
            }

            let up = ui.add_enabled(index > 0, egui::Button::new("▲").small());
            record_keyed_rect("up", effect.id.0, up.rect);
            if up.clicked() {
                chain.effects_mut(edits).swap(index, index - 1);
            }
            let down = ui.add_enabled(index < last, egui::Button::new("▼").small());
            record_keyed_rect("down", effect.id.0, down.rect);
            if down.clicked() {
                chain.effects_mut(edits).swap(index, index + 1);
            }
            let remove = ui.add(
                egui::Button::new(egui::RichText::new("Remove").color(color::STATUS_DANGER))
                    .small(),
            );
            record_keyed_rect("remove", effect.id.0, remove.rect);
            if remove.clicked() {
                let id = effect.id;
                chain
                    .effects_mut(edits)
                    .retain(|candidate| candidate.id != id);
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn card_body(
    ui: &mut egui::Ui,
    chain: MixerChain,
    index: usize,
    levels: &MixerMeterLevels,
    position: TimeCode,
    edits: &mut MixerChainEdits,
    learn: &mut NoiseLearn<'_>,
) {
    let effect = &chain.effects()[index];
    let Some(descriptor) = effect_descriptor(&effect.name) else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        for parameter in descriptor.parameters {
            // AU5 §6.1 rule 120: the 31 profile rows are read from the noise
            // well, never dragged — skipped by the one core predicate,
            // exactly as `bypass` is skipped by exact name.
            if parameter.name == "bypass" || is_noise_profile_parameter(parameter.name) {
                continue;
            }
            mixer_parameter_control(
                ui,
                chain,
                effect,
                parameter.name,
                &parameter_label(parameter.name),
                mixer_unit(parameter.name),
                parameter_value(effect, parameter.name, position),
                edits,
            );
        }
    });
    if let Some(magnitude) = well_magnitude_source(&effect.name) {
        eq_well(ui, effect, position, magnitude);
    }
    if effect.name == "audio_denoise" {
        noise_well(ui, effect);
        learn_row(ui, chain, effect, learn);
    }
    if has_gain_computer(&effect.name) {
        reduction_bar(
            ui,
            chain
                .selection()
                .chain()
                .expect("gain reduction is painted on a bus or the master"),
            effect.id,
            levels,
        );
    }
}

/// AU4 §5.4 rule 113: what the `AUTOMATION` section is editing.
///
/// One parameter at a time. `Fader` is the chain's own gain; a node target
/// names the node by id, so a reorder earlier in the same frame cannot
/// retarget it, exactly as `MixerChain::effect_mut` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomationTarget {
    Fader,
    Node(EffectId, &'static str),
    TrackParameter(&'static str),
}

/// AU4 §5.4 rule 112: the section's caps label.
pub(crate) const AUTOMATION_LABEL: &str = "AUTOMATION";
/// AU4 §5.4 rule 114: what the section says when the chosen parameter has no
/// curve yet.
pub(crate) const AUTOMATION_EMPTY_NOTE: &str = "No automation on this parameter yet.";
/// AU4 §5.4 rule 114: the button that seeds or extends a curve.
pub(crate) const AUTOMATION_ADD_KEY: &str = "+ Key at playhead";
/// AU4 §5.4 rule 114: the button that removes the whole curve.
pub(crate) const AUTOMATION_CLEAR: &str = "Clear";

/// How many keyframe rows the section shows before the list scrolls.
///
/// Four: the section has a 120 px budget (rule 116) and every row is one
/// `ICON_SM` control row plus the strip's own spacing.
pub(crate) const AUTOMATION_VISIBLE_ROWS: u8 = 4;

/// AU4 §5.4 rule 113: every parameter of this chain that may carry a curve.
///
/// `Fader` first, then each node's parameters in descriptor order, skipping
/// the hold-only ones (`bypass`, `detector`, `true_peak`) and the static ones
/// (both `lookahead_milliseconds` and `rms_window_milliseconds`). These are
/// `is_hold_only_parameter` and `is_static_audio_parameter`'s first app
/// readers: until now the app guessed automatability from `mixer_unit()`'s
/// name match, which does not know about either rule.
pub(crate) fn automation_targets(chain: MixerChain) -> Vec<AutomationTarget> {
    let mut targets = vec![AutomationTarget::Fader];
    if matches!(chain, MixerChain::Track(_, _)) {
        targets.push(AutomationTarget::TrackParameter(
            TRACK_AUTOMATION_PARAMETERS[1],
        ));
        return targets;
    }
    for effect in chain.effects() {
        let Some(descriptor) = effect_descriptor(&effect.name) else {
            continue;
        };
        for parameter in descriptor.parameters {
            if is_hold_only_parameter(&effect.name, parameter.name)
                || is_static_audio_parameter(&effect.name, parameter.name)
            {
                continue;
            }
            targets.push(AutomationTarget::Node(effect.id, parameter.name));
        }
    }
    targets
}

/// The label one target wears in the section's combo.
pub(crate) fn automation_target_label(chain: MixerChain, target: AutomationTarget) -> String {
    match target {
        AutomationTarget::Fader => "Fader".to_owned(),
        AutomationTarget::TrackParameter(name) => parameter_label(name),
        AutomationTarget::Node(effect, name) => {
            let node = chain
                .effects()
                .iter()
                .find(|candidate| candidate.id == effect)
                .map_or_else(
                    || format!("node {}", effect.0),
                    |effect| effect_display_name(&effect.name).to_owned(),
                );
            format!("{node} · {}", parameter_label(name))
        }
    }
}

/// AU4 §5.4 rule 114: the id key one target's keyframe rows push.
///
/// Per target, not the constant `AUTOMATION_LABEL`: `keyframe_row` derives its
/// `DragValue` and `ComboBox` ids from `(key, index)`, so a single key for
/// every parameter lets an in-flight text edit or drag state carry across a
/// change of the target combo — row 0 of the fader and row 0 of a node's
/// threshold would be the same widget. Built from the node's id and the
/// parameter's name, both of which survive a reorder.
fn automation_row_key(target: AutomationTarget) -> String {
    match target {
        AutomationTarget::Fader => "automation:fader".to_owned(),
        AutomationTarget::TrackParameter(parameter) => format!("automation:track:{parameter}"),
        AutomationTarget::Node(effect, parameter) => format!("automation:{}:{parameter}", effect.0),
    }
}

/// The curve one target carries today, if any.
fn automation_curve(chain: MixerChain<'_>, target: AutomationTarget) -> Option<&AutomationCurve> {
    match target {
        AutomationTarget::Fader => chain.gain_curve(),
        AutomationTarget::TrackParameter(_) => match chain {
            MixerChain::Track(_, mix) => mix.pan_curve.as_ref(),
            _ => None,
        },
        AutomationTarget::Node(effect, name) => chain
            .effects()
            .iter()
            .find(|candidate| candidate.id == effect)
            .and_then(|effect| effect.keyframes.get(name)),
    }
}

/// The value one target reads at the audible frame, which is what
/// `+ Key at playhead` writes.
fn automation_current_value(
    chain: MixerChain,
    target: AutomationTarget,
    position: TimeCode,
) -> i64 {
    match target {
        AutomationTarget::Fader => chain
            .gain_curve()
            .and_then(|curve| curve.value_at(position))
            .unwrap_or_else(|| i64::from(chain.gain_tenth_db())),
        AutomationTarget::TrackParameter(_) => match chain {
            MixerChain::Track(_, mix) => mix
                .pan_curve
                .as_ref()
                .and_then(|curve| curve.value_at(position))
                .unwrap_or_else(|| i64::from(mix.pan_percent)),
            _ => 0,
        },
        AutomationTarget::Node(effect, name) => chain
            .effects()
            .iter()
            .find(|candidate| candidate.id == effect)
            .map_or(0, |effect| parameter_value(effect, name, position)),
    }
}

/// The range a key of one target is clamped to before it is written.
fn automation_range(chain: MixerChain, target: AutomationTarget) -> std::ops::RangeInclusive<i64> {
    match target {
        AutomationTarget::Fader => chain.gain_range(),
        AutomationTarget::TrackParameter(_) => {
            i64::from(TRACK_MIX_PAN_MIN)..=i64::from(TRACK_MIX_PAN_MAX)
        }
        AutomationTarget::Node(effect, name) => chain
            .effects()
            .iter()
            .find(|candidate| candidate.id == effect)
            .map_or(i64::MIN..=i64::MAX, |effect| {
                mixer_parameter_range(effect, name, parameter_value(effect, name, TimeCode::ZERO))
            }),
    }
}

/// Write one target's curve into the edited chain copy. `None` clears it.
pub(crate) fn set_automation_curve(
    chain: MixerChain,
    target: AutomationTarget,
    curve: Option<AutomationCurve>,
    edits: &mut MixerChainEdits,
) {
    match target {
        AutomationTarget::Fader => chain.set_gain_curve(edits, curve),
        AutomationTarget::TrackParameter(_) => {
            if let MixerChain::Track(_, mix) = chain {
                edits.track(mix).pan_curve = curve;
            }
        }
        AutomationTarget::Node(effect, name) => {
            if let Some(node) = chain.effect_mut(edits, effect) {
                match curve {
                    Some(curve) => {
                        node.keyframes.insert(name.to_owned(), curve);
                    }
                    None => {
                        node.keyframes.remove(name);
                    }
                }
            }
        }
    }
}

/// The id of the chosen-parameter memory slot for one chain.
fn automation_memory_id(selection: MixerSelection) -> egui::Id {
    match selection {
        MixerSelection::Bus(bus) => egui::Id::new(("mixer-automation-target", "bus", bus.0)),
        MixerSelection::Master => egui::Id::new(("mixer-automation-target", "master", 0_u64)),
        MixerSelection::Track(track) => {
            egui::Id::new(("mixer-automation-target", "track", track.0))
        }
    }
}

/// AU4 §5.4 rules 112-116: the chain pane's `AUTOMATION` section.
///
/// Below `LOUDNESS` on the master pane, at the top of a bus pane, editing one
/// parameter at a time. This discharges AU2's deferral *"Automation editing UI
/// for bus and master parameters"* by surface. Full automation lanes on the
/// timeline are rejected here and deferred (§1.3): the row asks for editing
/// "in the mixer" and this is the smallest surface that is it.
///
/// Nothing here pushes an operation: `Fader` writes `AudioBus.gain_curve` /
/// `AudioMaster.gain_curve` and a node parameter writes `Effect.keyframes`,
/// both through `MixerChainEdits`' existing fold, so one frame is still one
/// `UpsertAudioBus`/`SetAudioMaster` per chain.
pub(crate) fn automation_section(
    ui: &mut egui::Ui,
    chain: MixerChain,
    position: TimeCode,
    duration: TimeCode,
    edits: &mut MixerChainEdits,
) {
    // Bus and master curves are keyed in PROJECT frames, so `duration` is the
    // bound `validate_document` checks them against.
    let last = duration.0.saturating_sub(1).max(0);
    let targets = automation_targets(chain);
    let stored: Option<AutomationTarget> =
        ui.data(|data| data.get_temp(automation_memory_id(chain.selection())));
    let mut target = stored
        .filter(|target| targets.contains(target))
        .unwrap_or(AutomationTarget::Fader);
    ui.scope(|ui| {
        // The section is a list, not a stack of sections: its rows sit as
        // close as a strip's do and are one `ICON_SM` control tall. This is
        // what keeps it inside its 120 px budget (rule 116).
        ui.spacing_mut().item_spacing.y = space::HALF;
        ui.spacing_mut().interact_size.y = size::ICON_SM;
        ui.spacing_mut().button_padding = egui::vec2(space::HALF, 0.0);
        ui.label(theme::caps_label(AUTOMATION_LABEL, color::TEXT_MUTED));
        let combo = egui::ComboBox::from_id_salt("mixer-automation-target")
            .selected_text(automation_target_label(chain, target))
            .width(size::MIXER_CHAIN_PANE_WIDTH - space::EIGHT)
            .show_ui(ui, |ui| {
                for candidate in &targets {
                    ui.selectable_value(
                        &mut target,
                        *candidate,
                        automation_target_label(chain, *candidate),
                    );
                }
            });
        if combo.response.changed() || stored != Some(target) {
            ui.data_mut(|data| data.insert_temp(automation_memory_id(chain.selection()), target));
        }

        let curve = automation_curve(chain, target).cloned();
        let range = automation_range(chain, target);
        // A fixed-height list so the section measures the same with an empty
        // curve and with a scrolled ten-key one (rule 116).
        let list_size = egui::vec2(
            ui.available_width(),
            f32::from(AUTOMATION_VISIBLE_ROWS) * (size::ICON_SM + space::HALF),
        );
        let mut pending: Option<CurveWrite> = None;
        let mut live = false;
        let mut gesture_started = false;
        ui.allocate_ui(list_size, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("mixer-automation-keys")
                .auto_shrink([false, false])
                .show(ui, |ui| match &curve {
                    None => {
                        ui.label(
                            egui::RichText::new(AUTOMATION_EMPTY_NOTE)
                                .font(theme::medium(type_size::MICRO))
                                .color(color::TEXT_MUTED),
                        );
                    }
                    Some(curve) => {
                        let row_key = automation_row_key(target);
                        for index in 0..curve.keyframes.len() {
                            let mut action = keyframe_row(ui, &row_key, curve, index);
                            gesture_started |= action.gesture_started;
                            // The row is pure and domain-free, so the
                            // project's own bound is applied here.
                            if let Some(edited) = action.edited.as_mut() {
                                edited.at = TimeCode(edited.at.0.clamp(0, last));
                            }
                            if let Some(next) =
                                apply_keyframe_row_action(curve, index, &action, range.clone())
                            {
                                live = action.live && !action.removed;
                                pending = Some(next);
                            }
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            let add = ui
                .small_button(AUTOMATION_ADD_KEY)
                .on_hover_text("Add a key at the audible frame, holding the value it has there.");
            record_strip_rect("automation_add_key", add.rect);
            if add.clicked() {
                let at = TimeCode(position.0.clamp(0, last));
                let value = automation_current_value(chain, target, at);
                pending = Some(CurveWrite::Set(upsert_keyframe(
                    curve.as_ref(),
                    at,
                    value.clamp(*range.start(), *range.end()),
                )));
                live = false;
            }
            let clear = ui
                .add_enabled(curve.is_some(), egui::Button::new(AUTOMATION_CLEAR).small())
                .on_hover_text("Remove this parameter's automation.");
            record_strip_rect("automation_clear", clear.rect);
            if clear.clicked() {
                pending = Some(CurveWrite::Clear);
                live = false;
            }
        });
        if gesture_started {
            edits.begin_gesture();
        }
        if let Some(next) = pending {
            set_automation_curve(chain, target, next.into_curve(), edits);
            edits.mark_live(live);
        }
    });
}

/// The value of one parameter at the audible frame, or its descriptor neutral.
///
/// AU4 §5.4 rule 115: resolved through `Effect::integer_parameter_at` rather
/// than `static_integer_parameter`, so a keyframed card stops displaying its
/// neutral while automation drives it. `integer_parameter_at` falls back to
/// the static value, so an un-keyframed parameter reads exactly as it did.
pub(crate) fn parameter_value(effect: &Effect, name: &str, at: TimeCode) -> i64 {
    effect.integer_parameter_at(name, at).unwrap_or_else(|| {
        effect_descriptor(&effect.name)
            .and_then(|descriptor| descriptor.parameter(name))
            .map_or(0, |parameter| parameter.neutral)
    })
}

/// Read from the node's own `EFFECT_DESCRIPTORS` entry rather than transcribed
/// here, exactly as `matte_parameter_range` does: the descriptor is what the
/// operation validates against, so a control built from anything else can
/// offer a value core will reject.
fn mixer_parameter_range(effect: &Effect, name: &str, value: i64) -> std::ops::RangeInclusive<i64> {
    let Some(parameter) =
        effect_descriptor(&effect.name).and_then(|descriptor| descriptor.parameter(name))
    else {
        debug_assert!(
            false,
            "unregistered mixer control {name} on {}",
            effect.name
        );
        return value..=value;
    };
    parameter.min..=parameter.max
}

/// Three controls to a row of the pane, derived from the pane's own token so
/// nothing here invents a width (DESIGN.md).
fn control_cell_width() -> f32 {
    (size::MIXER_CHAIN_PANE_WIDTH - space::EIGHT) / 3.0
}

/// One descriptor-driven chain control (AU2 §6.7).
///
/// Mutates the chain copy; never pushes. The no-op filter is the strip's:
/// a readout that commits on Enter reports one `changed()` frame with the
/// unchanged value, and a write that changes nothing is not an edit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mixer_parameter_control(
    ui: &mut egui::Ui,
    chain: MixerChain,
    effect: &Effect,
    name: &str,
    label: &str,
    unit: MixerUnit,
    value: i64,
    edits: &mut MixerChainEdits,
) {
    // The cell is allocated at a known size so `horizontal_wrapped` can wrap
    // between controls: a nested `Ui` of unknown width takes the whole row and
    // the wrap never fires, which ran a parametric EQ's eighteen controls off
    // the edge of the pane.
    let cells = if unit == MixerUnit::Flag { 2.0 } else { 1.0 };
    let cell = egui::vec2(cells * control_cell_width(), size::ICON_BUTTON);
    ui.allocate_ui(cell, |ui| {
        ui.spacing_mut().interact_size.y = size::ICON_SM;
        ui.spacing_mut().item_spacing.x = space::HALF;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(label)
                    .font(theme::medium(type_size::MICRO))
                    .color(color::TEXT_MUTED),
            );
            if unit == MixerUnit::Flag {
                flag_control(ui, chain, effect, name, value, edits);
                return;
            }
            let mut edited = value;
            let response = ui.add(
                egui::DragValue::new(&mut edited)
                    .range(mixer_parameter_range(effect, name, value))
                    .custom_formatter(move |value, _| format_mixer_unit(unit, value))
                    .custom_parser(move |text| parse_mixer_unit_from(unit, text, value))
                    .update_while_editing(false),
            );
            record_param_rect("param", effect.id, name, response.rect);
            if response.drag_started() {
                edits.begin_gesture();
            }
            if response.changed() && edited != value {
                if let Some(target) = chain.effect_mut(edits, effect.id) {
                    target
                        .parameters
                        .insert(name.to_owned(), ParamValue::Integer(edited));
                }
                edits.mark_live(is_live_drag(&response));
            }
        });
    });
}

/// A hold-only flag is a two-item choice, not a number on a rail (AU2 §6.7).
///
/// Only the rectangle of the option that is *not* current is recorded: it is
/// the one a test presses to flip the flag, and two rows sharing a name would
/// make the recorder ambiguous.
fn flag_control(
    ui: &mut egui::Ui,
    chain: MixerChain,
    effect: &Effect,
    name: &str,
    value: i64,
    edits: &mut MixerChainEdits,
) {
    for (flag, label) in flag_labels(name) {
        let response = ui.selectable_label(value == flag, label);
        if flag != value {
            record_param_rect("param", effect.id, name, response.rect);
        }
        if response.clicked()
            && value != flag
            && let Some(target) = chain.effect_mut(edits, effect.id)
        {
            target
                .parameters
                .insert(name.to_owned(), ParamValue::Integer(flag));
        }
    }
}

const fn flag_labels(name: &str) -> [(i64, &'static str); 2] {
    match name.as_bytes() {
        b"true_peak" => [(0, "Sample"), (1, "True peak")],
        _ => [(0, "Peak"), (1, "RMS")],
    }
}

/// The reduction readout a card header and a bar both show.
pub(crate) fn reduction_readout(reduction_db: f32) -> String {
    format!("-{reduction_db:.1} dB")
}

// ---------------------------------------------------------------------------
// Units
// ---------------------------------------------------------------------------

/// The unit one chain control reads and writes in (AU2 §6.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MixerUnit {
    /// Tenths of a decibel, shown as decibels.
    Decibels,
    Hertz,
    Milliseconds,
    /// Hundredths of a ratio, shown as `4.0:1`.
    Ratio,
    /// Hundredths of a Q, shown as `Q 0.71`.
    Q,
    /// A two-item choice rather than a number.
    Flag,
    /// A registered name in none of the units above. `audio_hum_removal`'s
    /// `harmonic_count` is the first audio parameter to reach it (AU5 §6.1
    /// rule 121); the variant keeps an unrecognised control readable instead
    /// of labelling it in a unit it is not in.
    Plain,
}

/// The unit of one audio parameter, read off its name.
///
/// Every audio descriptor spells its unit in the parameter's suffix, which is
/// the convention ROADMAP:630-633 fixed for integer document controls, so the
/// table cannot drift from the forty rows of §2.1.
pub(crate) fn mixer_unit(name: &str) -> MixerUnit {
    if matches!(name, "detector" | "true_peak") {
        MixerUnit::Flag
    } else if name.ends_with("_tenth_db") {
        MixerUnit::Decibels
    } else if name.ends_with("_hertz") {
        MixerUnit::Hertz
    } else if name.ends_with("_milliseconds") {
        MixerUnit::Milliseconds
    } else if name.ends_with("_q_hundredths") {
        MixerUnit::Q
    } else if name.ends_with("_hundredths") {
        MixerUnit::Ratio
    } else {
        MixerUnit::Plain
    }
}

/// One parameter's value in the unit it is displayed in (AU2 §6.7).
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn format_mixer_unit(unit: MixerUnit, value: f64) -> String {
    let integer = value.round() as i64;
    match unit {
        MixerUnit::Decibels => crate::mixer_ui::format_gain_db(value),
        MixerUnit::Hertz => {
            if integer >= 1_000 {
                format!("{:.1} kHz", value / 1_000.0)
            } else {
                format!("{integer} Hz")
            }
        }
        MixerUnit::Milliseconds => format!("{integer} ms"),
        MixerUnit::Ratio => format!("{:.1}:1", value / 100.0),
        MixerUnit::Q => format!("Q {:.2}", value / 100.0),
        MixerUnit::Flag | MixerUnit::Plain => format!("{integer}"),
    }
}

/// Read a control's readout back, treating the value it is already showing as
/// itself (AU2 §6.7).
///
/// `format_mixer_unit` is lossy for two of its units — `1.2 kHz` is every
/// frequency from 1 150 to 1 249, and `4.0:1` is every ratio from 395 to 404 —
/// and egui re-parses the *displayed* text whenever the readout loses focus.
/// Without this, clicking a band frequency to read it and clicking away wrote
/// a rounded value to the document and opened an undo entry, moving the band
/// by up to 500 Hz. An unchanged readout therefore parses back to the exact
/// stored value, the no-op filter holds, and only text that differs from what
/// the control is showing is read as a new value.
pub(crate) fn parse_mixer_unit_from(unit: MixerUnit, text: &str, current: i64) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    let current_value = current as f64;
    if text
        .trim()
        .eq_ignore_ascii_case(format_mixer_unit(unit, current_value).trim())
    {
        return Some(current_value);
    }
    parse_mixer_unit(unit, text)
}

/// Read a value typed into a chain control back in the unit it shows.
///
/// Case-insensitive, whitespace-trimming, and `None` on anything else, so a
/// bad entry leaves the value alone (AU2 §6.7).
///
/// The result is rounded because every audio parameter is an integer document
/// control and the scaling that gets it there is not exact in binary:
/// `0.57 * 100.0` is `56.999999999999993`, and egui writes an `i64` through
/// emath's `from_f64`, which truncates toward zero. Without the rounding,
/// typing `Q 0.57` — the very spelling the field shows for 57 — stored 56, and
/// 137 of the 1 991 Q values, 17 of the 191 one-decimal ratios, and 18 of the
/// 1 901 two-decimal kilohertz spellings landed one unit low. Decibels happen
/// to be exact across their whole range; the rounding is applied to every unit
/// anyway, because which products are exact is an accident of the scale
/// factor, not a property anything here can rely on.
pub(crate) fn parse_mixer_unit(unit: MixerUnit, text: &str) -> Option<f64> {
    let text = text.trim();
    match unit {
        MixerUnit::Decibels => crate::mixer_ui::parse_gain_db(text),
        MixerUnit::Hertz => parse_hertz(text),
        MixerUnit::Milliseconds => parse_suffixed(text, "ms", 1.0),
        MixerUnit::Ratio => parse_suffixed(text, ":1", 100.0),
        MixerUnit::Q => parse_q(text),
        MixerUnit::Flag | MixerUnit::Plain => text.parse::<f64>().ok(),
    }
    .map(f64::round)
}

fn parse_suffixed(text: &str, suffix: &str, scale: f64) -> Option<f64> {
    let lowered = text.trim().to_ascii_lowercase();
    let number = lowered.strip_suffix(suffix).unwrap_or(&lowered);
    number.trim().parse::<f64>().ok().map(|value| value * scale)
}

/// `1.2 kHz`, `1.2k`, `1200 hz`, and `1200` all mean 1 200 Hz.
fn parse_hertz(text: &str) -> Option<f64> {
    let lowered = text.trim().to_ascii_lowercase();
    let without_unit = lowered.strip_suffix("hz").unwrap_or(&lowered).trim();
    let (number, scale) = match without_unit.strip_suffix('k') {
        Some(rest) => (rest, 1_000.0),
        None => (without_unit, 1.0),
    };
    number.trim().parse::<f64>().ok().map(|value| value * scale)
}

/// `Q 0.71` and `0.71` both mean 71 hundredths.
fn parse_q(text: &str) -> Option<f64> {
    let lowered = text.trim().to_ascii_lowercase();
    let number = lowered.strip_prefix('q').unwrap_or(&lowered);
    number.trim().parse::<f64>().ok().map(|value| value * 100.0)
}

/// The label one parameter wears in a card.
pub(crate) fn parameter_label(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("band")
        && let Some(index) = rest.chars().next().filter(char::is_ascii_digit)
        && let Some(tail) = rest.get(2..)
    {
        return match tail {
            "hertz" => format!("Band {index}"),
            "gain_tenth_db" => format!("Band {index} gain"),
            _ => format!("Band {index} Q"),
        };
    }
    match name {
        "gain_tenth_db" => "Gain",
        "low_gain_tenth_db" => "Low",
        "mid_gain_tenth_db" => "Mid",
        "high_gain_tenth_db" => "High",
        "threshold_tenth_db" | "detector_threshold_tenth_db" => "Threshold",
        "ratio_hundredths" => "Ratio",
        "attack_milliseconds" => "Attack",
        "release_milliseconds" => "Release",
        "makeup_gain_tenth_db" => "Makeup",
        "knee_tenth_db" => "Knee",
        "detector" => "Detector",
        "rms_window_milliseconds" => "RMS window",
        "lookahead_milliseconds" => "Lookahead",
        "reduction_tenth_db" => "Reduction",
        "ceiling_tenth_db" => "Ceiling",
        "true_peak" => "Peak mode",
        "high_pass_hertz" => "High-pass",
        "low_shelf_hertz" => "Low shelf",
        "low_shelf_gain_tenth_db" => "Low shelf gain",
        "high_shelf_hertz" => "High shelf",
        "high_shelf_gain_tenth_db" => "High shelf gain",
        "output_gain_tenth_db" => "Output",
        "range_tenth_db" => "Range",
        "hold_milliseconds" => "Hold",
        // AU5 §6.1 rule 120. `reduction_tenth_db` and `lookahead_milliseconds`
        // already have their arms above and are shared with AU2's limiter and
        // compressor.
        "floor_offset_tenth_db" => "Floor offset",
        "smoothing_milliseconds" => "Smoothing",
        "fundamental_hertz" => "Mains",
        "harmonic_count" => "Harmonics",
        "depth_tenth_db" => "Depth",
        "notch_q_hundredths" => "Notch Q",
        "max_click_milliseconds" => "Max click",
        other => other,
    }
    .to_owned()
}

// ---------------------------------------------------------------------------
// The EQ magnitude well and the gain-reduction bar
// ---------------------------------------------------------------------------

/// Sampled points across the EQ well, the `CURVE_SAMPLES` figure the curve
/// editor uses (AU2 §6.7).
pub(crate) const EQ_WELL_SAMPLES: usize = 96;
/// The well's vertical half-range, in decibels.
pub(crate) const EQ_WELL_RANGE_DB: f64 = 24.0;
/// The well's frequency span, in hertz.
pub(crate) const EQ_WELL_MIN_HERTZ: f64 = 20.0;
/// The well's frequency span, in hertz.
pub(crate) const EQ_WELL_MAX_HERTZ: f64 = 20_000.0;
/// The rate the response is designed at. The app cannot know the device rate,
/// so it states the one it drew (AU2 §6.7).
pub(crate) const EQ_WELL_SAMPLE_RATE: u32 = 48_000;
/// A device not running at 48 kHz gets different coefficients, so the well
/// says which rate it drew (AU2 §6.10).
pub(crate) const EQ_WELL_TOOLTIP: &str = "Magnitude at 48 kHz.";

/// The frequency of one sample of the well, log-spaced 20 Hz to 20 kHz.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn eq_well_hertz(index: usize) -> f64 {
    let span = (EQ_WELL_SAMPLES - 1) as f64;
    EQ_WELL_MIN_HERTZ * (EQ_WELL_MAX_HERTZ / EQ_WELL_MIN_HERTZ).powf(index as f64 / span)
}

/// AU5 §6.2 rule 124: the one thing the well is parameterised by.
///
/// `parametric_eq_magnitude_db` and `hum_removal_magnitude_db` are the two
/// analytic responses the media crate publishes, and they share this shape
/// deliberately, so the comb is `eq_well` whole rather than a second chart.
pub(crate) type MagnitudeSource = fn(&Effect, TimeCode, f64, u32) -> f64;

/// The node's own magnitude at each sampled frequency, in decibels.
pub(crate) fn eq_well_magnitudes(
    effect: &Effect,
    at: TimeCode,
    magnitude: MagnitudeSource,
) -> Vec<f64> {
    (0..EQ_WELL_SAMPLES)
        .map(|index| magnitude(effect, at, eq_well_hertz(index), EQ_WELL_SAMPLE_RATE))
        .collect()
}

/// AU5 §6.2 rule 124: the magnitude source one card's well is drawn from, if
/// the card has a well at all.
pub(crate) fn well_magnitude_source(name: &str) -> Option<MagnitudeSource> {
    match name {
        "audio_parametric_eq" => Some(kinewright_media::parametric_eq_magnitude_db),
        "audio_hum_removal" => Some(kinewright_media::hum_removal_magnitude_db),
        _ => None,
    }
}

/// Where one frequency lands across the well: log across 20 Hz to 20 kHz.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn eq_well_x(rect: egui::Rect, hertz: f64) -> f32 {
    let fraction =
        (hertz / EQ_WELL_MIN_HERTZ).log10() / (EQ_WELL_MAX_HERTZ / EQ_WELL_MIN_HERTZ).log10();
    rect.left() + rect.width() * fraction.clamp(0.0, 1.0) as f32
}

/// Where one magnitude lands down the well: +24 dB at the top, -24 at the
/// bottom, clamped.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn eq_well_y(rect: egui::Rect, decibels: f64) -> f32 {
    let fraction = ((EQ_WELL_RANGE_DB - decibels) / (2.0 * EQ_WELL_RANGE_DB)).clamp(0.0, 1.0);
    rect.top() + rect.height() * fraction as f32
}

/// The polyline the well paints for one node.
pub(crate) fn eq_well_points(
    effect: &Effect,
    rect: egui::Rect,
    at: TimeCode,
    magnitude: MagnitudeSource,
) -> Vec<egui::Pos2> {
    eq_well_magnitudes(effect, at, magnitude)
        .into_iter()
        .enumerate()
        .map(|(index, decibels)| {
            egui::pos2(
                eq_well_x(rect, eq_well_hertz(index)),
                eq_well_y(rect, decibels),
            )
        })
        .collect()
}

/// The read-only magnitude well of an expanded parametric EQ (AU2 §6.7), and
/// of an expanded hum-removal card, whole (AU5 §6.2 rule 124).
///
/// The comb parameterises exactly one thing — where the decibels come from.
/// The x-map, the y-map, the 1.6 px polyline, the `eq_well:{id}` rect and the
/// "Magnitude at 48 kHz." tooltip are the EQ well's, unchanged, because a
/// notch cascade and a shelving cascade are the same kind of picture.
fn eq_well(ui: &mut egui::Ui, effect: &Effect, at: TimeCode, magnitude: MagnitudeSource) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::MIXER_EQ_CURVE_HEIGHT),
        egui::Sense::hover(),
    );
    record_keyed_rect("eq_well", effect.id.0, rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, radius::SM, color::LETTERBOX);
    theme::paint_inset_well(&painter, rect, radius::px(radius::SM));
    painter.rect_stroke(
        rect,
        radius::SM,
        egui::Stroke::new(1.0, color::BORDER_SUBTLE),
        egui::StrokeKind::Inside,
    );
    for decibels in [-EQ_WELL_RANGE_DB, -12.0, 0.0, 12.0, EQ_WELL_RANGE_DB] {
        let y = eq_well_y(rect, decibels);
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(1.0, color::BORDER_SUBTLE),
        );
    }
    for hertz in [100.0, 1_000.0, 10_000.0] {
        let x = eq_well_x(rect, hertz);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0, color::BORDER_SUBTLE),
        );
    }
    painter.add(egui::Shape::line(
        eq_well_points(effect, rect, at, magnitude),
        egui::Stroke::new(1.6, color::TEXT_PRIMARY),
    ));
    response.on_hover_text(EQ_WELL_TOOLTIP);
}

// ---------------------------------------------------------------------------
// The `Learn` gesture
// ---------------------------------------------------------------------------

/// The button that teaches a denoise node its floor (AU5 §6.3 rule 126).
pub(crate) const LEARN_PROFILE_BUTTON: &str = "Learn profile";
/// AU5 §6.3 rule 128, first refusal: the analysis has not finished.
///
/// Distinct from [`LEARN_NO_SILENCE`] on purpose. Collapsing the two would
/// tell the editor the recording has no silence in it when what is true is
/// that nothing has been looked at yet.
pub(crate) const LEARN_ANALYSIS_RUNNING: &str =
    "Silence analysis is still running for these tracks.";
/// AU5 §6.3 rule 128, second refusal: the analysis is in and nothing is long
/// enough.
///
/// 469 ms is `NOISE_PROFILE_MINIMUM_FRAMES` (22 528 sample frames at the
/// 48 kHz render rate) said in the only unit an editor has (AU5 §3.7 rule 63).
pub(crate) const LEARN_NO_SILENCE: &str = "No silence span reaches 469 ms.";

/// What the denoise card knows about the range a `Learn` would read (AU5 §6.3
/// rule 128).
///
/// Session state read *in*; the click that acts on it travels *out* through
/// [`NoiseLearn::requested`]. The card itself reaches no document and no
/// engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NoiseLearnRange {
    /// `silence_status` is not ready for at least one asset on the tracks
    /// feeding this chain.
    #[default]
    Analysing,
    /// Ready, and no span reaches [`LEARN_NO_SILENCE`]'s 469 ms.
    NoSilence,
    /// The longest silence span on those tracks, in **project** frames.
    Span(TimeCode, TimeCode),
}

/// The `Learn` channel, in and out, in one argument (AU5 §6.3 rule 126).
///
/// `requested` is a request rather than an edit: document edits leave the
/// Mixer through `InspectorEdits`, so "the card pushes no operation" stays
/// enforced by the type the card cannot reach rather than by this one.
#[derive(Debug)]
pub(crate) struct NoiseLearn<'a> {
    pub(crate) range: NoiseLearnRange,
    pub(crate) requested: &'a mut Option<(AudioChain, EffectId)>,
}

/// The `Learn profile` row of an expanded denoise card (AU5 §6.3).
fn learn_row(ui: &mut egui::Ui, chain: MixerChain, effect: &Effect, learn: &mut NoiseLearn<'_>) {
    let span = match learn.range {
        NoiseLearnRange::Span(start, end) => Some((start, end)),
        NoiseLearnRange::Analysing | NoiseLearnRange::NoSilence => None,
    };
    ui.horizontal(|ui| {
        let button = ui.add_enabled(span.is_some(), egui::Button::new(LEARN_PROFILE_BUTTON));
        record_keyed_rect("learn", effect.id.0, button.rect);
        let button = match span {
            // Hover names the span that will be used, so the measurement is
            // never taken from a range the editor cannot see.
            Some((start, end)) => button.on_hover_text(format!(
                "Learn the floor from frames {}\u{2013}{} \u{2014} the longest silence on the \
                 tracks feeding this chain.",
                start.0, end.0
            )),
            None => button,
        };
        if button.clicked() {
            *learn.requested = Some((
                chain
                    .selection()
                    .chain()
                    .expect("Learn profile is offered on a bus or the master"),
                effect.id,
            ));
        }
    });
    // The refusal takes its own line and **wraps**: it is a sentence, and a
    // sentence beside the button pushes the 400 px column wider than the
    // pane's own token, which the dock pin catches.
    let refusal = match learn.range {
        NoiseLearnRange::Analysing => Some(LEARN_ANALYSIS_RUNNING),
        NoiseLearnRange::NoSilence => Some(LEARN_NO_SILENCE),
        NoiseLearnRange::Span(_, _) => None,
    };
    if let Some(refusal) = refusal {
        ui.add(
            egui::Label::new(
                egui::RichText::new(refusal)
                    .font(theme::medium(type_size::MICRO))
                    .color(color::TEXT_MUTED),
            )
            .wrap(),
        );
    }
}

// ---------------------------------------------------------------------------
// The learned-noise-floor well
// ---------------------------------------------------------------------------

/// The bottom of the noise well's scale, in decibels (AU5 §6.2 rule 125).
///
/// `PROFILE_BAND_NEUTRAL_TENTH_DB` is −1200, so an unlearned band sits exactly
/// on the floor and paints nothing at all.
pub(crate) const NOISE_WELL_FLOOR_DB: f64 = -120.0;
/// What the well says when nothing has been learned yet (AU5 §6.2 rule 125).
///
/// Deliberately not a warning: DESIGN.md reserves status colour for outcomes,
/// and an untaught denoiser is a node at its identity, not a fault.
pub(crate) const NOISE_WELL_EMPTY_NOTE: &str = "No profile learned.";
/// What the well is, said once, where the editor can reach it.
pub(crate) const NOISE_WELL_TOOLTIP: &str =
    "The noise floor this node was taught, −120…0 dB per third octave.";

/// The 31 learned bands of one denoise node, low to high, in tenth dB.
///
/// A missing row reads the neutral, which is what makes AU5 §2.1 rule 6's
/// write-all-or-none block legible: an inserted node carries none of the 31
/// and the well shows an untaught floor rather than an empty chart.
pub(crate) fn noise_well_bands(effect: &Effect) -> [i64; NOISE_PROFILE_BAND_COUNT] {
    let mut bands = [PROFILE_BAND_NEUTRAL_TENTH_DB; NOISE_PROFILE_BAND_COUNT];
    for (band, name) in bands.iter_mut().zip(NOISE_PROFILE_PARAMETER_NAMES) {
        if let Some(ParamValue::Integer(value)) = effect.parameters.get(name) {
            *band = *value;
        }
    }
    bands
}

/// Whether every band is still at the neutral, which is the runtime's own
/// "nothing has been learned" test (AU5 §2.1 rule 5).
pub(crate) fn noise_well_is_unlearned(bands: &[i64; NOISE_PROFILE_BAND_COUNT]) -> bool {
    bands
        .iter()
        .all(|band| *band <= PROFILE_BAND_NEUTRAL_TENTH_DB)
}

/// The 31 bars the well paints, low band to high, in the rect it was given.
///
/// Pure geometry: a band at the −120 dB floor is a zero-height rect and a band
/// at 0 dB fills the well. Bars are laid on an exact 31-wide grid with a 1 px
/// gutter, so the chart reads as a spectrum rather than as 31 meters.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub(crate) fn noise_well_bars(
    rect: egui::Rect,
    bands: &[i64; NOISE_PROFILE_BAND_COUNT],
) -> Vec<egui::Rect> {
    let step = rect.width() / NOISE_PROFILE_BAND_COUNT as f32;
    bands
        .iter()
        .enumerate()
        .map(|(index, band)| {
            let decibels = *band as f64 / 10.0;
            let fraction =
                ((decibels - NOISE_WELL_FLOOR_DB) / -NOISE_WELL_FLOOR_DB).clamp(0.0, 1.0) as f32;
            let left = rect.left() + step * index as f32;
            egui::Rect::from_min_max(
                egui::pos2(left, rect.bottom() - rect.height() * fraction),
                egui::pos2(left + (step - 1.0).max(1.0), rect.bottom()),
            )
        })
        .collect()
}

/// The read-only learned-floor well of an expanded denoise card (AU5 §6.2
/// rule 125).
///
/// A **new** chart, not the EQ well parameterised again: it shares the well
/// chrome — the letterbox fill, the inset and the subtle border — and nothing
/// else. Its scale is −120…0 dB rather than ±24, it has 31 bars rather than a
/// polyline, and it is drawn in `TEXT_SECONDARY` and never in the accent,
/// because it reports the floor the node was taught rather than an outcome.
fn noise_well(ui: &mut egui::Ui, effect: &Effect) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::MIXER_NOISE_WELL_HEIGHT),
        egui::Sense::hover(),
    );
    record_keyed_rect("noise_well", effect.id.0, rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, radius::SM, color::LETTERBOX);
    theme::paint_inset_well(&painter, rect, radius::px(radius::SM));
    painter.rect_stroke(
        rect,
        radius::SM,
        egui::Stroke::new(1.0, color::BORDER_SUBTLE),
        egui::StrokeKind::Inside,
    );
    let bands = noise_well_bands(effect);
    if noise_well_is_unlearned(&bands) {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            NOISE_WELL_EMPTY_NOTE,
            theme::medium(type_size::MICRO),
            color::TEXT_MUTED,
        );
    } else {
        for bar in noise_well_bars(rect, &bands) {
            painter.rect_filled(bar, 0.0, color::TEXT_SECONDARY);
        }
    }
    response.on_hover_text(NOISE_WELL_TOOLTIP);
}

/// The filled part of a gain-reduction bar (AU2 §6.7).
///
/// The one meter in the product that fills from the right, because it shows
/// how far a signal has been pushed down rather than how loud it is.
pub(crate) fn reduction_fill_rect(rect: egui::Rect, reduction_db: f32) -> egui::Rect {
    let fraction = (reduction_db / MIXER_REDUCTION_METER_RANGE_DB).clamp(0.0, 1.0);
    egui::Rect::from_min_max(
        egui::pos2(rect.right() - rect.width() * fraction, rect.top()),
        rect.max,
    )
}

fn reduction_bar(
    ui: &mut egui::Ui,
    chain: AudioChain,
    effect: EffectId,
    levels: &MixerMeterLevels,
) {
    let reduction = levels.reduction(chain, effect);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::MIXER_REDUCTION_METER_HEIGHT),
        egui::Sense::hover(),
    );
    record_keyed_rect("reduction", effect.0, rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 1.0, color::SURFACE_ACTIVE);
    let fill = reduction_fill_rect(rect, reduction);
    if fill.width() > 0.0 {
        // A functional status colour, never the accent (DESIGN.md).
        painter.rect_filled(fill, 1.0, color::STATUS_WARNING);
    }
    response.on_hover_text(reduction_readout(reduction));
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------

/// The nine nodes the mixer's `+ Effect` menu offers, in menu order (AU2 §6.8,
/// AU5 §6.1 rule 118).
///
/// `audio_eq` and `audio_limiter` are retained, valid, and processed, but a
/// new chain never grows one: the parametric EQ and the true-peak limiter
/// replace them.
///
/// AU5's three repair nodes are appended after `audio_gate` rather than at the
/// end, because `audio_ducking` and `audio_true_peak_limiter` are the two the
/// menu already warns about and a repair node is an ordinary identity insert.
pub(crate) const INSERTABLE_AUDIO_EFFECTS: [&str; 9] = [
    "audio_gain",
    "audio_parametric_eq",
    "audio_compressor",
    "audio_gate",
    "audio_denoise",
    "audio_hum_removal",
    "audio_declick",
    "audio_ducking",
    "audio_true_peak_limiter",
];

/// The two parameter names the menu's self-check accepts at `MixerUnit::Plain`
/// (AU5 §6.1 rule 119).
///
/// `bypass` is a flag the card header draws as a checkbox and never as a
/// number, and `harmonic_count` is a count — a pure integer with no unit to
/// wear. Everything else that reads `Plain` is a parameter whose suffix
/// `mixer_unit` does not know, which is the drift the check exists to catch.
const PLAIN_UNIT_ALLOW_LIST: [&str; 2] = ["bypass", "harmonic_count"];

/// Why one `+ Effect` menu entry is malformed, if it is (AU5 §6.1 rule 119).
///
/// AU2 asserted that each name of `INSERTABLE_AUDIO_EFFECTS` was a member of
/// `INSERTABLE_AUDIO_EFFECTS`, which cannot fail. This one can: it asks
/// whether the name is a registered audio effect, whether the registry has a
/// descriptor to insert at, and whether every row of that descriptor resolves
/// to a unit the card can label — the three ways an appended name could
/// silently paint a broken card.
fn insertable_menu_defect(name: &str) -> Option<String> {
    if !kinewright_core::is_audio_effect(name) {
        return Some(format!(
            "the `+ Effect` menu offers `{name}`, which is not an audio effect"
        ));
    }
    let Some(descriptor) = effect_descriptor(name) else {
        return Some(format!(
            "the `+ Effect` menu offers `{name}`, which has no effect descriptor"
        ));
    };
    descriptor
        .parameters
        .iter()
        .find(|parameter| {
            mixer_unit(parameter.name) == MixerUnit::Plain
                && !PLAIN_UNIT_ALLOW_LIST.contains(&parameter.name)
        })
        .map(|parameter| {
            format!(
                "the `+ Effect` menu offers `{name}`, whose `{}` reads no unit from its \
                 suffix \u{2014} add the suffix to `mixer_unit` or the name to \
                 `PLAIN_UNIT_ALLOW_LIST`",
                parameter.name
            )
        })
}

/// Why `Ducking` is refused on a chain with no sidechain source.
pub(crate) const DUCKING_NEEDS_SIDECHAIN: &str = "Ducking needs at least one sidechain track";
/// Why a lookahead node is refused on a chain that has spent its budget.
pub(crate) const LOOKAHEAD_BUDGET_SPENT: &str =
    "This chain already uses its 20 ms lookahead budget";

/// The declared lookahead one node inserts at, from its descriptor neutral.
pub(crate) fn insertion_lookahead_milliseconds(name: &str) -> i64 {
    effect_descriptor(name)
        .and_then(|descriptor| descriptor.parameter("lookahead_milliseconds"))
        .map_or(0, |parameter| parameter.neutral)
}

/// Why one menu entry is disabled on this chain, if it is (AU2 §6.8).
pub(crate) fn insertion_block(
    effects: &[Effect],
    sidechain: &[TrackId],
    name: &str,
) -> Option<&'static str> {
    if name == "audio_ducking" && sidechain.is_empty() {
        return Some(DUCKING_NEEDS_SIDECHAIN);
    }
    let after = chain_lookahead_milliseconds(effects) + insertion_lookahead_milliseconds(name);
    if after > CHAIN_LOOKAHEAD_MILLISECONDS {
        return Some(LOOKAHEAD_BUDGET_SPENT);
    }
    None
}

/// What a menu entry warns about when it is offered but is not an identity.
pub(crate) const fn insertion_warning(name: &str) -> &'static str {
    match name.as_bytes() {
        b"audio_ducking" => "Inserts a working 12 dB duck below -30 dBFS.",
        b"audio_true_peak_limiter" => {
            "Inserts a 0 dBFS true-peak ceiling with 5 ms of lookahead, which stops playback once."
        }
        _ => "Inserts at its neutral values, which is an exact identity.",
    }
}

/// Append one node at its descriptor neutrals (AU2 §6.8).
///
/// Every parameter is written explicitly, so the menu can never diverge from
/// the descriptor table, and the id is `max + 1` **within this chain** — there
/// is no document-wide effect-id helper (AU2 §5.4, N1).
pub(crate) fn insert_audio_effect(effects: &mut Vec<Effect>, name: &str) {
    let Some(descriptor) = effect_descriptor(name) else {
        return;
    };
    let id = EffectId(
        effects
            .iter()
            .map(|effect| effect.id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
    );
    effects.push(Effect {
        id,
        name: name.to_owned(),
        parameters: descriptor
            .parameters
            .iter()
            // AU5 §2.1 rule 6: the 31 profile rows are a write-all-or-none
            // block whose absence *is* the neutral, so an inserted denoise
            // node carries none of them. Writing 31 `-1200` entries into
            // every document that inserts the node would also keep §4.2's
            // compact-render omission arm from ever firing in the app's own
            // projects.
            .filter(|parameter| !is_noise_profile_parameter(parameter.name))
            .map(|parameter| {
                (
                    parameter.name.to_owned(),
                    ParamValue::Integer(parameter.neutral),
                )
            })
            .collect::<BTreeMap<_, _>>(),
        keyframes: BTreeMap::new(),
    });
}

fn add_effect_menu(ui: &mut egui::Ui, chain: MixerChain, edits: &mut MixerChainEdits) {
    let response = ui.menu_button("+ Effect", |ui| {
        for name in INSERTABLE_AUDIO_EFFECTS {
            debug_assert!(
                insertable_menu_defect(name).is_none(),
                "{}",
                insertable_menu_defect(name).unwrap_or_default()
            );
            let block = insertion_block(chain.effects(), chain.sidechain(), name);
            let entry = ui.add_enabled(
                block.is_none(),
                egui::Button::new(effect_display_name(name)),
            );
            record_param_rect("insert", EffectId(0), name, entry.rect);
            let entry = match block {
                Some(reason) => entry.on_disabled_hover_text(reason),
                None => entry.on_hover_text(insertion_warning(name)),
            };
            if entry.clicked() {
                insert_audio_effect(chain.effects_mut(edits), name);
                ui.close();
            }
        }
    });
    record_strip_rect("add_effect", response.response.rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::{EBU_R128_PROGRAMME_TARGET, STREAMING_PLATFORM_TARGET};

    fn snapshot() -> LoudnessSnapshot {
        LoudnessSnapshot {
            momentary_lufs_hundredths: Some(-1_830),
            short_term_lufs_hundredths: Some(-1_705),
            integrated_lufs_hundredths: Some(-1_600),
            loudness_range_lu_hundredths: Some(620),
            true_peak_dbtp_hundredths: Some(-130),
            programme_seconds: 42,
        }
    }

    /// AU3 §7 A18: the bar runs −40…0 LUFS, empty for `None`, clamped.
    #[test]
    fn loudness_bar_fill_spans_minus_forty_to_zero_lufs() {
        for (lufs, expected, label) in [
            (Some(-4_000), 0.0, "the floor"),
            (Some(-2_000), 0.5, "halfway"),
            (Some(0), 1.0, "the ceiling"),
            (None, 0.0, "none"),
            (Some(-9_000), 0.0, "clamped at the floor"),
            (Some(300), 1.0, "clamped at the ceiling"),
        ] {
            let fill = loudness_bar_fill(lufs);
            assert!(
                (fill - expected).abs() < f32::EPSILON,
                "{label}: {lufs:?} fills {fill}, expected {expected}"
            );
        }
    }

    /// AU3 §7 A18: the truth table against the dialog's profile target —
    /// `text-secondary` below the tolerance, success inside it, warning
    /// above, no fill for `None`; quiet is never a failure and the accent is
    /// never used.
    #[test]
    fn loudness_bar_color_follows_the_target_tolerance() {
        for (target, label) in [
            (STREAMING_PLATFORM_TARGET, "streaming"),
            (EBU_R128_PROGRAMME_TARGET, "R128"),
        ] {
            let centre = target.integrated_lufs_hundredths;
            let tol = target.tolerance_lu_hundredths;
            assert_eq!(loudness_bar_color(None, target), None, "{label}: none");
            assert_eq!(
                loudness_bar_color(Some(centre - tol - 1), target),
                Some(color::TEXT_SECONDARY),
                "{label}: just under the tolerance is quiet, not a failure"
            );
            for inside in [centre - tol, centre, centre + tol] {
                assert_eq!(
                    loudness_bar_color(Some(inside), target),
                    Some(color::STATUS_SUCCESS),
                    "{label}: {inside} is inside [t − tol, t + tol]"
                );
            }
            assert_eq!(
                loudness_bar_color(Some(centre + tol + 1), target),
                Some(color::STATUS_WARNING),
                "{label}: just over the tolerance warns"
            );
            assert_eq!(
                loudness_bar_color(Some(0), target),
                Some(color::STATUS_WARNING),
                "{label}: full scale warns"
            );
        }
        assert_ne!(
            loudness_bar_color(Some(-1_400), STREAMING_PLATFORM_TARGET),
            loudness_bar_color(Some(-1_400), EBU_R128_PROGRAMME_TARGET),
            "the two targets disagree about −14 LUFS"
        );
        for lufs in [None, Some(-9_000), Some(-1_400), Some(0)] {
            assert_ne!(
                loudness_bar_color(lufs, STREAMING_PLATFORM_TARGET),
                Some(color::ACCENT),
                "never the accent"
            );
        }
    }

    /// AU3 §4.4 item 3: `TP` turns status-danger strictly above the target's
    /// ceiling, so a peak exactly at the ceiling still conforms.
    #[test]
    fn true_peak_color_turns_danger_only_above_the_ceiling() {
        for (target, label) in [
            (STREAMING_PLATFORM_TARGET, "streaming"),
            (EBU_R128_PROGRAMME_TARGET, "R128"),
        ] {
            let ceiling = target.true_peak_ceiling_dbtp_hundredths;
            for (dbtp, description) in [
                (None, "an unmeasured peak"),
                (Some(ceiling - 1), "just under the ceiling"),
                (Some(ceiling), "exactly at the ceiling"),
            ] {
                assert_eq!(
                    true_peak_color(dbtp, target),
                    color::TEXT_SECONDARY,
                    "{label}: {description} is not a failure"
                );
            }
            for (dbtp, description) in [(ceiling + 1, "just over the ceiling"), (0, "full scale")] {
                assert_eq!(
                    true_peak_color(Some(dbtp), target),
                    color::STATUS_DANGER,
                    "{label}: {description} is over the ceiling"
                );
            }
        }
    }

    /// AU3 §7 A18: the readout line and the strip line, with `—` per `None`.
    #[test]
    fn loudness_readout_renders_a_dash_per_none() {
        assert_eq!(
            loudness_readout(snapshot()),
            "I -16.0 LUFS · LRA 6.2 LU · TP -1.3 dBTP · 0:42"
        );
        assert_eq!(
            loudness_readout(LoudnessSnapshot::default()),
            "I — LUFS · LRA — LU · TP — dBTP · 0:00"
        );
        assert_eq!(
            loudness_readout(LoudnessSnapshot {
                integrated_lufs_hundredths: Some(-2_296),
                loudness_range_lu_hundredths: None,
                true_peak_dbtp_hundredths: Some(-5),
                programme_seconds: 3_725,
                ..LoudnessSnapshot::default()
            }),
            "I -23.0 LUFS · LRA — LU · TP -0.1 dBTP · 62:05",
            "the tenth rounds half away from zero and the sign survives a value in (−1, 0)"
        );
        assert_eq!(integrated_strip_line(snapshot()), "I -16.0");
        assert_eq!(integrated_strip_line(LoudnessSnapshot::default()), "I —");
        assert_eq!(loudness_value(Some(0)), "0.0");
        assert_eq!(loudness_value(Some(-1_835)), "-18.4");
        assert_eq!(loudness_value(Some(1_234)), "12.3");
        assert_eq!(programme_clock(59), "0:59");
        assert_eq!(programme_clock(60), "1:00");
    }

    /// Lay the section out at the pane's width and report its height.
    fn measure_loudness_section(snapshot: LoudnessSnapshot) -> f32 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut measured = egui::Vec2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
                loudness_section(ui, snapshot, STREAMING_PLATFORM_TARGET);
                measured = ui.min_rect().size();
            });
        });
        measured.y
    }

    /// AU3 §7 A18: the section spends at most 90 px of the pane, measured
    /// with every value present and with none — the pane scrolls, but the
    /// section should be on screen before the first card in a 260 px dock.
    /// Measured on this build: 86 px either way.
    ///
    /// Part A measured 74 px against an 80 px budget. Part B's half of the
    /// F21 sentence (§4.4, §6.8) takes the muted line to two rows at the
    /// pane's width, which is one `MICRO` row — 12 px — more section. The
    /// sentence is the contract and the budget is the constraint it has to be
    /// checked against, so the budget moves with the measurement recorded
    /// beside it rather than the sentence being cut to fit.
    #[test]
    fn the_loudness_section_fits_its_height_budget() {
        const BUDGET: f32 = 90.0;
        const MEASURED: f32 = 86.0;
        for (label, snapshot) in [
            ("measured", snapshot()),
            ("unmeasured", LoudnessSnapshot::default()),
        ] {
            let height = measure_loudness_section(snapshot);
            assert!(
                height <= BUDGET,
                "the {label} LOUDNESS section is {height} px tall, over the {BUDGET} px budget"
            );
            assert!(
                (height - MEASURED).abs() <= 1.0,
                "the doc comment says the {label} section measures {MEASURED} px; it measured {height}"
            );
        }
    }

    /// AU3 §7 A18: what the section says, headless — the caps label, both bar
    /// labels with their readouts, the readout line, `Reset`, and the Part A
    /// F21 sentence — and that it says `—` where the meter has nothing yet.
    #[test]
    fn the_loudness_section_paints_its_labels_and_dashes() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        for (snapshot, momentary, expect_reset) in [
            (snapshot(), "-18.3", false),
            (LoudnessSnapshot::default(), LOUDNESS_NONE, false),
        ] {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
                let reset = loudness_section(ui, snapshot, STREAMING_PLATFORM_TARGET);
                assert_eq!(reset, expect_reset, "nothing was clicked");
            });
            let painted = theme::painted_text(&output);
            for expected in [
                "LOUDNESS",
                "M",
                "S",
                momentary,
                "Reset",
                LOUDNESS_MONITORING_NOTE,
            ] {
                assert!(
                    painted.iter().any(|text| text == expected),
                    "the section paints {expected:?}; it painted {painted:?}"
                );
            }
            let line = loudness_readout(snapshot);
            assert!(
                painted.contains(&line),
                "the readout line is drawn whole as {line:?}; it painted {painted:?}"
            );
        }
    }

    // ---- AU4 Part B §5.4: the chain pane's `AUTOMATION` section ----

    fn automation_effects(names: &[&str]) -> Vec<Effect> {
        let mut effects = Vec::new();
        for name in names {
            insert_audio_effect(&mut effects, name);
        }
        effects
    }

    fn automation_bus(effects: Vec<Effect>, curve: Option<AutomationCurve>) -> AudioBus {
        AudioBus {
            id: kinewright_core::AudioBusId(1),
            name: "Dialogue".to_owned(),
            tracks: vec![TrackId(1)],
            gain_tenth_db: -30,
            effects,
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: curve,
        }
    }

    /// Ten keys, so the list has to scroll inside its four visible rows.
    fn ten_key_curve() -> AutomationCurve {
        AutomationCurve {
            keyframes: (0..10)
                .map(|index| kinewright_core::Keyframe {
                    at: TimeCode(index * 10),
                    value: -index * 10,
                    interpolation: kinewright_core::KeyframeInterpolation::Linear,
                })
                .collect(),
        }
    }

    /// Lay the section out at the pane's width and report its height.
    fn measure_automation_section(bus: &AudioBus) -> f32 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut edits = MixerChainEdits::default();
        let mut measured = egui::Vec2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
                automation_section(
                    ui,
                    MixerChain::Bus(bus),
                    TimeCode::ZERO,
                    TimeCode(300),
                    &mut edits,
                );
                measured = ui.min_rect().size();
            });
        });
        measured.y
    }

    /// AU4 §7 B9 (rule 116): the section spends at most 120 px of the 400 px
    /// pane, measured with an empty curve and with a scrolled ten-key one.
    ///
    /// The list is allocated at a fixed four rows so both cases measure the
    /// same, exactly as `the_loudness_section_fits_its_height_budget` pins its
    /// own two.
    #[test]
    fn the_automation_section_fits_its_height_budget() {
        const BUDGET: f32 = 120.0;
        const MEASURED: f32 = 116.0;
        for (label, curve) in [("empty", None), ("scrolled ten-key", Some(ten_key_curve()))] {
            let bus = automation_bus(automation_effects(&["audio_compressor"]), curve);
            let height = measure_automation_section(&bus);
            println!("AU4_AUTOMATION_SECTION case=\"{label}\" px={height}");
            assert!(
                height <= BUDGET,
                "the {label} AUTOMATION section is {height} px tall, over the {BUDGET} px budget"
            );
            assert!(
                (height - MEASURED).abs() <= 1.0,
                "the doc comment says the {label} section measures {MEASURED} px; \
                 it measured {height}"
            );
        }
    }

    /// AU4 §7 B9 (rule 113): the combo lists `Fader` plus exactly the
    /// non-hold-only, non-static parameters of the selected chain.
    ///
    /// `is_hold_only_parameter` and `is_static_audio_parameter` gain their
    /// first app readers here: the app stops guessing automatability from
    /// `mixer_unit()`'s name match.
    #[test]
    fn the_automation_combo_lists_only_the_automatable_parameters() {
        let bus = automation_bus(
            automation_effects(&[
                "audio_compressor",
                "audio_true_peak_limiter",
                "audio_ducking",
            ]),
            None,
        );
        let chain = MixerChain::Bus(&bus);
        let targets = automation_targets(chain);
        assert_eq!(
            targets.first(),
            Some(&AutomationTarget::Fader),
            "`Fader` is always the first choice"
        );
        let named = targets
            .iter()
            .filter_map(|target| match target {
                AutomationTarget::Fader | AutomationTarget::TrackParameter(_) => None,
                AutomationTarget::Node(_, name) => Some(*name),
            })
            .collect::<Vec<_>>();
        for excluded in [
            "bypass",
            "detector",
            "true_peak",
            "lookahead_milliseconds",
            "rms_window_milliseconds",
        ] {
            assert!(
                !named.contains(&excluded),
                "`{excluded}` is hold-only or static and takes no curve: {named:?}"
            );
        }
        assert!(
            named.contains(&"threshold_tenth_db") && named.contains(&"ratio_hundredths"),
            "the compressor's automatable parameters are offered: {named:?}"
        );
        // Every offered parameter is one core would accept a curve on.
        for effect in &bus.effects {
            for parameter in effect_descriptor(&effect.name)
                .expect("a registered node")
                .parameters
            {
                let offered = targets.contains(&AutomationTarget::Node(effect.id, parameter.name));
                let automatable = !is_hold_only_parameter(&effect.name, parameter.name)
                    && !is_static_audio_parameter(&effect.name, parameter.name);
                assert_eq!(
                    offered, automatable,
                    "`{}`.`{}` offered={offered} automatable={automatable}",
                    effect.name, parameter.name
                );
            }
        }
        assert_eq!(
            automation_target_label(chain, AutomationTarget::Fader),
            "Fader"
        );
    }

    /// AU4 §7 B9 (rule 115): a keyframed card reads the audible frame instead
    /// of its neutral, and the EQ well samples the same frame.
    #[test]
    fn a_keyframed_card_reads_the_audible_frame_instead_of_its_neutral() {
        let mut effects = automation_effects(&["audio_gain"]);
        effects[0].keyframes.insert(
            "gain_tenth_db".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    kinewright_core::Keyframe {
                        at: TimeCode::ZERO,
                        value: -120,
                        interpolation: kinewright_core::KeyframeInterpolation::Linear,
                    },
                    kinewright_core::Keyframe {
                        at: TimeCode(30),
                        value: 0,
                        interpolation: kinewright_core::KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        assert_eq!(
            parameter_value(&effects[0], "gain_tenth_db", TimeCode::ZERO),
            -120,
            "the card shows the ride, not the stored neutral"
        );
        assert_eq!(
            parameter_value(&effects[0], "gain_tenth_db", TimeCode(30)),
            0
        );
        assert_eq!(
            parameter_value(&effects[0], "gain_tenth_db", TimeCode(15)),
            -60,
            "and every frame between them"
        );
        // An un-keyframed parameter still reads exactly as it did.
        let plain = automation_effects(&["audio_gain"]);
        assert_eq!(
            parameter_value(&plain[0], "gain_tenth_db", TimeCode(15)),
            parameter_value(&plain[0], "gain_tenth_db", TimeCode::ZERO)
        );

        let mut eq = automation_effects(&["audio_parametric_eq"]);
        eq[0].keyframes.insert(
            "band1_gain_tenth_db".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    kinewright_core::Keyframe {
                        at: TimeCode::ZERO,
                        value: 0,
                        interpolation: kinewright_core::KeyframeInterpolation::Linear,
                    },
                    kinewright_core::Keyframe {
                        at: TimeCode(30),
                        value: 120,
                        interpolation: kinewright_core::KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        let flat = eq_well_magnitudes(
            &eq[0],
            TimeCode::ZERO,
            kinewright_media::parametric_eq_magnitude_db,
        );
        let boosted = eq_well_magnitudes(
            &eq[0],
            TimeCode(30),
            kinewright_media::parametric_eq_magnitude_db,
        );
        assert!(
            boosted.iter().zip(&flat).any(|(late, early)| late > early),
            "the well samples the audible frame, not `TimeCode::ZERO`"
        );
    }

    /// A master chain carrying one node, so the master pane has cards as well
    /// as its two sections.
    fn automation_master_document(curve: Option<AutomationCurve>) -> Document {
        let mut document = Document {
            duration: TimeCode(300),
            ..Document::default()
        };
        document.audio_mix.master = AudioMaster {
            gain_tenth_db: -20,
            effects: automation_effects(&["audio_gain"]),
            gain_curve: curve,
        };
        document
    }

    /// Lay the whole master pane out at the pane's width and report its
    /// content height — what the 400 px column has to scroll.
    fn measure_master_pane(document: &Document) -> f32 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let levels = MixerMeterLevels::default();
        let mut edits = MixerChainEdits::default();
        let mut measured = egui::Vec2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
                let mut requested = None;
                let mut learn = NoiseLearn {
                    range: NoiseLearnRange::default(),
                    requested: &mut requested,
                };
                let reset = master_pane(
                    ui,
                    document,
                    &levels,
                    snapshot(),
                    TimeCode::ZERO,
                    STREAMING_PLATFORM_TARGET,
                    &mut edits,
                    &mut learn,
                );
                assert!(!reset, "nothing was clicked");
                measured = ui.min_rect().size();
            });
        });
        measured.y
    }

    /// AU4 §7 B9 (rule 116): the pane itself is asserted, not only the
    /// section.
    ///
    /// On the master pane a 120 px-budget `AUTOMATION` section sits below a
    /// 90 px-budget `LOUDNESS` section, so the two together are a pane-level
    /// claim. The pane is a scroll area in a dock whose minimum is 260 px, so
    /// the budget is what the column has to be able to show, and the content
    /// above it is what proves the scroll is doing work.
    #[test]
    fn the_master_pane_carrying_both_sections_fits_its_budget() {
        const BUDGET: f32 = 420.0;
        const MEASURED: f32 = 398.0;
        for (label, curve) in [("empty", None), ("scrolled ten-key", Some(ten_key_curve()))] {
            let document = automation_master_document(curve);
            let height = measure_master_pane(&document);
            println!("AU4_MASTER_PANE case=\"{label}\" px={height}");
            assert!(
                height <= BUDGET,
                "the {label} master pane is {height} px tall, over the {BUDGET} px budget"
            );
            assert!(
                (height - MEASURED).abs() <= 1.0,
                "the doc comment says the {label} master pane measures {MEASURED} px; \
                 it measured {height}"
            );
            assert!(
                height > 260.0,
                "and it is taller than the 260 px dock minimum, so the pane's scroll area \
                 is load-bearing rather than decorative"
            );
        }
    }

    /// AU4 §5.4 rule 114: every automation target gets its own row id key.
    ///
    /// `keyframe_row` derives its `DragValue` and `ComboBox` ids from
    /// `(key, index)`, so one shared key would let an in-flight text edit or
    /// drag survive a change of the target combo and land on a different
    /// parameter's row.
    #[test]
    fn each_automation_target_pushes_its_own_keyframe_row_id() {
        let targets = [
            AutomationTarget::Fader,
            AutomationTarget::Node(EffectId(3), "threshold_tenth_db"),
            AutomationTarget::Node(EffectId(3), "ratio_tenth"),
            AutomationTarget::Node(EffectId(4), "threshold_tenth_db"),
        ];
        let keys: Vec<String> = targets
            .iter()
            .map(|target| automation_row_key(*target))
            .collect();
        let unique: std::collections::BTreeSet<&String> = keys.iter().collect();
        assert_eq!(
            unique.len(),
            keys.len(),
            "no two targets share a row id key: {keys:?}"
        );
        assert_ne!(
            keys[0], AUTOMATION_LABEL,
            "and the key is not the section's constant caps label"
        );
        assert_eq!(
            automation_row_key(AutomationTarget::Node(EffectId(3), "ratio_tenth")),
            keys[2],
            "the key is stable for one target across frames"
        );
    }

    /// AU5 §7 A13 (§4.4 rule 84): `has_gain_computer` lives in core **once**,
    /// and neither of the two copies it replaced has grown back.
    ///
    /// The predicate existed twice, byte-identically: here, driving
    /// `reduction_bar`, and in `kinewright-media`'s `audio.rs`, driving
    /// `gain_reduction_keys`. AU5 adds `audio_denoise` to it, and a copy left
    /// behind in either file would silently take the old answer — the app half
    /// would draw no bar for the denoiser, the media half would publish no
    /// data for the bar to draw. A grep cannot fail a build, so the pin reads
    /// both files as text.
    ///
    /// `mixer_pane_ui.rs` is read to the start of this test module only,
    /// because the module quotes the name the pin is looking for. That
    /// truncation is kept honest by counting the **two call sites** as well:
    /// they sit in the middle of the production half, so a searched region
    /// that ever went short would fail loudly instead of passing on an empty
    /// prefix.
    #[test]
    fn au5_has_gain_computer_is_declared_in_neither_crate_that_used_to_own_it() {
        const PANE: &str = include_str!("mixer_pane_ui.rs");
        const MEDIA_AUDIO: &str = include_str!("../../kinewright-media/src/audio.rs");
        let pane = PANE
            .split_once("\n#[cfg(test)]")
            .expect("mixer_pane_ui.rs has a test module")
            .0;
        for (label, source) in [
            ("crates/kinewright-app/src/mixer_pane_ui.rs", pane),
            ("crates/kinewright-media/src/audio.rs", MEDIA_AUDIO),
        ] {
            assert_eq!(
                source.matches("fn has_gain_computer").count(),
                0,
                "{label} still declares its own `has_gain_computer`; AU5 rule 84 deletes both \
                 copies and reads `kinewright_core::has_gain_computer` instead"
            );
        }
        assert!(
            pane.contains("has_gain_computer,"),
            "and this file reads the core predicate through its `kinewright_core` import"
        );
        // Rule 84's second half — "both callers read the core one" — pinned in
        // the same test, and the reason the truncation above cannot make this
        // pin vacuous: the two call sites sit at the middle of the production
        // half, so a `#[cfg(test)]` item landing earlier in the file would
        // shorten `pane` past them and fail here rather than pass silently.
        assert_eq!(
            pane.matches("has_gain_computer(&effect.name)").count(),
            2,
            "the header readout and `reduction_bar`'s gate both read the core predicate, \
             and both are inside the region this pin searches"
        );
        assert!(
            has_gain_computer("audio_denoise"),
            "which is the one that knows about the denoiser, so `reduction_bar` gets a bar"
        );
    }

    // -----------------------------------------------------------------------
    // AU5 §6.1-§6.5: the three repair cards, the two wells and the four pins
    // -----------------------------------------------------------------------

    /// A bus carrying the three repair nodes, in chain order.
    fn repair_bus(names: &[&str]) -> AudioBus {
        automation_bus(automation_effects(names), None)
    }

    /// Lay one **expanded** card out at the pane's width and report its height.
    fn measure_card(bus: &AudioBus, index: usize, learn: NoiseLearnRange) -> f32 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let levels = MixerMeterLevels::default();
        let mut edits = MixerChainEdits::default();
        let mut requested = None;
        let mut measured = egui::Vec2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
                let mut learn = NoiseLearn {
                    range: learn,
                    requested: &mut requested,
                };
                chain_card(
                    ui,
                    MixerChain::Bus(bus),
                    index,
                    true,
                    &levels,
                    TimeCode::ZERO,
                    &mut edits,
                    &mut learn,
                );
                measured = ui.min_rect().size();
            });
        });
        assert!(requested.is_none(), "nothing was clicked");
        measured.y
    }

    /// AU5 §7 B13: the menu offers nine nodes and its self-check can fail.
    ///
    /// The AU2 assert it replaces re-asserted membership of the array it was
    /// iterating, which no edit could ever break. This one reads the registry.
    #[test]
    fn au5_the_effect_menu_self_check_can_fail() {
        for name in INSERTABLE_AUDIO_EFFECTS {
            assert_eq!(
                insertable_menu_defect(name),
                None,
                "every offered node passes its own check"
            );
        }
        assert!(
            insertable_menu_defect("color_wheels")
                .is_some_and(|defect| defect.contains("not an audio effect")),
            "a non-audio name is caught"
        );
        assert!(
            insertable_menu_defect("audio_nonesuch").is_some(),
            "a name with no descriptor is caught"
        );
        // The allow-list is exactly the two rows that are legitimately
        // unitless, and `harmonic_count` really does reach `Plain`.
        assert_eq!(mixer_unit("harmonic_count"), MixerUnit::Plain);
        assert_eq!(mixer_unit("notch_q_hundredths"), MixerUnit::Q);
        // `bypass` is drawn as the header's checkbox, never as a numbered
        // control, so it never reaches `mixer_unit`'s flag arm — which is why
        // it is on the allow-list rather than in the `detector | true_peak`
        // match.
        assert_eq!(mixer_unit("bypass"), MixerUnit::Plain);
        assert_eq!(mixer_unit("detector"), MixerUnit::Flag);
        assert_eq!(PLAIN_UNIT_ALLOW_LIST, ["bypass", "harmonic_count"]);
    }

    /// AU5 §2.1 rule 6 and §6.1 rule 120: the 31 profile rows are written by
    /// neither the insert nor the card.
    #[test]
    fn au5_the_profile_rows_are_never_inserted_and_never_dragged() {
        let effects = automation_effects(&["audio_denoise"]);
        let denoise = &effects[0];
        assert_eq!(
            denoise
                .parameters
                .keys()
                .filter(|name| is_noise_profile_parameter(name))
                .count(),
            0,
            "an inserted denoise node carries none of the 31 bands: {:?}",
            denoise.parameters.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            denoise.parameters.len(),
            5,
            "and it carries all five of the controls: {:?}",
            denoise.parameters.keys().collect::<Vec<_>>()
        );
        // The card draws a control for every row it does not skip, so the
        // count of drawn controls is the count of non-profile, non-bypass rows.
        let descriptor = effect_descriptor("audio_denoise").expect("registered");
        let drawn = descriptor
            .parameters
            .iter()
            .filter(|parameter| {
                parameter.name != "bypass" && !is_noise_profile_parameter(parameter.name)
            })
            .count();
        assert_eq!(drawn, 4, "denoise draws four controls, not 35");
    }

    /// AU5 §7 B14 (§6.2 rule 125): the noise well is pure geometry.
    #[test]
    fn au5_the_noise_well_is_a_pure_function_of_its_bands() {
        let effects = automation_effects(&["audio_denoise"]);
        let bands = noise_well_bands(&effects[0]);
        assert_eq!(bands.len(), NOISE_PROFILE_BAND_COUNT);
        assert!(
            noise_well_is_unlearned(&bands),
            "an inserted node reads unlearned, because a missing row is the neutral"
        );
        let rect = egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(310.0, size::MIXER_NOISE_WELL_HEIGHT),
        );
        let empty = noise_well_bars(rect, &bands);
        assert_eq!(empty.len(), NOISE_PROFILE_BAND_COUNT, "31 bars, always");
        for bar in &empty {
            assert!(
                bar.height() <= f32::EPSILON,
                "a band at the −120 dB floor paints nothing: {bar:?}"
            );
        }

        // A learned floor: −120 dB at the bottom band, 0 dB at the top.
        let mut learned = bands;
        learned[0] = -1_200;
        learned[NOISE_PROFILE_BAND_COUNT - 1] = 0;
        learned[15] = -600;
        assert!(!noise_well_is_unlearned(&learned));
        let bars = noise_well_bars(rect, &learned);
        assert!(bars[0].height() <= f32::EPSILON, "the floor is still empty");
        assert!(
            (bars[NOISE_PROFILE_BAND_COUNT - 1].height() - rect.height()).abs() <= 0.01,
            "0 dB fills the well: {} px of {}",
            bars[NOISE_PROFILE_BAND_COUNT - 1].height(),
            rect.height()
        );
        assert!(
            (bars[15].height() - rect.height() / 2.0).abs() <= 0.01,
            "−60 dB is half the well: {} px",
            bars[15].height()
        );
        // Low band to high, left to right, on an exact 31-wide grid.
        assert!(bars[0].left() < bars[1].left());
        assert!(bars[0].right() <= bars[1].left(), "the bars do not overlap");
        // Out-of-range values clamp rather than escaping the well.
        let mut wild = bands;
        wild[3] = 900;
        wild[4] = -9_000;
        let clamped = noise_well_bars(rect, &wild);
        assert!((clamped[3].height() - rect.height()).abs() <= 0.01);
        assert!(clamped[4].height() <= f32::EPSILON);
    }

    /// AU5 §7 B14 (§6.2 rule 124): the comb is the EQ well, parameterised.
    #[test]
    fn au5_the_hum_comb_is_the_eq_well_drawn_from_the_hum_response() {
        let mut effects = automation_effects(&["audio_hum_removal"]);
        // A working notch, so the comb is not a flat line.
        let node = &mut effects[0];
        node.parameters
            .insert("depth_tenth_db".to_owned(), ParamValue::Integer(-400));
        node.parameters
            .insert("harmonic_count".to_owned(), ParamValue::Integer(3));
        let source = well_magnitude_source("audio_hum_removal").expect("the hum card has a well");
        let magnitudes = eq_well_magnitudes(&effects[0], TimeCode::ZERO, source);
        assert_eq!(magnitudes.len(), EQ_WELL_SAMPLES);
        for (index, decibels) in magnitudes.iter().enumerate() {
            let expected = kinewright_media::hum_removal_magnitude_db(
                &effects[0],
                TimeCode::ZERO,
                eq_well_hertz(index),
                EQ_WELL_SAMPLE_RATE,
            );
            assert!(
                (decibels - expected).abs() <= 1e-12,
                "sample {index} is the design's own answer: {decibels} vs {expected}"
            );
        }
        assert!(
            magnitudes.iter().any(|decibels| *decibels < -1.0),
            "a 40 dB notch shows in the comb"
        );
        let eq = automation_effects(&["audio_parametric_eq"]);
        let eq_source =
            well_magnitude_source("audio_parametric_eq").expect("the EQ card has a well");
        assert_eq!(
            eq_well_magnitudes(&eq[0], TimeCode::ZERO, eq_source),
            eq_well_magnitudes(
                &eq[0],
                TimeCode::ZERO,
                kinewright_media::parametric_eq_magnitude_db
            ),
            "the EQ card still draws the EQ response"
        );
        assert!(
            well_magnitude_source("audio_denoise").is_none(),
            "the denoise card has a bar chart, not a magnitude well"
        );
    }

    /// AU5 §7 B14 (§6.5 rule 131): the denoise card's height budget.
    ///
    /// group frame + header 31.5 + **two** wrapped control rows at 33.5 each,
    /// 39.5 marginal with their row spacing + noise well 48 + the
    /// `Learn profile` row 26, which is one bare `egui::Button` + reduction
    /// bar 6 + `item_spacing.y` 6 between each, and a further 18 px in the two
    /// refused states, where rule 128's sentence takes its own wrapped line:
    /// `31.5 + 6 + 73 + 6 + 48 + 6 + 26 + 6 + 6 = 208.5`. Rule 131's own
    /// arithmetic put a control row at 22 px and the group at 8 and left the
    /// `Learn` row out altogether; the budget therefore moves 160 → 240 with
    /// the measurements recorded beside it, as an AU5 §0 erratum (R120).
    ///
    /// All three learn states are pinned, and the refused ones are the tall
    /// ones: the sentence cannot sit beside the button, because a
    /// 49-character label in a `ui.horizontal` pushes the pane's 400 px
    /// column wider than its own token (AU5 §0 R132).
    #[test]
    fn au5_the_denoise_card_fits_its_height_budget() {
        const BUDGET: f32 = 240.0;
        /// The `Span` state: the button alone, no sentence.
        const MEASURED_LEARNABLE: f32 = 208.5;
        /// Either refusal: the button plus rule 128's own wrapped line.
        const MEASURED_REFUSED: f32 = 226.5;
        let bus = repair_bus(&["audio_denoise"]);
        for (label, learn, measured) in [
            ("analysing", NoiseLearnRange::Analysing, MEASURED_REFUSED),
            ("no silence", NoiseLearnRange::NoSilence, MEASURED_REFUSED),
            (
                "learnable",
                NoiseLearnRange::Span(TimeCode(0), TimeCode(60)),
                MEASURED_LEARNABLE,
            ),
        ] {
            let height = measure_card(&bus, 0, learn);
            println!("AU5_DENOISE_CARD case=\"{label}\" px={height}");
            assert!(
                height <= BUDGET,
                "the {label} denoise card is {height} px tall, over the {BUDGET} px budget"
            );
            assert!(
                (height - measured).abs() <= 1.0,
                "the doc comment says the {label} denoise card measures {measured} px; \
                 it measured {height}"
            );
        }
    }

    /// AU5 §7 B14 (§6.5 rule 131): the hum card's height budget.
    ///
    /// group frame + header 31.5 + two wrapped control rows at 33.5 each +
    /// comb 96 + `item_spacing.y` 6 between each:
    /// `31.5 + 6 + 73 + 6 + 96 = 212.5`. Still the tallest **card** of the
    /// three; the budget moves 190 → 220 for the same reason the denoise
    /// card's does (AU5 §0 R120).
    #[test]
    fn au5_the_hum_card_fits_its_height_budget() {
        const BUDGET: f32 = 220.0;
        const MEASURED: f32 = 212.5;
        let bus = repair_bus(&["audio_hum_removal"]);
        let height = measure_card(&bus, 0, NoiseLearnRange::default());
        println!("AU5_HUM_CARD px={height}");
        assert!(
            height <= BUDGET,
            "the hum card is {height} px tall, over the {BUDGET} px budget"
        );
        assert!(
            (height - MEASURED).abs() <= 1.0,
            "the doc comment says the hum card measures {MEASURED} px; it measured {height}"
        );
    }

    /// AU5 §7 B14 (§6.5 rule 131): the de-click card's height budget.
    ///
    /// group frame + header 31.5 + `item_spacing.y` 6 + one wrapped control
    /// row 33.5 = 71.0. This card carries **no** AU5 geometry at all — no
    /// well, no learn row, no reduction bar — and still overruns rule 131's
    /// 70, which is what identifies the shortfall as the contract's estimate
    /// rather than slack in the implementation; the budget moves 70 → 80
    /// (AU5 §0 R120).
    #[test]
    fn au5_the_declick_card_fits_its_height_budget() {
        const BUDGET: f32 = 80.0;
        const MEASURED: f32 = 71.0;
        let bus = repair_bus(&["audio_declick"]);
        let height = measure_card(&bus, 0, NoiseLearnRange::default());
        println!("AU5_DECLICK_CARD px={height}");
        assert!(
            height <= BUDGET,
            "the de-click card is {height} px tall, over the {BUDGET} px budget"
        );
        assert!(
            (height - MEASURED).abs() <= 1.0,
            "the doc comment says the de-click card measures {MEASURED} px; it measured {height}"
        );
    }

    /// Lay a whole bus pane out at the pane's width and report its content
    /// height — what the 400 px column has to scroll (AU5 §0 R47).
    fn measure_repair_bus_pane(
        document: &Document,
        expanded: EffectId,
        learn: NoiseLearnRange,
    ) -> f32 {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let levels = MixerMeterLevels::default();
        let mut edits = MixerChainEdits::default();
        let mut requested = None;
        let mut measured = egui::Vec2::ZERO;
        let bus = &document.audio_mix.buses[0];
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            // Which card is expanded is egui memory, so it is set here rather
            // than clicked: the pane is being measured, not driven.
            expand_card(ui, MixerSelection::Bus(bus.id), expanded);
            ui.vertical(|ui| {
                ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
                let mut learn = NoiseLearn {
                    range: learn,
                    requested: &mut requested,
                };
                bus_pane(
                    ui,
                    document,
                    bus,
                    &levels,
                    TimeCode::ZERO,
                    &mut edits,
                    &mut learn,
                );
                measured = ui.min_rect().size();
            });
        });
        measured.y
    }

    /// A document whose one bus carries the whole repair prefix.
    fn repair_bus_document() -> Document {
        let mut document = Document {
            duration: TimeCode(300),
            ..Document::default()
        };
        document.audio_mix.buses.push(repair_bus(&[
            "audio_denoise",
            "audio_hum_removal",
            "audio_declick",
        ]));
        document
    }

    /// AU5 §7 B14 (§6.5 rule 131, R47): the bus pane's **content** budget.
    ///
    /// Not a fit budget: `chain_pane` opens with a `ScrollArea`, so an overrun
    /// costs scroll distance and never clipping.
    ///
    /// Two arms, because R46 and the product disagree about which case is the
    /// tallest. R46 fixes the measurement at the **hum** card expanded, the
    /// tallest of the three cards at 212.5 px, and that arm reads **522.5**.
    /// But `expanded_card` expands the **first** node when nothing is
    /// remembered, which on the repair prefix is the **denoise** card, and its
    /// default learn state is `Analysing` — so the pane an editor meets on
    /// first paint carries the 226.5 px refused denoise card and reads
    /// **536.5**. That is the default *and* the worst case, so it is the one
    /// the budget is set above (AU5 §0 R121).
    #[test]
    fn au5_the_repair_bus_pane_fits_its_content_budget() {
        const BUDGET: f32 = 540.0;
        /// R46's case: the hum card, the tallest of the three cards.
        const MEASURED_HUM: f32 = 522.5;
        /// The product's own default: the first node, in its default state.
        const MEASURED_DEFAULT: f32 = 536.5;
        let document = repair_bus_document();
        let effects = &document.audio_mix.buses[0].effects;
        assert_eq!(
            effects[0].name, "audio_denoise",
            "`expanded_card` expands the first node, which is the denoise card"
        );
        assert_eq!(effects[1].name, "audio_hum_removal");
        for (label, expanded, learn, measured) in [
            (
                "denoise expanded, analysing (default)",
                effects[0].id,
                NoiseLearnRange::Analysing,
                MEASURED_DEFAULT,
            ),
            (
                "hum expanded",
                effects[1].id,
                NoiseLearnRange::default(),
                MEASURED_HUM,
            ),
        ] {
            let height = measure_repair_bus_pane(&document, expanded, learn);
            println!("AU5_REPAIR_BUS_PANE case=\"{label}\" px={height}");
            assert!(
                height <= BUDGET,
                "the repair bus pane ({label}) is {height} px of content, over the \
                 {BUDGET} px budget"
            );
            assert!(
                (height - measured).abs() <= 1.0,
                "the doc comment says the repair bus pane ({label}) measures {measured} px; \
                 it measured {height}"
            );
        }
    }
}

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
    AudioBus, AudioChain, AudioMaster, CHAIN_LOOKAHEAD_MILLISECONDS, Document, Effect, EffectId,
    LoudnessSnapshot, LoudnessTarget, PanLaw, ParamValue, TimeCode, TrackId,
    chain_lookahead_milliseconds, effect_descriptor,
};

use crate::{
    inspector_ui::{effect_display_name, is_live_drag},
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
}

impl<'a> MixerChain<'a> {
    pub(crate) const fn selection(self) -> MixerSelection {
        match self {
            Self::Bus(bus) => MixerSelection::Bus(bus.id),
            Self::Master(_) => MixerSelection::Master,
        }
    }

    const fn effects(self) -> &'a [Effect] {
        match self {
            Self::Bus(bus) => bus.effects.as_slice(),
            Self::Master(master) => master.effects.as_slice(),
        }
    }

    /// The sidechain sources this chain can duck from. The master has none,
    /// which is why `audio_ducking` is rejected there outright.
    const fn sidechain(self) -> &'a [TrackId] {
        match self {
            Self::Bus(bus) => bus.ducking_sidechain_tracks.as_slice(),
            Self::Master(_) => &[],
        }
    }

    /// The edited copy of this chain's effect list.
    fn effects_mut(self, edits: &mut MixerChainEdits) -> &mut Vec<Effect> {
        match self {
            Self::Bus(bus) => &mut edits.bus(bus).effects,
            Self::Master(master) => &mut edits.master(master).effects,
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
pub(crate) fn chain_pane(
    ui: &mut egui::Ui,
    document: &Document,
    selection: MixerSelection,
    levels: &MixerMeterLevels,
    snapshot: LoudnessSnapshot,
    target: LoudnessTarget,
    edits: &mut MixerChainEdits,
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
        egui::ScrollArea::vertical()
            .id_salt("mixer-chain-pane")
            .max_width(size::MIXER_CHAIN_PANE_WIDTH)
            .show(ui, |ui| {
                ui.set_max_width(size::MIXER_CHAIN_PANE_WIDTH);
                match selection {
                    MixerSelection::Bus(id) => {
                        if let Some(bus) = document.audio_mix.bus(id) {
                            bus_pane(ui, document, bus, levels, edits);
                        }
                        false
                    }
                    MixerSelection::Master => {
                        master_pane(ui, document, levels, snapshot, target, edits)
                    }
                }
            })
            .inner
    })
    .inner
}

fn bus_pane(
    ui: &mut egui::Ui,
    document: &Document,
    bus: &AudioBus,
    levels: &MixerMeterLevels,
    edits: &mut MixerChainEdits,
) {
    let chain = MixerChain::Bus(bus);
    pane_title(ui, &format!("Bus: {}", bus.name));
    routing_rows(ui, document, bus, edits);
    sidechain_rows(ui, document, bus, edits);
    ui.separator();
    chain_cards(ui, chain, levels, edits);
    add_effect_menu(ui, chain, edits);
}

/// The master pane: the `LOUDNESS` section first, then the pan law and the
/// chain (AU3 §4.4). Returns whether `Reset` was clicked.
fn master_pane(
    ui: &mut egui::Ui,
    document: &Document,
    levels: &MixerMeterLevels,
    snapshot: LoudnessSnapshot,
    target: LoudnessTarget,
    edits: &mut MixerChainEdits,
) -> bool {
    let master = &document.audio_mix.master;
    let chain = MixerChain::Master(master);
    pane_title(ui, "Master");
    let reset_loudness = loudness_section(ui, snapshot, target);
    pan_law_rows(ui, document.audio_mix.pan_law, edits);
    ui.separator();
    chain_cards(ui, chain, levels, edits);
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
/// The muted sentence that closes the section (AU3 §4.4, F14/F21). Part B
/// appends the export step's half.
pub(crate) const LOUDNESS_MONITORING_NOTE: &str =
    "monitoring is not delivery: playback is never normalised";

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
    edits: &mut MixerChainEdits,
) {
    let effects = chain.effects();
    if effects.is_empty() {
        ui.colored_label(color::TEXT_MUTED, "No effects on this chain yet.");
        return;
    }
    let expanded = expanded_card(ui, chain.selection(), effects);
    for (index, effect) in effects.iter().enumerate() {
        chain_card(ui, chain, index, expanded == Some(effect.id), levels, edits);
    }
}

/// The id of the expanded-card memory slot for one chain.
fn expanded_memory_id(selection: MixerSelection) -> egui::Id {
    match selection {
        MixerSelection::Bus(bus) => egui::Id::new(("mixer-expanded-card", "bus", bus.0)),
        MixerSelection::Master => egui::Id::new(("mixer-expanded-card", "master", 0_u64)),
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
/// A collapsed card carries no frame, because a `ui.group`'s margins and
/// stroke alone are 8 px and the collapsed row's whole budget is `ICON_BUTTON`
/// (AU2 §6.7). The expanded card takes the frame, where the grouping is what
/// tells a wrapped row of controls from the row below it.
pub(crate) fn chain_card(
    ui: &mut egui::Ui,
    chain: MixerChain,
    index: usize,
    expanded: bool,
    levels: &MixerMeterLevels,
    edits: &mut MixerChainEdits,
) {
    if expanded {
        ui.group(|ui| {
            card_header(ui, chain, index, expanded, levels, edits);
            card_body(ui, chain, index, levels, edits);
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

            let mut bypassed = parameter_value(effect, "bypass") == 1;
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
                let reduction = levels.reduction(chain.selection().chain(), effect.id);
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

fn card_body(
    ui: &mut egui::Ui,
    chain: MixerChain,
    index: usize,
    levels: &MixerMeterLevels,
    edits: &mut MixerChainEdits,
) {
    let effect = &chain.effects()[index];
    let Some(descriptor) = effect_descriptor(&effect.name) else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        for parameter in descriptor.parameters {
            if parameter.name == "bypass" {
                continue;
            }
            mixer_parameter_control(
                ui,
                chain,
                effect,
                parameter.name,
                &parameter_label(parameter.name),
                mixer_unit(parameter.name),
                parameter_value(effect, parameter.name),
                edits,
            );
        }
    });
    if effect.name == "audio_parametric_eq" {
        eq_well(ui, effect);
    }
    if has_gain_computer(&effect.name) {
        reduction_bar(ui, chain.selection().chain(), effect.id, levels);
    }
}

/// The stored value of one parameter, or its descriptor neutral.
pub(crate) fn parameter_value(effect: &Effect, name: &str) -> i64 {
    effect.static_integer_parameter(name).unwrap_or_else(|| {
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

/// Whether this node reports gain reduction (AU2 §3.8).
pub(crate) fn has_gain_computer(name: &str) -> bool {
    matches!(
        name,
        "audio_compressor" | "audio_ducking" | "audio_gate" | "audio_true_peak_limiter"
    )
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
    /// A registered name in none of the units above. No audio descriptor has
    /// one today; the variant keeps an unrecognised control readable instead
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
        "threshold_tenth_db" => "Threshold",
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

/// The node's own magnitude at each sampled frequency, in decibels.
pub(crate) fn eq_well_magnitudes(effect: &Effect) -> Vec<f64> {
    (0..EQ_WELL_SAMPLES)
        .map(|index| {
            kinewright_media::parametric_eq_magnitude_db(
                effect,
                TimeCode::ZERO,
                eq_well_hertz(index),
                EQ_WELL_SAMPLE_RATE,
            )
        })
        .collect()
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
pub(crate) fn eq_well_points(effect: &Effect, rect: egui::Rect) -> Vec<egui::Pos2> {
    eq_well_magnitudes(effect)
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

/// The read-only magnitude well of an expanded parametric EQ (AU2 §6.7).
fn eq_well(ui: &mut egui::Ui, effect: &Effect) {
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
        eq_well_points(effect, rect),
        egui::Stroke::new(1.6, color::TEXT_PRIMARY),
    ));
    response.on_hover_text(EQ_WELL_TOOLTIP);
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

/// The six nodes the mixer's `+ Effect` menu offers, in menu order (AU2 §6.8).
///
/// `audio_eq` and `audio_limiter` are retained, valid, and processed, but a
/// new chain never grows one: the parametric EQ and the true-peak limiter
/// replace them.
pub(crate) const INSERTABLE_AUDIO_EFFECTS: [&str; 6] = [
    "audio_gain",
    "audio_parametric_eq",
    "audio_compressor",
    "audio_gate",
    "audio_ducking",
    "audio_true_peak_limiter",
];

/// Whether the mixer's `+ Effect` menu offers one audio effect (AU2 §6.8).
pub(crate) fn is_audio_effect_insertable(name: &str) -> bool {
    INSERTABLE_AUDIO_EFFECTS.contains(&name)
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
                is_audio_effect_insertable(name),
                "the menu offers only the six nodes of AU2 §6.8"
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

    /// AU3 §7 A18: the section spends at most 80 px of the pane, measured
    /// with every value present and with none — the pane scrolls, but the
    /// section should be on screen before the first card in a 260 px dock.
    /// Measured on this build: 74 px either way.
    #[test]
    fn the_loudness_section_fits_its_height_budget() {
        const BUDGET: f32 = 80.0;
        const MEASURED: f32 = 74.0;
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
}

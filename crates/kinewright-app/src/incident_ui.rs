//! The incident card: one pure view builder and one paint function.
//!
//! IN1 §5.3 splits the card in two. [`incident_card`] takes **no** `egui::Ui`
//! argument, touches no `egui` type, and is the only tested half;
//! [`show_incident_card`] consumes the view it returns and is untested, exactly
//! as every other app surface in this repository is.
//!
//! The card never builds a [`RecoveryAction`] itself. Everything it offers for
//! an `AutoApply` or `AskFirst` incident is the `Vec<RecoveryAction>` that
//! `kinewright_core::policy_recovery` produced at observe time, which is what
//! makes IN1 §9 clause 6's equality assertion — "never below the button" —
//! meaningful rather than aspirational.

use eframe::egui;
use kinewright_core::{
    Incident, IncidentCode, IncidentOutcome, IncidentSeverity, IncidentState, IncidentSubject,
    Operation, PolicyClass, RecoveryAction, RecoveryKind, SourceColorIncident, explain_body,
    recovery_description,
};

use crate::theme::{self, color, space, type_size};

/// The button text of the card's own control (IN1 §0.1 B4, option (b)).
///
/// Global `Undo` is deliberately **not** the card's control: `Command::Undo`
/// restores whatever is on top of the single global stack, which after one
/// further edit is the person's edit rather than the assumption.
pub(crate) const REVERT_LABEL: &str = "Revert to probed description";

/// Everything the Media panel needs to draw one incident, and nothing that
/// needs a `Document` or an `egui::Ui` to compute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncidentCardView {
    /// How badly the problem affects the person's work.
    pub(crate) severity: IncidentSeverity,
    /// What Kinewright was allowed to do about it without asking.
    pub(crate) class: PolicyClass,
    /// The subject's short label, for example `Asset 1`.
    pub(crate) subject_label: String,
    /// One sentence saying what happened and why, in Kinewright's voice.
    pub(crate) headline: &'static str,
    /// `Open`, `Applied`, `Reverted` or `Explained`.
    pub(crate) state_label: &'static str,
    /// What the person may press, in the order they are drawn.
    pub(crate) actions: Vec<CardAction>,
    /// The typed facts, verbatim, in IN1 §5.3 rule 32's fixed key order.
    pub(crate) details: Vec<(&'static str, String)>,
}

/// One control on the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CardAction {
    /// Button text, always the recovery's own label.
    pub(crate) label: &'static str,
    /// Whether pressing it can succeed. A revert is disabled once the asset has
    /// nothing left to revert to: that operation would carry the probed
    /// provenance, fail both halves of the operation guard's admission test and
    /// be refused outright — a hard rejection from a button the card would
    /// otherwise advertise as live (IN1 §5.3 rule 29).
    pub(crate) enabled: bool,
    /// The recovery itself, equal to the one `policy_recovery` returns for
    /// every `AutoApply` and `AskFirst` row (IN1 §5.3 rule 31).
    pub(crate) recovery: RecoveryAction,
}

/// Build the card for one incident.
///
/// `assumed_from_present` is the one document fact the card needs and the
/// incident cannot carry: whether the subject asset still has something to
/// revert to. Passing it keeps this function pure and egui-free.
#[must_use]
pub(crate) fn incident_card(incident: &Incident, assumed_from_present: bool) -> IncidentCardView {
    IncidentCardView {
        severity: incident.severity,
        class: incident.class,
        subject_label: incident.subject.label(),
        headline: incident_headline(incident.code, incident.class),
        state_label: state_label(incident.state),
        actions: card_actions(incident, assumed_from_present),
        details: card_details(incident, assumed_from_present),
    }
}

/// Which outcome pressing one action records.
///
/// The card's own control restores the bytes the incident captured at open
/// time, so pressing it resolves `Reverted`; anything else the card offers is
/// a recovery being applied.
#[must_use]
pub(crate) fn card_action_outcome(incident: &Incident, action: &CardAction) -> IncidentOutcome {
    match &action.recovery.kind {
        RecoveryKind::Operation(Operation::SetAssetColorDescription {
            color_description, ..
        }) if incident
            .evidence
            .probed()
            .is_some_and(|probed| color_description == probed) =>
        {
            IncidentOutcome::Reverted
        }
        RecoveryKind::Operation(_) => IncidentOutcome::Applied,
        RecoveryKind::Explain(_) => IncidentOutcome::Explained,
    }
}

/// The pinned headline for one code under one resolved class.
///
/// `&'static str` on purpose: a headline cannot embed a path, a file name or a
/// measured number, which is what makes "the card headline is identical for
/// both containers" a type-level fact rather than a hope. The asset identity
/// lives in `subject_label` and the probed tuple in `details`.
///
/// The table has **fifteen** rows for twelve codes because the three
/// `Rec709Compatible` codes are reachable under both `AutoApply` and `Explain`;
/// the other nine are reachable only as `Explain`. The match carries no
/// wildcard arm in either position, so a new code or a new class breaks the
/// build (IN1 §5.3 rules 25–27).
#[must_use]
pub(crate) fn incident_headline(code: IncidentCode, class: PolicyClass) -> &'static str {
    // `IN1b` §3.8 break 4: `IncidentCode` has 67 variants after Part B and the
    // 70-row table of `IN1b` §5.5 rule 30 is implementer C's. Until it lands,
    // a non-colour code takes core's own written body, which is a
    // `&'static str` that names what must change; Part A's fifteen colour rows
    // below are unchanged byte for byte, so IN1 §9 clause 3's pinned headline
    // still holds.
    let IncidentCode::SourceColor(incident) = code else {
        return explain_body(code);
    };
    match (incident, class) {
        (SourceColorIncident::UnknownPrimaries, PolicyClass::AutoApply) => {
            "Kinewright assumed Rec.709 for this source because its colour primaries were unknown."
        }
        (SourceColorIncident::UnknownTransfer, PolicyClass::AutoApply) => {
            "Kinewright assumed Rec.709 for this source because its colour transfer was unknown."
        }
        (SourceColorIncident::UnknownMatrix, PolicyClass::AutoApply) => {
            "Kinewright assumed Rec.709 for this source because its colour matrix was unknown."
        }
        (SourceColorIncident::UnknownPrimaries, PolicyClass::AskFirst | PolicyClass::Explain) => {
            "This source does not say what colour primaries it uses, and the rest of its colour metadata rules out Rec.709."
        }
        (SourceColorIncident::UnknownTransfer, PolicyClass::AskFirst | PolicyClass::Explain) => {
            "This source does not say what colour transfer it uses, and the rest of its colour metadata rules out Rec.709."
        }
        (SourceColorIncident::UnknownMatrix, PolicyClass::AskFirst | PolicyClass::Explain) => {
            "This source does not say what colour matrix it uses, and the rest of its colour metadata rules out Rec.709."
        }
        (
            SourceColorIncident::UnknownRange,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => "This source does not say whether its levels are full or limited.",
        (
            SourceColorIncident::UnknownBitDepth,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => "This source does not say how many bits per sample it uses.",
        (
            SourceColorIncident::UnsupportedPrimaries,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => "This source's colour primaries are not ones Kinewright can manage yet.",
        (
            SourceColorIncident::UnsupportedTransfer,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => "This source's colour transfer is not one Kinewright can manage yet.",
        (
            SourceColorIncident::UnsupportedMatrix,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => "This source's colour matrix is not one Kinewright can manage yet.",
        (
            SourceColorIncident::UnsupportedRange,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => "This source's level range is not one Kinewright can manage yet.",
        (
            SourceColorIncident::UnsupportedWhitePoint,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => "This source's white point is not one Kinewright can manage yet.",
        (
            SourceColorIncident::UnsupportedBitDepth,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => "This source's bit depth is not one Kinewright can manage yet.",
        (
            SourceColorIncident::UnsupportedCombination,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => {
            "This source's colour metadata does not add up to a profile Kinewright can manage yet."
        }
        // The thirteenth code (`IN1b` §3.2 rule 16).
        (
            SourceColorIncident::UnknownWhitePoint,
            PolicyClass::AutoApply | PolicyClass::AskFirst | PolicyClass::Explain,
        ) => "This source does not say what white point it was graded against.",
    }
}

/// `Open`, `Applied`, `Reverted` or `Explained`, from the state alone.
const fn state_label(state: IncidentState) -> &'static str {
    match state {
        IncidentState::Open => "Open",
        IncidentState::Resolved(IncidentOutcome::Applied) => "Applied",
        IncidentState::Resolved(IncidentOutcome::Reverted) => "Reverted",
        IncidentState::Resolved(IncidentOutcome::Explained) => "Explained",
    }
}

/// The card's controls.
///
/// An applied auto-apply offers exactly one thing: put the probed description
/// back. Everything else offers exactly what `policy_recovery` returned, one
/// `CardAction` per entry, in order, enabled for an operation and disabled for
/// an explanation (IN1 §5.3 rules 29–30).
fn card_actions(incident: &Incident, assumed_from_present: bool) -> Vec<CardAction> {
    // `IN1b` §3.8 break 3: only an asset-scoped incident carrying a probed
    // description has something to revert to; every other incident falls
    // through to the `policy_recovery` branch below.
    if incident.class == PolicyClass::AutoApply
        && incident.state == IncidentState::Resolved(IncidentOutcome::Applied)
        && let (IncidentSubject::Asset(asset), Some(probed)) =
            (incident.subject, incident.evidence.probed())
    {
        return vec![CardAction {
            label: REVERT_LABEL,
            enabled: assumed_from_present,
            recovery: RecoveryAction {
                label: REVERT_LABEL,
                kind: RecoveryKind::Operation(Operation::SetAssetColorDescription {
                    asset,
                    color_description: probed.clone(),
                }),
            },
        }];
    }
    incident
        .recoveries
        .iter()
        .map(|recovery| CardAction {
            label: recovery.label,
            enabled: matches!(recovery.kind, RecoveryKind::Operation(_)),
            recovery: recovery.clone(),
        })
        .collect()
}

/// The typed fields verbatim, in IN1 §5.3 rule 32's fixed order.
///
/// The `assumed` row carries the description the recovery wrote, which is
/// `recovery_description` of the probed tuple by construction: it is the only
/// honest "after" to set beside the `probed` "before", and the card has no
/// document to read the live description from.
fn card_details(incident: &Incident, assumed_from_present: bool) -> Vec<(&'static str, String)> {
    let mut details = vec![
        ("code", incident.code.code().to_owned()),
        ("field", incident.field.to_owned()),
        ("observed", incident.observed.clone()),
        ("allowed", incident.allowed.clone().unwrap_or_default()),
    ];
    // `IN1b` §3.4 rule 25 row 7: the `probed` and `assumed` rows exist only
    // where the evidence carries a probed description.
    if let Some(probed) = incident.evidence.probed() {
        details.push(("probed", format!("{probed:?}")));
        if assumed_from_present {
            details.push(("assumed", format!("{:?}", recovery_description(probed))));
        }
    }
    details.push(("revision", incident.revision.to_string()));
    details.push(("seen", incident.count.to_string()));
    details
}

/// Paint one card and report which action the person pressed.
///
/// Untested by design (IN1 §5.3 rule 23): every assertion lives on
/// [`incident_card`], and the placement of the card in the Media panel is not
/// proven at all.
pub(crate) fn show_incident_card(ui: &mut egui::Ui, view: &IncidentCardView) -> Option<usize> {
    let mut pressed = None;
    theme::card_frame(false).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(view.subject_label.as_str())
                    .font(theme::semibold(type_size::CAPTION)),
            );
            ui.colored_label(color::TEXT_MUTED, view.state_label);
        });
        ui.colored_label(severity_color(view.severity), view.headline);
        for (key, value) in &view.details {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(color::TEXT_MUTED, *key);
                ui.label(value);
            });
        }
        ui.add_space(space::ONE);
        ui.horizontal_wrapped(|ui| {
            for (index, action) in view.actions.iter().enumerate() {
                match &action.recovery.kind {
                    RecoveryKind::Operation(_) => {
                        if ui
                            .add_enabled(action.enabled, egui::Button::new(action.label))
                            .on_disabled_hover_text(
                                "There is nothing left to revert to on this asset.",
                            )
                            .clicked()
                        {
                            pressed = Some(index);
                        }
                    }
                    RecoveryKind::Explain(body) => {
                        ui.colored_label(color::TEXT_SECONDARY, *body);
                    }
                }
            }
        });
    });
    pressed
}

/// The colour one severity paints its headline in.
const fn severity_color(severity: IncidentSeverity) -> egui::Color32 {
    match severity {
        IncidentSeverity::Blocks => color::STATUS_DANGER,
        IncidentSeverity::Degrades => color::STATUS_WARNING,
        IncidentSeverity::Informs => color::TEXT_SECONDARY,
    }
}

#[cfg(test)]
mod tests {
    use kinewright_core::{
        AssetId, ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance,
        ColorRange, ColorTransfer, ColorWhitePoint, IncidentEvidence, IncidentLog,
        IncidentObservation, Observed, POLICY, TimelineRevision, policy_recovery,
    };

    use super::*;

    /// IN1 §3 rule 5 row 1: the pinned `in1_untagged.mp4` probe, which is the
    /// canonical evidence for every `POLICY` row (IN1 §9 clauses 2 and 6).
    fn untagged_mp4_probe() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Unknown,
            transfer: ColorTransfer::Unknown,
            matrix: ColorMatrix::Unknown,
            range: ColorRange::Unknown,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 2_000,
            provenance: ColorProvenance::Inferred,
        }
    }

    fn observation(code: IncidentCode, probed: &ColorDescription) -> IncidentObservation {
        IncidentObservation {
            code,
            subject: IncidentSubject::Asset(AssetId(1)),
            observed: "unknown".to_owned(),
            allowed: Some("bt709 or srgb in a supported CC1 profile".to_owned()),
            evidence: IncidentEvidence::SourceColor {
                probed: probed.clone(),
                assumption: None,
            },
            revision: TimelineRevision(1),
        }
    }

    /// Open one incident for `code` on the pinned probe and hand back the log
    /// that owns it, so nothing outside `incident.rs` ever builds an
    /// `Incident` by hand.
    fn opened(
        code: IncidentCode,
        probed: &ColorDescription,
    ) -> (IncidentLog, kinewright_core::IncidentId) {
        let mut log = IncidentLog::default();
        let Observed::Opened(id) = log.observe(observation(code, probed)) else {
            panic!("a fresh log opens the first observation");
        };
        (log, id)
    }

    #[test]
    fn in1_the_untagged_primaries_card_pins_its_headline_and_its_class() {
        let probed = untagged_mp4_probe();
        let (log, id) = opened(
            IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries),
            &probed,
        );
        let incident = log.get(id).expect("the log keeps what it opened");
        assert_eq!(incident.class, PolicyClass::AutoApply);
        let view = incident_card(incident, false);
        assert_eq!(
            view.headline,
            "Kinewright assumed Rec.709 for this source because its colour primaries were unknown."
        );
        assert_eq!(view.subject_label, "Asset 1");
        assert_eq!(view.state_label, "Open");
        assert_eq!(view.severity, IncidentSeverity::Blocks);
        assert_eq!(view.class, PolicyClass::AutoApply);
    }

    #[test]
    fn in1_the_headline_is_identical_for_both_containers() {
        let mp4 = untagged_mp4_probe();
        let webm = ColorDescription {
            range: ColorRange::Limited,
            confidence_basis_points: 4_000,
            provenance: ColorProvenance::StreamMetadata,
            ..untagged_mp4_probe()
        };
        assert_ne!(mp4, webm, "the two pinned probes differ in three fields");
        let code = IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries);
        let (mp4_log, mp4_id) = opened(code, &mp4);
        let (webm_log, webm_id) = opened(code, &webm);
        assert_eq!(
            incident_card(mp4_log.get(mp4_id).expect("open"), false).headline,
            incident_card(webm_log.get(webm_id).expect("open"), false).headline
        );
    }

    #[test]
    fn in1_a_known_bt2020_probe_reads_the_explain_headline_instead() {
        let probed = ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            ..untagged_mp4_probe()
        };
        let (log, id) = opened(
            IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries),
            &probed,
        );
        let incident = log.get(id).expect("open");
        assert_eq!(incident.class, PolicyClass::Explain);
        assert_eq!(
            incident_card(incident, false).headline,
            "This source does not say what colour primaries it uses, and the rest of its colour metadata rules out Rec.709."
        );
    }

    #[test]
    fn in1_every_headline_row_is_distinct_and_covers_the_whole_table() {
        let mut headlines = Vec::new();
        for entry in POLICY {
            headlines.push(incident_headline(entry.code, PolicyClass::Explain));
            if entry.class == PolicyClass::AutoApply {
                headlines.push(incident_headline(entry.code, PolicyClass::AutoApply));
            }
        }
        // Seventy rows for 67 codes, because the three `Rec709Compatible` codes
        // are reachable under both classes (`IN1b` §5.5 rule 30). Part A's
        // fifteen colour rows are unchanged; the other 54 take core's written
        // body through `incident_headline`'s shim until implementer C writes
        // the per-code headline table.
        assert_eq!(headlines.len(), 70, "seventy rows for sixty-seven codes");
        let mut sorted = headlines.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), headlines.len(), "every headline is distinct");
    }

    #[test]
    fn in1_no_card_action_sits_below_the_button_policy_declared() {
        let probed = untagged_mp4_probe();
        for entry in POLICY {
            if !matches!(entry.class, PolicyClass::AutoApply | PolicyClass::AskFirst) {
                continue;
            }
            let (log, id) = opened(entry.code, &probed);
            let incident = log.get(id).expect("open");
            let view = incident_card(incident, false);
            let expected = policy_recovery(entry.code, incident.subject, &incident.evidence);
            let offered = view
                .actions
                .iter()
                .map(|action| action.recovery.clone())
                .collect::<Vec<_>>();
            assert_eq!(
                offered,
                expected,
                "{} must offer exactly what policy_recovery returns",
                entry.code.code()
            );
        }
    }

    #[test]
    fn in1_an_explain_row_offers_the_core_sentence_and_nothing_to_press() {
        let probed = untagged_mp4_probe();
        let (log, id) = opened(
            IncidentCode::SourceColor(SourceColorIncident::UnknownRange),
            &probed,
        );
        let incident = log.get(id).expect("open");
        let view = incident_card(incident, false);
        assert_eq!(view.actions.len(), 1);
        assert_eq!(view.actions[0].label, "How to fix this");
        assert!(!view.actions[0].enabled);
        assert!(matches!(
            view.actions[0].recovery.kind,
            RecoveryKind::Explain(_)
        ));
        assert_eq!(
            view.actions,
            policy_recovery(incident.code, incident.subject, &incident.evidence)
                .into_iter()
                .map(|recovery| CardAction {
                    label: recovery.label,
                    enabled: false,
                    recovery,
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn in1_an_applied_incident_offers_the_revert_and_disables_it_without_an_assumption() {
        let probed = untagged_mp4_probe();
        let (mut log, id) = opened(
            IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries),
            &probed,
        );
        assert!(log.resolve(id, IncidentOutcome::Applied));
        let incident = log.get(id).expect("open");

        let live = incident_card(incident, true);
        assert_eq!(live.state_label, "Applied");
        assert_eq!(live.actions.len(), 1);
        assert_eq!(live.actions[0].label, REVERT_LABEL);
        assert!(live.actions[0].enabled);
        assert_eq!(
            live.actions[0].recovery.kind,
            RecoveryKind::Operation(Operation::SetAssetColorDescription {
                asset: AssetId(1),
                color_description: probed.clone(),
            }),
            "the revert sends the captured bytes back, which is the guard's byte-equality case"
        );
        assert_eq!(
            card_action_outcome(incident, &live.actions[0]),
            IncidentOutcome::Reverted
        );

        let after_undo = incident_card(incident, false);
        assert!(
            !after_undo.actions[0].enabled,
            "a global Undo clears assumed_from, so the revert must grey out"
        );
    }

    #[test]
    fn in1_the_card_details_carry_the_typed_fields_in_order() {
        let probed = untagged_mp4_probe();
        let (log, id) = opened(
            IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries),
            &probed,
        );
        let incident = log.get(id).expect("open");

        let plain = incident_card(incident, false);
        let keys = plain
            .details
            .iter()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "code", "field", "observed", "allowed", "probed", "revision", "seen"
            ]
        );
        assert_eq!(plain.details[0].1, "unknown_source_primaries");
        assert_eq!(plain.details[1].1, "primaries");
        assert_eq!(plain.details[2].1, "unknown");
        assert_eq!(
            plain.details[3].1,
            "bt709 or srgb in a supported CC1 profile"
        );
        assert_eq!(plain.details[6].1, "1");

        let assumed = incident_card(incident, true);
        let keys = assumed
            .details
            .iter()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "code", "field", "observed", "allowed", "probed", "assumed", "revision", "seen"
            ]
        );
        assert_eq!(
            assumed.details[5].1,
            format!("{:?}", recovery_description(&probed))
        );
    }
}

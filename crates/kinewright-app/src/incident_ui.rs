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
    ColorQcIncident, DeliveryColorIncident, DeliveryVerificationIncident, INVESTIGATOR_ALLOWLIST,
    Incident, IncidentCode, IncidentFamily, IncidentOutcome, IncidentResolver, IncidentSeverity,
    IncidentState, IncidentSubject, LabelIncident, MediaIncident, Operation, PolicyClass,
    RecoveryAction, RecoveryKind, RejectionIncident, SourceColorIncident, recovery_description,
};

use crate::investigator::{
    InvestigatingCard, InvestigatorSessionPairs, ProposalAction, ProposalCardView,
    proposal_card_with_reinvestigate, shows_proposal_card,
};
use crate::theme::{self, color, space, type_size};

/// The button text of the card's own control (IN1 §0.1 B4, option (b)).
///
/// Global `Undo` is deliberately **not** the card's control: `Command::Undo`
/// restores whatever is on top of the single global stack, which after one
/// further edit is the person's edit rather than the assumption.
pub(crate) const REVERT_LABEL: &str = "Revert to probed description";

/// The stopped card's way back (IN2 §3.7 rule 39).
pub(crate) const REINVESTIGATE_LABEL: &str = "Re-investigate";

/// The per-code opt-out (IN2 §2.4 rule 14).
pub(crate) const NEVER_INVESTIGATE_LABEL: &str = "Never investigate this";

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
    /// IN2 §3.7 rule 39: a stopped investigation offers Re-investigate.
    ///
    /// Beside `actions` rather than in it: a `CardAction` is a stored
    /// recovery by construction, and the investigator's controls are not
    /// recoveries.
    pub(crate) reinvestigate: bool,
    /// IN2 §2.4 rule 14: an allowlisted code offers Never-investigate-this.
    pub(crate) never_investigate: bool,
}

/// What the person pressed on a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardPress {
    /// One of `view.actions`, by index.
    Action(usize),
    /// The Re-investigate button.
    Reinvestigate,
    /// The Never-investigate-this button.
    NeverInvestigate,
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
/// `revert_available` is the one document fact the card needs and the incident
/// cannot carry: whether the subject still has something to revert to. Passing
/// it keeps this function pure and egui-free.
///
/// `IN1b` §5.5 rule 28 split it out of Part A's `assumed_from_present`, which
/// was doing two jobs — gating the revert button *and* gating a `details` row
/// that prints the description the recovery wrote — under a name that only
/// described the second. The flag now means one thing, and the `assumed` row
/// moves behind the evidence: [`card_details`] prints it when the evidence
/// carries a probed description **and** a revert is available, which is
/// exactly the condition that made it true before and is now readable.
/// Colour supplies `asset.assumed_from.is_some()`; every other code supplies
/// `false` until it ships a revert of its own.
#[must_use]
pub(crate) fn incident_card(incident: &Incident, revert_available: bool) -> IncidentCardView {
    IncidentCardView {
        severity: incident.severity,
        class: incident.class,
        subject_label: incident.subject.label(),
        headline: incident_headline(incident.code, incident.class),
        state_label: state_label(incident.state),
        actions: card_actions(incident, revert_available),
        details: card_details(incident, revert_available),
        reinvestigate: incident.state == IncidentState::Open
            && matches!(
                incident.telemetry.resolver,
                Some(IncidentResolver::Session { .. })
            ),
        never_investigate: INVESTIGATOR_ALLOWLIST.contains(&incident.code),
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
/// The table has **seventy** rows for sixty-seven codes because the three
/// `Rec709Compatible` codes are reachable under both `AutoApply` and `Explain`;
/// the other sixty-four are reachable only as `Explain` (`IN1b` §5.5 rule 30).
/// The match carries no wildcard arm in either position, so a new code or a new
/// class breaks the build (IN1 §5.3 rules 25–27).
///
/// A headline says **what happened**. `kinewright_core::explain_body` says
/// what the person must change, and the two are never the same string:
/// `in1b_every_headline_row_is_distinct_and_covers_the_whole_table` asserts
/// that row by row, which is what stops a shim that returns the body from
/// passing the distinctness test for the wrong reason (erratum `IN1b`-A-R10).
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one arm per (code, class) row; `IN1b` §5.5 rule 30 requires the \
              table be exhaustive with no wildcard in either position, so the \
              length is the contract's and splitting it would hide the \
              exhaustiveness the type system is checking"
)]
pub(crate) fn incident_headline(code: IncidentCode, class: PolicyClass) -> &'static str {
    let incident = match code {
        IncidentCode::SourceColor(incident) => incident,
        IncidentCode::Media(incident) => return media_headline(incident),
        IncidentCode::DeliveryColor(incident) => return delivery_color_headline(incident),
        IncidentCode::DeliveryVerification(incident) => {
            return delivery_verification_headline(incident);
        }
        IncidentCode::ColorQc(incident) => return color_qc_headline(incident),
        IncidentCode::Operation(family) => return operation_headline(family),
        IncidentCode::LutAssetPolicy => {
            return "This LUT is not the file the project recorded for it.";
        }
        IncidentCode::EditRevisionConflict => {
            return "The timeline moved between planning this edit and sending it, so nothing was applied.";
        }
        IncidentCode::Rejection(incident) => return rejection_headline(incident),
        IncidentCode::Label(incident) => return label_headline(incident),
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

/// The two non-colour `Media` rows (`IN1b` §5.5 rule 30).
const fn media_headline(incident: MediaIncident) -> &'static str {
    match incident {
        MediaIncident::UnsupportedDecoderFormat => {
            "This file's pixel format is not one the managed renderer can prove it may decode."
        }
        MediaIncident::BackendUnclassified => {
            "The media engine refused this work and gave no code Kinewright can act on."
        }
    }
}

/// The four managed-delivery colour rows.
const fn delivery_color_headline(incident: DeliveryColorIncident) -> &'static str {
    match incident {
        DeliveryColorIncident::UnsupportedCodec => {
            "This export's video codec cannot carry managed delivery colour tags."
        }
        DeliveryColorIncident::UnsupportedField => {
            "One of this export's delivery colour fields is outside the managed set."
        }
        DeliveryColorIncident::PixelFormatDepthMismatch => {
            "The negotiated pixel format does not carry the bit depth this export declared."
        }
        DeliveryColorIncident::EncoderPixelFormatUnavailable => {
            "This build's encoder does not offer the pixel format the export needs."
        }
    }
}

/// The five delivery-verification rows. Every one is a **degraded** result:
/// the file was written and only the measurement is missing.
const fn delivery_verification_headline(incident: DeliveryVerificationIncident) -> &'static str {
    match incident {
        DeliveryVerificationIncident::NotFullResolution => {
            "The export was written, but its verification sampled a reduced-resolution render."
        }
        DeliveryVerificationIncident::PlaneOutOfContainer => {
            "The export was written, but a sampled plane does not fit the container it declared."
        }
        DeliveryVerificationIncident::FrameCountMismatch => {
            "The export was written, but it holds a different number of frames than the timeline."
        }
        DeliveryVerificationIncident::FrameCountOutOfRange => {
            "The export was written, but the number of frames asked for is outside the sampling range."
        }
        DeliveryVerificationIncident::BudgetLaneMismatch => {
            "The export was written, but its verification was asked for against another lane's budgets."
        }
    }
}

/// The six colour-QC rows. Each is a refused measurement that mutated nothing.
const fn color_qc_headline(incident: ColorQcIncident) -> &'static str {
    match incident {
        ColorQcIncident::ProxyProofRefused => {
            "A proxy picture was offered as the reference for a delivery measurement, and refused."
        }
        ColorQcIncident::RasterLengthMismatch => {
            "The picture handed to the scope does not hold as many samples as its size says."
        }
        ColorQcIncident::EmptyPopulation => {
            "The region this measurement was asked for covers no pixels."
        }
        ColorQcIncident::NodeBudgetExceeded => {
            "More colour nodes were asked for in one measurement than the budget allows."
        }
        ColorQcIncident::MatteRegionRasterMismatch => {
            "The coverage picture does not match the region it was measured against."
        }
        ColorQcIncident::NodeRemovalRejected => {
            "The temporary node this measurement added could not be taken out again."
        }
    }
}

/// The eleven `OpError` family rows. Each says what **kind** of thing the
/// edit got wrong; the message itself names which one.
const fn operation_headline(family: IncidentFamily) -> &'static str {
    match family {
        IncidentFamily::Bounds => {
            "This edit was refused: a value it carries is outside the range the edit allows."
        }
        IncidentFamily::Malformed => {
            "This edit was refused: one of the fields it carries is missing, empty or the wrong kind."
        }
        IncidentFamily::Duplicate => {
            "This edit was refused: something with the same identity is already in the project."
        }
        IncidentFamily::Placement => {
            "This edit was refused: it was aimed at the wrong kind of thing."
        }
        IncidentFamily::Missing => {
            "This edit was refused: the thing it names is not in the project any more."
        }
        IncidentFamily::Structure => {
            "This edit was refused: something else in the project has to change first."
        }
        IncidentFamily::Relink => {
            "This edit was refused: the replacement media could not be matched to the clip it replaces."
        }
        IncidentFamily::Unrepresentable => {
            "This edit was refused: it cannot be expressed on this project's whole-frame grid."
        }
        IncidentFamily::UnknownName => {
            "This edit was refused: Kinewright knows nothing by the name it used."
        }
        IncidentFamily::Internal => {
            "Kinewright itself stopped part-way through this edit. Nothing you did caused it."
        }
        IncidentFamily::ColorPolicy => {
            "This colour override was refused: it is not one Kinewright will write."
        }
    }
}

/// The seven typed-rejection rows.
const fn rejection_headline(incident: RejectionIncident) -> &'static str {
    match incident {
        RejectionIncident::EditPlan => {
            "This edit plan was refused as a whole, so nothing in it was applied."
        }
        RejectionIncident::DeliveryVariant => {
            "The delivery variant could not be built from this project."
        }
        RejectionIncident::AgentBranch => "The isolated agent branch refused this request.",
        RejectionIncident::SourceEdit => {
            "Something the Source edit depended on changed while the source was being verified."
        }
        RejectionIncident::Relink => "This file cannot stand in for the asset it would replace.",
        RejectionIncident::ProjectSave => {
            "The project file could not be written, so this project is still only in memory."
        }
        RejectionIncident::CaptionPlan => "The caption track could not be planned from these cues.",
    }
}

/// The seventeen source-label rows. Fifteen are placeholders — the label's
/// failures carry no typed payload yet — and `look_incomplete` and
/// `media_incomplete` are not: their work finished with something missing,
/// which is why they are declared apart from their labels' blocking twins.
const fn label_headline(incident: LabelIncident) -> &'static str {
    match incident {
        LabelIncident::Operations => "This edit was not applied.",
        LabelIncident::Look => "The look or LUT could not be imported, restored or applied.",
        LabelIncident::LookIncomplete => {
            "The project was saved or opened, but not every look came with it."
        }
        LabelIncident::Export => "The export did not start, or did not finish.",
        LabelIncident::SourceMonitor => {
            "No edit was applied: the Source monitor could not act on what is loaded."
        }
        LabelIncident::Relink => "The relink could not go ahead.",
        LabelIncident::AgentBranch => {
            "The isolated branch this agent thread edits in could not be created or used."
        }
        LabelIncident::TranscriptEdit => "The transcript edit produced no change.",
        LabelIncident::Media => {
            "Playback, import or capture stopped, or there was nothing to play."
        }
        LabelIncident::MediaIncomplete => {
            "The project opened, but some of its media or its timeline pictures did not come with it."
        }
        LabelIncident::Agent => "The agent harness could not be reached, started, or spoken to.",
        LabelIncident::Recording => "The recording did not start, or stopped before it finished.",
        LabelIncident::Project => "The project could not be opened, created, read, or restored.",
        LabelIncident::Captions => "The captions could not be generated or saved.",
        LabelIncident::Mixer => "The mixer could not do what was asked.",
        LabelIncident::MediaCache => "The media cache could not be cleared.",
        LabelIncident::Timeline => "The timeline gesture did not complete.",
    }
}

/// `Open`, `Investigating`, `Applied`, `Reverted`, `Explained` or `Rejected`,
/// from the state alone.
///
/// Six arms since IN2 §3.6 rule 33: `Investigating` is a third open state and
/// `Rejected` is a fourth outcome.
const fn state_label(state: IncidentState) -> &'static str {
    match state {
        IncidentState::Open => "Open",
        IncidentState::Investigating => "Investigating",
        IncidentState::Resolved(IncidentOutcome::Applied) => "Applied",
        IncidentState::Resolved(IncidentOutcome::Reverted) => "Reverted",
        IncidentState::Resolved(IncidentOutcome::Explained) => "Explained",
        IncidentState::Resolved(IncidentOutcome::Rejected) => "Rejected",
    }
}

/// The card's controls.
///
/// An applied auto-apply offers exactly one thing: put the probed description
/// back. Everything else offers exactly what `policy_recovery` returned, one
/// `CardAction` per entry, in order, enabled for an operation and disabled for
/// an explanation (IN1 §5.3 rules 29–30).
fn card_actions(incident: &Incident, revert_available: bool) -> Vec<CardAction> {
    // `IN1b` §3.8 break 3 as re-keyed by IN2 N5.1: the revert branch keys on
    // `Resolved(Applied)` plus an asset subject plus probed evidence — not on
    // the stored class — now that three `Explain` rows carry a button of
    // their own. Every other incident falls through to the `policy_recovery`
    // branch below.
    if incident.state == IncidentState::Resolved(IncidentOutcome::Applied)
        && let (IncidentSubject::Asset(asset), Some(probed)) =
            (incident.subject, incident.evidence.probed())
    {
        return vec![CardAction {
            label: REVERT_LABEL,
            enabled: revert_available,
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
fn card_details(incident: &Incident, revert_available: bool) -> Vec<(&'static str, String)> {
    let mut details = vec![
        ("code", incident.code.code().to_owned()),
        ("field", incident.field.to_owned()),
        ("observed", incident.observed.clone()),
        ("allowed", incident.allowed.clone().unwrap_or_default()),
    ];
    // `IN1b` §3.4 rule 25 row 7 and §5.5 rule 28: the `probed` and `assumed`
    // rows exist only where the evidence carries a probed description, and the
    // `assumed` row additionally needs a revert to be available — it is the
    // description the revert would take back.
    if let Some(probed) = incident.evidence.probed() {
        details.push(("probed", format!("{probed:?}")));
        if revert_available {
            details.push(("assumed", format!("{:?}", recovery_description(probed))));
        }
    }
    details.push(("revision", incident.revision.to_string()));
    details.push(("seen", incident.count.to_string()));
    // IN2 §3.7 rule 39, N5.3: the stopped row exists only on an `Open` card
    // whose resolver is a finished session.
    if incident.state == IncidentState::Open
        && let Some(IncidentResolver::Session { stop, .. }) = incident.telemetry.resolver.as_ref()
    {
        details.push(("stopped", investigation_stopped_sentence(stop)));
    }
    details
}

/// Turn an internal stop/telemetry string into one calm sentence a person can
/// act on. The raw resolver remains available in telemetry for diagnosis, but
/// it never becomes UI copy (§3.7 rule 39, review fix F13).
fn investigation_stopped_sentence(stop: &str) -> String {
    let reason = if stop.contains("budget: turns") {
        "it ran out of turns"
    } else if stop.contains("budget: wall time") {
        "it ran out of time"
    } else if stop.contains("budget: tokens") {
        "it reached its token budget"
    } else if stop.contains("switched off") {
        "you switched investigation off"
    } else if stop.contains("confirmation") {
        "it asked for confirmation"
    } else if stop.contains("disconnected") {
        "the agent disconnected"
    } else if stop.contains("re-investigated") {
        "you asked to try again"
    } else {
        "the session stopped"
    };
    format!("Investigation stopped: {reason}. Press Re-investigate to try again.")
}

/// Group a count with ASCII spaces: `12400` reads `"12 400"` (IN2 §5.5
/// rule 14's `"12 400 of 40 000"`).
fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, (_, digit)) in digits.char_indices().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(' ');
        }
        grouped.push(digit);
    }
    grouped.chars().rev().collect()
}

/// Append the running session's counter rows to a card view (IN2 §5.5
/// rule 14): turns, tokens, and elapsed beside their budgets.
pub(crate) fn append_investigating_rows(view: &mut IncidentCardView, card: &InvestigatingCard) {
    view.details
        .push(("turns", format!("{} of {}", card.turns, card.max_turns)));
    view.details.push((
        "tokens",
        if card.tokens_known {
            format!(
                "{} of {}",
                group_thousands(card.input_tokens.saturating_add(card.output_tokens)),
                group_thousands(card.max_tokens),
            )
        } else {
            "unknown".to_owned()
        },
    ));
    view.details.push((
        "elapsed",
        format!("{}s of {}s", card.elapsed_secs, card.max_wall_secs),
    ));
}

/// Whether the toolbar badge is drawn, and the number it shows
/// (erratum `IN1b`-C-R49).
///
/// The number is always the **open** count — problems currently unresolved,
/// never lines ever written (`IN1b` §5.4 rule 25). What the audit log's size
/// decides is only whether the badge is *there*: before C-R49 the badge
/// vanished the moment the last incident resolved, and with it the only way
/// back to the audit view, which still had every line of the session in it.
/// So the badge renders whenever either quantity is non-zero, still showing
/// the open count — which is honestly `0` on a session whose problems are all
/// fixed — and the Incidents panel it opens carries the **Audit log** button
/// that reaches the other view.
///
/// `audit_entries` is the whole `ErrorLog`'s length rather than a count of
/// `"Incident"`-source lines, and after Part B those are the same number:
/// `KinewrightApp::note_incident` is the crate's only writer and it writes
/// that one source, which the sink gate's counts 2 and 3 prove
/// (review-final nit 6).
///
/// Pure on purpose: this is the whole of the badge's rule, and
/// `in1b_the_badge_is_reachable_while_the_audit_view_has_anything_to_show`
/// is its test. Where it is drawn is not tested (`IN1b` §10 limit 1).
#[must_use]
pub(crate) const fn incident_badge(open_count: usize, audit_entries: usize) -> Option<usize> {
    if open_count > 0 || audit_entries > 0 {
        Some(open_count)
    } else {
        None
    }
}

/// One incident's card, and the subject needed to send what it offers.
///
/// The Incidents panel builds this list under the log's read guard, then draws
/// it after the guard is gone, so the paint never holds the lock.
pub(crate) struct IncidentPanelRow {
    /// Which incident the row is about.
    pub(crate) id: kinewright_core::IncidentId,
    /// The pure view, exactly as the Media panel's card is built.
    pub(crate) view: IncidentCardView,
    /// The proposal card, when the incident carries a proposal (IN2 §4.3).
    pub(crate) proposal: Option<ProposalCardView>,
}

/// Every incident the badge counts, as cards, in the order the log keeps them.
///
/// Pure, and the tested half of the Incidents panel (`IN1b` §5.5 rule 31):
/// `show_incidents_panel` places it and is untested by design (IN1 §11.1
/// limit 2, `IN1b` §10 limit 1).
///
/// The Media panel shows only asset-subject incidents, deliberately
/// (`media_bin.rs`'s `==` filter). Everything else — a refused trim on a clip,
/// a refused mix on a bus, a failed save on the project, a branch conflict on
/// the agent — has no surface of its own without this list, which is why
/// `IN1b` §12 records that cutting the panel leaves those incidents nowhere to
/// appear.
///
/// `revert_available` is `false` for every row: colour's revert is the only one
/// `IN1b` ships and it is offered beside its asset, where the document fact
/// that gates it can be read (`IN1b` §5.5 rule 28).
///
/// `investigating` is the running session's counters, when a session is
/// running on this project: the matching row gains the three counter rows
/// (IN2 §5.5 rule 14).
#[must_use]
pub(crate) fn incident_panel_rows<'a>(
    incidents: impl Iterator<Item = &'a Incident>,
    investigating: Option<InvestigatingCard>,
    session_pairs: &InvestigatorSessionPairs,
) -> Vec<IncidentPanelRow> {
    incidents
        .filter(|incident| incident.state.is_open())
        .map(|incident| {
            let mut view = incident_card(incident, false);
            if let Some(card) = investigating
                && card.id == incident.id
            {
                append_investigating_rows(&mut view, &card);
            }
            let proposal = if shows_proposal_card(incident) {
                incident.proposal.as_ref().map(|proposal| {
                    proposal_card_with_reinvestigate(
                        incident,
                        proposal,
                        session_pairs.can_reinvestigate(
                            incident.id,
                            incident.code,
                            incident.subject,
                        ),
                    )
                })
            } else {
                None
            };
            IncidentPanelRow {
                id: incident.id,
                view,
                proposal,
            }
        })
        .collect()
}

/// Paint one card and report what the person pressed.
///
/// Untested by design (IN1 §5.3 rule 23): every assertion lives on
/// [`incident_card`], and the placement of the card in the Media panel is not
/// proven at all.
pub(crate) fn show_incident_card(ui: &mut egui::Ui, view: &IncidentCardView) -> Option<CardPress> {
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
                            pressed = Some(CardPress::Action(index));
                        }
                    }
                    RecoveryKind::Explain(body) => {
                        ui.colored_label(color::TEXT_SECONDARY, *body);
                    }
                }
            }
        });
        if view.reinvestigate || view.never_investigate {
            ui.horizontal_wrapped(|ui| {
                if view.reinvestigate && ui.button(REINVESTIGATE_LABEL).clicked() {
                    pressed = Some(CardPress::Reinvestigate);
                }
                if view.never_investigate && ui.button(NEVER_INVESTIGATE_LABEL).clicked() {
                    pressed = Some(CardPress::NeverInvestigate);
                }
            });
        }
    });
    pressed
}

/// Paint the proposal card below its incident card and report which proposal
/// action the person pressed (IN2 §4.3).
///
/// Untested by design, exactly as [`show_incident_card`] is: the tested thing
/// is [`proposal_card`](crate::investigator::proposal_card).
pub(crate) fn show_proposal_card(
    ui: &mut egui::Ui,
    view: &ProposalCardView,
) -> Option<ProposalAction> {
    let mut pressed = None;
    theme::card_frame(false).show(ui, |ui| {
        ui.label(egui::RichText::new(view.headline).font(theme::semibold(type_size::CAPTION)));
        ui.colored_label(color::TEXT_MUTED, view.subject_label.as_str());
        if view.stale {
            ui.colored_label(
                color::TEXT_MUTED,
                "This proposal no longer applies; Re-investigate to try again.",
            );
        }
        ui.label(view.explanation.as_str());
        for line in &view.operation_summary {
            ui.label(line);
        }
        if !view.stale {
            ui.colored_label(color::TEXT_MUTED, view.approval_note);
        }
        ui.horizontal_wrapped(|ui| {
            for action in &view.actions {
                let label = match action {
                    ProposalAction::Approve => "Approve",
                    ProposalAction::Reject => "Reject",
                    ProposalAction::Reinvestigate => REINVESTIGATE_LABEL,
                };
                if ui.button(label).clicked() {
                    pressed = Some(*action);
                }
            }
        });
    });
    pressed
}

/// Draw the badge-anchored Incidents panel (`IN1b` §5.5 rule 31).
///
/// The same window shape the error-log window already has, listing every open
/// incident's card whatever its subject. Untested by design, exactly as
/// [`show_incident_card`] is: the tested thing is
/// [`incident_panel_rows`], which needs no `egui::Ui` at all.
#[allow(
    clippy::too_many_lines,
    reason = "the panel owns the complete card and proposal interaction path"
)]
pub(crate) fn show_incidents_panel(app: &mut crate::app::KinewrightApp, ctx: &egui::Context) {
    if !app.incidents_open {
        return;
    }
    let project_index = app.focused_project;
    let investigating = app.investigating_card_for_project(project_index);
    let session_pairs = app.investigator_session_pairs_for_project(project_index);
    let rows = {
        let handle = std::sync::Arc::clone(&app.focused().incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        incident_panel_rows(log.all(), investigating, &session_pairs)
    };
    let mut pressed = None;
    let mut reinvestigate = None;
    let mut mute = None;
    let mut proposal_press = None;
    let mut open = app.incidents_open;
    let mut show_audit = false;
    let audit_entries = app.error_log.len();
    egui::Window::new("Incidents")
        .open(&mut open)
        .default_width(620.0)
        .default_height(320.0)
        .show(ctx, |ui| {
            // Erratum `IN1b`-C-R49: the audit view's way back. The badge is
            // the Incidents panel's anchor, so the panel is where the other
            // view has to be reachable from.
            ui.horizontal(|ui| {
                ui.label(format!("{} open problem(s)", rows.len()));
                if ui
                    .add_enabled(audit_entries > 0, egui::Button::new("Audit log"))
                    .on_hover_text("Every incident this session opened, in order")
                    .on_disabled_hover_text("Nothing has been recorded this session")
                    .clicked()
                {
                    show_audit = true;
                }
            });
            ui.separator();
            if rows.is_empty() {
                ui.colored_label(color::TEXT_MUTED, "Nothing is outstanding.");
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for row in &rows {
                    match show_incident_card(ui, &row.view) {
                        Some(CardPress::Action(index)) => {
                            let action = &row.view.actions[index];
                            if let RecoveryKind::Operation(operation) = &action.recovery.kind {
                                pressed = Some((row.id, operation.clone(), action.clone()));
                            }
                        }
                        Some(CardPress::Reinvestigate) => {
                            reinvestigate = Some(row.id);
                        }
                        Some(CardPress::NeverInvestigate) => {
                            mute = Some(row.id);
                        }
                        None => {}
                    }
                    if let Some(proposal) = &row.proposal
                        && let Some(action) = show_proposal_card(ui, proposal)
                    {
                        proposal_press = Some((row.id, action));
                    }
                    ui.add_space(space::ONE);
                }
            });
        });
    app.incidents_open = open;
    if show_audit {
        app.error_log_open = true;
    }
    if let Some((id, operation, action)) = pressed {
        let outcome = {
            let handle = std::sync::Arc::clone(&app.projects[project_index].incidents);
            let log = handle
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.get(id)
                .map(|incident| card_action_outcome(incident, &action))
        };
        if let Some(outcome) = outcome {
            app.send_incident_recovery(project_index, id, operation, outcome);
        }
    }
    if let Some(id) = reinvestigate {
        app.reinvestigate(project_index, id);
    }
    if let Some(id) = mute {
        let code = {
            let handle = std::sync::Arc::clone(&app.projects[project_index].incidents);
            let log = handle
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.get(id).map(|incident| incident.code)
        };
        if let Some(code) = code {
            app.mute_investigator_code(project_index, code);
        }
    }
    if let Some((id, action)) = proposal_press {
        match action {
            ProposalAction::Approve => {
                app.approve_investigator_proposal(project_index, id);
            }
            ProposalAction::Reject => {
                app.reject_investigator_proposal(project_index, id);
            }
            ProposalAction::Reinvestigate => {
                app.reinvestigate(project_index, id);
            }
        }
    }
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
        IncidentObservation, Observed, POLICY, TimelineRevision, explain_body, policy_recovery,
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

    /// `IN1b` §5.5 rule 30 and §9 clause 18: the headline table is **seventy**
    /// rows over sixty-seven codes, every row distinct, and every row is a
    /// headline rather than a body.
    ///
    /// The last clause is what erratum `IN1b`-A-R10 asks for. Stage A left a
    /// shim at `incident_headline` returning `explain_body(code)` for every
    /// non-colour code, and core already proves the 54 non-colour bodies
    /// pairwise distinct — so the distinctness half passed for the wrong
    /// reason on 54 of its 70 rows. Asserting
    /// `incident_headline(code, class) != explain_body(code)` row by row fails
    /// against the shim and passes against the table, which is the only
    /// difference between the two that a test can see.
    ///
    /// A headline says what happened; a body says what the person must change.
    /// Part A's fifteen colour rows are unchanged byte for byte, so IN1 §9
    /// clause 3's pinned headline still holds.
    #[test]
    fn in1b_every_headline_row_is_distinct_and_covers_the_whole_table() {
        let mut headlines = Vec::new();
        for entry in POLICY {
            headlines.push(incident_headline(entry.code, PolicyClass::Explain));
            if entry.class == PolicyClass::AutoApply {
                headlines.push(incident_headline(entry.code, PolicyClass::AutoApply));
            }
        }
        assert_eq!(POLICY.len(), 67, "sixty-seven declared codes");
        assert_eq!(headlines.len(), 70, "seventy rows for sixty-seven codes");
        let mut sorted = headlines.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), headlines.len(), "every headline is distinct");
        for entry in POLICY {
            for class in [
                PolicyClass::AutoApply,
                PolicyClass::AskFirst,
                PolicyClass::Explain,
            ] {
                let headline = incident_headline(entry.code, class);
                assert_ne!(
                    headline,
                    explain_body(entry.code),
                    "{}: the headline must say what happened, not what to change",
                    entry.code.code()
                );
                assert!(
                    !headline.is_empty(),
                    "{}: every row says something",
                    entry.code.code()
                );
            }
        }
    }

    /// Erratum `IN1b`-C-R49: the badge is reachable while the audit view has
    /// anything to show, and the number it shows is always the open count.
    ///
    /// Before C-R49 the badge rendered on `open_count() > 0` alone, so the
    /// moment the last incident resolved it vanished — and with it the only
    /// affordance that reopened the audit window, which still held every line
    /// of the session. The first row below is that case.
    #[test]
    fn in1b_the_badge_is_reachable_while_the_audit_view_has_anything_to_show() {
        assert_eq!(
            incident_badge(0, 1),
            Some(0),
            "nothing open and one audit line: the badge is there, reading zero"
        );
        assert_eq!(
            incident_badge(0, 0),
            None,
            "nothing open and nothing recorded: no badge at all"
        );
        assert_eq!(
            incident_badge(3, 0),
            Some(3),
            "open problems put their own count on the badge"
        );
        assert_eq!(
            incident_badge(2, 7),
            Some(2),
            "and the audit view's size never changes the number, only the visibility"
        );
    }

    /// `IN1b` §7 item 26: a non-colour incident's card offers the written
    /// `Explain` body and exactly one thing, which is not a button.
    ///
    /// `revert_available` is `false` for every non-colour code until one ships
    /// a revert (`IN1b` §5.5 rule 28), and `policy_recovery` returns a single
    /// `Explain` recovery for every `Explain` row, so the card is one
    /// unpressable sentence — which is the whole visible difference between an
    /// incident and the scrolling log line it replaced.
    #[test]
    fn in1b_a_non_colour_card_offers_the_explain_body_and_nothing_to_press() {
        let code = IncidentCode::Operation(IncidentFamily::Bounds);
        let mut log = IncidentLog::default();
        let Observed::Opened(id) = log.observe(IncidentObservation::plain(
            code,
            IncidentSubject::Clip(kinewright_core::ClipId(4)),
            "split at project frame 900 is outside clip 4",
            TimelineRevision(2),
        )) else {
            panic!("a fresh log opens the first observation");
        };
        let incident = log.get(id).expect("the log keeps what it opened");
        let view = incident_card(incident, false);

        assert_eq!(view.class, PolicyClass::Explain);
        assert_eq!(view.severity, IncidentSeverity::Blocks);
        assert_eq!(view.subject_label, "Clip 4");
        assert_eq!(view.headline, incident_headline(code, PolicyClass::Explain));
        assert_ne!(view.headline, explain_body(code));
        assert_eq!(view.actions.len(), 1, "one explanation and nothing else");
        assert!(!view.actions[0].enabled, "there is nothing to press");
        assert_eq!(
            view.actions[0].recovery.kind,
            RecoveryKind::Explain(explain_body(code)),
            "the body below the card is core's written sentence, verbatim"
        );
        assert_eq!(
            view.actions
                .iter()
                .map(|action| action.recovery.clone())
                .collect::<Vec<_>>(),
            policy_recovery(code, incident.subject, &incident.evidence),
            "the card builds no recovery of its own"
        );
        // And the evidence rows the colour card carries are simply absent.
        let keys: Vec<&str> = view.details.iter().map(|(key, _)| *key).collect();
        assert!(!keys.contains(&"probed"));
        assert!(!keys.contains(&"assumed"));
    }

    /// `IN1b` §7 item 27 and §5.5 rule 28: the `assumed` detail row needs
    /// **both** a probed description in the evidence and a revert to be
    /// available.
    ///
    /// Part A gated it on one flag called `assumed_from_present`, which was
    /// also gating the revert button — one name for two questions. Splitting
    /// it makes the row's condition readable, and this test pins all four
    /// corners so a later pass cannot quietly drop half of it.
    #[test]
    fn in1b_the_assumed_detail_row_needs_both_a_probe_and_a_revert() {
        let probed = untagged_mp4_probe();
        let (colour_log, colour_id) = opened(
            IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries),
            &probed,
        );
        let colour = colour_log.get(colour_id).expect("open");
        assert!(colour.evidence.probed().is_some());

        let with_revert: Vec<&str> = incident_card(colour, true)
            .details
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert!(with_revert.contains(&"probed"));
        assert!(
            with_revert.contains(&"assumed"),
            "a probe and a revert together print the description the revert takes back"
        );

        let without_revert: Vec<&str> = incident_card(colour, false)
            .details
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert!(
            without_revert.contains(&"probed"),
            "the probe is evidence and stays"
        );
        assert!(
            !without_revert.contains(&"assumed"),
            "with nothing to revert to there is no `assumed` description to name"
        );

        // The other axis: no probe at all, both ways round.
        let mut log = IncidentLog::default();
        let Observed::Opened(id) = log.observe(IncidentObservation::plain(
            IncidentCode::Rejection(kinewright_core::RejectionIncident::ProjectSave),
            IncidentSubject::Project,
            "could not write the project file",
            TimelineRevision(1),
        )) else {
            panic!("a fresh log opens the first observation");
        };
        let plain = log.get(id).expect("open");
        assert!(plain.evidence.probed().is_none());
        for revert_available in [false, true] {
            let keys: Vec<&str> = incident_card(plain, revert_available)
                .details
                .iter()
                .map(|(key, _)| *key)
                .collect();
            assert!(!keys.contains(&"probed"));
            assert!(
                !keys.contains(&"assumed"),
                "a revert flag alone cannot conjure a description that was never probed"
            );
        }
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
        // IN2 §7 rule 1 gave `unknown_source_range` a button, so the row this
        // test needs is one of the seven `unsupported_source_*` rows, which
        // have neither a button nor a session and are IN3's (IN2 §10 limit 11).
        // Erratum IN2 A-R2: §9 regression R-A does not name this rewrite.
        //
        // The probe is the **reachable** one: an `unsupported_source_primaries`
        // row is only ever opened when the primaries carry a known non-Rec.709
        // value, and `rec709_compatible` of such a probe is false (IN2 §7
        // rule 5's reachability half).
        let probed = ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            ..untagged_mp4_probe()
        };
        let (log, id) = opened(
            IncidentCode::SourceColor(SourceColorIncident::UnsupportedPrimaries),
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

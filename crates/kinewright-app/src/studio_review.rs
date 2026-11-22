use eframe::egui;
use kinewright_core::output_review::SealedReviewOutcome;
use kinewright_project::{
    review_job::{
        ReviewAnnotationFields, ReviewControl, ReviewPhase, ReviewReceipt, ReviewStatus,
        delivery::{SealedSaveStatus, describe_exception, outcome_label, save_action_label},
    },
    studio_records::SnapshotKey,
};

use crate::ja1_attachment::Action;

pub(crate) struct ReviewRefresh(pub(crate) Option<ReviewStatus>);

/// Refresh review while no writer host holds the project.
pub(crate) const NO_HOST_REFRESH: &str =
    "Output reviews run while the writer host holds the saved project; showing the stored review.";

/// A review control and the version selected when it was sent; the reply
/// reloads that version's details, so a new receipt shows.
pub(crate) struct ReviewAction {
    pub(crate) control: ReviewControl,
    pub(crate) version: Option<SnapshotKey>,
}

/// What the panel shows for the selected version.
struct ReviewView<'a> {
    receipt: Option<(&'static str, Vec<String>)>,
    /// The selected version's own receipt, which a save names exactly.
    sealed: Option<&'a ReviewReceipt>,
    saved: Option<&'a ReviewAnnotationFields>,
    /// The details are the selected version's own.
    current: bool,
}

impl<'a> ReviewView<'a> {
    fn new(
        details: Option<&'a crate::ja1_attachment::EditorialDetails>,
        selected: Option<&SnapshotKey>,
    ) -> Self {
        let mut view = Self::of(
            details.and_then(|details| details.version.as_ref()),
            details
                .and_then(|details| details.receipt.as_ref())
                .map(|receipt| (&receipt.key.snapshot, &receipt.outcome)),
            details.and_then(|details| details.annotations.as_ref()),
            selected,
        );
        view.sealed = details
            .and_then(|details| details.receipt.as_ref())
            .filter(|receipt| view.receipt.is_some() && Some(&receipt.key.snapshot) == selected);
        view
    }

    fn of(
        read: Option<&SnapshotKey>,
        sealed: Option<(&SnapshotKey, &SealedReviewOutcome)>,
        saved: Option<&'a ReviewAnnotationFields>,
        selected: Option<&SnapshotKey>,
    ) -> Self {
        // Details read for another version say nothing about this one.
        let current = read.is_some() && read == selected;
        Self {
            receipt: sealed
                .filter(|_| current)
                .and_then(|(snapshot, outcome)| receipt_lines(snapshot, outcome, selected)),
            sealed: None,
            saved: saved.filter(|_| current),
            current,
        }
    }
}

#[derive(Default)]
pub(crate) struct OutputReview {
    status: Option<ReviewStatus>,
    annotation_version: Option<SnapshotKey>,
    sourced_elements: String,
    text_cards: String,
    intended_pauses: String,
    annotation_error: Option<String>,
    /// The last save status the host reported.
    save_status: Option<SealedSaveStatus>,
}

impl OutputReview {
    pub(crate) fn refresh(
        &mut self,
        reply: Option<ReviewRefresh>,
        project: Option<&kinewright_project::bundle_records::ProjectId>,
    ) {
        if let Some(ReviewRefresh(status)) = reply {
            self.status = status;
        }
        if self
            .status
            .as_ref()
            .is_some_and(|status| Some(&status.token.project) != project)
        {
            self.status = None;
        }
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        details: Option<&crate::ja1_attachment::EditorialDetails>,
        selected: Option<&SnapshotKey>,
        save_status: Option<SealedSaveStatus>,
    ) -> Option<Action> {
        if save_status.is_some() {
            self.save_status = save_status;
        }
        let version = selected;
        let view = ReviewView::new(details, selected);
        ui.separator();
        ui.strong("Output review");
        let mut control = ui
            .button("Refresh review")
            .clicked()
            .then_some(ReviewControl::Status);
        if let Some(status) = &self.status {
            let label = if status.stop_pending {
                "Stop pending"
            } else {
                match &status.phase {
                    ReviewPhase::Queued => "Queued",
                    ReviewPhase::Encoding => "Encoding",
                    ReviewPhase::Checking(_) => "Artifact ready for checks",
                    ReviewPhase::ReadyToSeal { .. } => "Ready to seal",
                    ReviewPhase::Paused { .. } => "Paused",
                    ReviewPhase::Stopping => "Stopping",
                    ReviewPhase::Stopped => "Stopped",
                }
            };
            ui.label(format!(
                "{label} · {}",
                status.token.snapshot.version.as_str()
            ));
            if let ReviewPhase::Paused {
                reason, findings, ..
            } = &status.phase
            {
                ui.label(reason);
                for finding in findings {
                    ui.label(format!("· {}", finding.note));
                }
            }
            ui.horizontal(|ui| {
                if !status.stop_pending
                    && matches!(
                        status.phase,
                        ReviewPhase::Queued | ReviewPhase::Paused { .. }
                    )
                    && ui.button("Resume review").clicked()
                {
                    control = Some(ReviewControl::Resume(status.token.clone()));
                }
                if status.phase != ReviewPhase::Stopped && ui.button("Stop review").clicked() {
                    control = Some(ReviewControl::Stop(status.token.clone()));
                }
            });
        }

        show_receipt(ui, view.receipt);
        if let Some(save) = self.show_save(ui, view.sealed) {
            control = Some(save);
        }

        if let Some(version) = version {
            if self
                .status
                .as_ref()
                .is_none_or(|status| status.phase == ReviewPhase::Stopped)
                && ui.button("Queue review of this version").clicked()
            {
                control = Some(ReviewControl::QueueDefault(version.clone()));
            }
            // Saved fields load once the selected version's details arrive.
            if view.current && self.annotation_version.as_ref() != Some(version) {
                self.annotation_version = Some(version.clone());
                let saved = view.saved;
                self.sourced_elements = json_array(saved.map(|saved| &saved.sourced_elements));
                self.text_cards = json_array(saved.map(|saved| &saved.text_cards));
                self.intended_pauses = json_array(saved.map(|saved| &saved.intended_pauses));
                self.annotation_error = None;
            }
            ui.collapsing("Review annotations", |ui| {
                ui.label("Sourced clip IDs, as a JSON array");
                ui.text_edit_singleline(&mut self.sourced_elements);
                ui.label("Card clip IDs, as a JSON array");
                ui.text_edit_singleline(&mut self.text_cards);
                ui.label("Intended holds in project frames, e.g. [{\"start\":24,\"end\":48}]");
                ui.text_edit_singleline(&mut self.intended_pauses);
                if ui.button("Save annotations for this version").clicked() {
                    match self.annotation_fields() {
                        Ok(fields) => {
                            self.annotation_error = None;
                            control = Some(ReviewControl::Annotate {
                                snapshot: version.clone(),
                                fields,
                            });
                        }
                        Err(error) => self.annotation_error = Some(error.to_string()),
                    }
                }
                if let Some(error) = &self.annotation_error {
                    ui.label(error);
                }
            });
        }
        control.map(|control| review_action(control, selected))
    }

    /// JA-1 S6 C2: the save action for a sealed cut, named by its own
    /// label, and the status line of a running or finished save.
    fn show_save(
        &self,
        ui: &mut egui::Ui,
        sealed: Option<&ReviewReceipt>,
    ) -> Option<ReviewControl> {
        let mut control = None;
        if let Some(receipt) = sealed
            && ui.button(save_action_label(&receipt.outcome)).clicked()
            && let Some(folder) = rfd::FileDialog::new().pick_folder()
        {
            control = Some(save_control(receipt, folder));
        }
        match &self.save_status {
            Some(SealedSaveStatus::Saving { name }) => {
                ui.label(format!("Saving {name}…"));
                if ui.button("Stop saving").clicked() {
                    control = Some(ReviewControl::StopSave);
                }
            }
            Some(SealedSaveStatus::Saved { path }) => {
                ui.label(format!("Saved to {}", path.display()));
            }
            Some(SealedSaveStatus::Failed { reason }) => {
                ui.label(format!("Save failed: {reason}"));
            }
            None => {}
        }
        control
    }

    fn annotation_fields(&self) -> Result<ReviewAnnotationFields, serde_json::Error> {
        Ok(ReviewAnnotationFields {
            sourced_elements: serde_json::from_str(&self.sourced_elements)?,
            text_cards: serde_json::from_str(&self.text_cards)?,
            intended_pauses: serde_json::from_str(&self.intended_pauses)?,
        })
    }
}

fn json_array<T: serde::Serialize>(value: Option<&Vec<T>>) -> String {
    value
        .and_then(|value| serde_json::to_string(value).ok())
        .unwrap_or_else(|| "[]".into())
}

/// The save of exactly the receipt Studio shows.
fn save_control(receipt: &ReviewReceipt, folder: std::path::PathBuf) -> ReviewControl {
    ReviewControl::SaveSealed {
        snapshot: receipt.key.snapshot.clone(),
        coverage: receipt.key.profile,
        key_sha256: receipt.key_sha256.clone(),
        folder,
    }
}

fn review_action(control: ReviewControl, selected: Option<&SnapshotKey>) -> Action {
    Action::StudioReview(Box::new(ReviewAction {
        control,
        version: selected.cloned(),
    }))
}

/// The seal label of the selected version only, with what was not checked.
fn show_receipt(ui: &mut egui::Ui, receipt: Option<(&'static str, Vec<String>)>) {
    if let Some((label, not_checked)) = receipt {
        ui.strong(label);
        if !not_checked.is_empty() {
            ui.label("Not checked:");
            for line in not_checked {
                ui.label(format!("· {line}"));
            }
        }
    }
}

/// The label and the elements not checked, when the receipt is the selected
/// version's own.
fn receipt_lines(
    sealed: &SnapshotKey,
    outcome: &SealedReviewOutcome,
    selected: Option<&SnapshotKey>,
) -> Option<(&'static str, Vec<String>)> {
    if selected != Some(sealed) {
        return None;
    }
    let not_checked = match outcome {
        SealedReviewOutcome::Delivered => Vec::new(),
        SealedReviewOutcome::ReviewedFactsNotChecked { exceptions } => {
            exceptions.iter().map(describe_exception).collect()
        }
    };
    Some((outcome_label(outcome), not_checked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::output_review::{DeliveryException, EligibilityReason};
    use kinewright_project::bundle_records::{HeadId, ObjectRef, ProjectId, RecordKind};
    use kinewright_project::studio_records::VersionId;

    fn key() -> SnapshotKey {
        SnapshotKey {
            project: ProjectId::fresh().unwrap(),
            version: VersionId::fresh().unwrap(),
            head: HeadId::fresh().unwrap(),
            document_hash: "0".repeat(64),
            input_manifest_ref: ObjectRef {
                sha256: "0".repeat(64),
                byte_length: 1,
                record_kind: RecordKind::InputManifest,
            },
        }
    }

    #[test]
    fn ja1_s5a_c7_studio_label_only_for_the_selected_version() {
        let sealed = key();
        let other = key();
        let facts = SealedReviewOutcome::ReviewedFactsNotChecked {
            exceptions: vec![DeliveryException::Element {
                element: kinewright_core::ClipId(2),
                reason: EligibilityReason::NumericCharacter,
            }],
        };
        assert_eq!(
            receipt_lines(&sealed, &facts, Some(&sealed)),
            Some((
                "Reviewed, facts not checked",
                vec!["Clip 2: it contains a number".to_owned()]
            ))
        );
        assert_eq!(
            receipt_lines(&sealed, &SealedReviewOutcome::Delivered, Some(&sealed)),
            Some(("Delivered", Vec::new()))
        );
        assert_eq!(
            receipt_lines(&sealed, &facts, Some(&other)),
            None,
            "C7_OTHER_VERSION"
        );
        assert_eq!(receipt_lines(&sealed, &facts, None), None, "C7_NO_VERSION");
    }

    #[test]
    fn ja1_s5a_c7_fix1_label_follows_the_person_selection() {
        let a = key();
        let b = key();
        let saved = ReviewAnnotationFields::default();
        let delivered = SealedReviewOutcome::Delivered;
        let view =
            |selected| ReviewView::of(Some(&a), Some((&a, &delivered)), Some(&saved), selected);
        // Selection A with A's details: A's label.
        let shown = view(Some(&a));
        assert_eq!(
            shown.receipt,
            Some(("Delivered", Vec::new())),
            "C7F_SELECTED_LABEL"
        );
        assert!(shown.current && shown.saved.is_some());
        // The person selects B before B's details arrive: no label, and A's
        // saved annotations are not shown as B's.
        let shown = view(Some(&b));
        assert_eq!(shown.receipt, None, "C7F_LABEL_FOLLOWS_SELECTION");
        assert!(
            !shown.current && shown.saved.is_none(),
            "C7F_STALE_DETAILS_IGNORED"
        );
        assert!(view(None).receipt.is_none());
    }

    #[test]
    fn ja1_s5a_c7_fix1_refresh_reloads_the_selected_version() {
        let b = key();
        let Action::StudioReview(review) = review_action(ReviewControl::Status, Some(&b)) else {
            panic!("C7F_REVIEW_ACTION");
        };
        assert!(matches!(review.control, ReviewControl::Status));
        assert_eq!(review.version, Some(b), "C7F_REFRESH_NAMES_THE_SELECTION");
    }
}

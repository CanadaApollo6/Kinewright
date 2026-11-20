use eframe::egui;
use kinewright_project::{
    review_job::{ReviewAnnotationFields, ReviewControl, ReviewPhase, ReviewStatus},
    studio_records::SnapshotKey,
};

use crate::ja1_attachment::Action;

pub(crate) struct ReviewRefresh(pub(crate) Option<ReviewStatus>);

#[derive(Default)]
pub(crate) struct OutputReview {
    status: Option<ReviewStatus>,
    annotation_version: Option<SnapshotKey>,
    sourced_elements: String,
    text_cards: String,
    intended_pauses: String,
    annotation_error: Option<String>,
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
    ) -> Option<Action> {
        let version = details.and_then(|details| details.version.as_ref());
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
            if let ReviewPhase::Paused { reason, .. } = &status.phase {
                ui.label(reason);
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

        if let Some(version) = version {
            if self
                .status
                .as_ref()
                .is_none_or(|status| status.phase == ReviewPhase::Stopped)
                && ui.button("Queue review of this version").clicked()
            {
                control = Some(ReviewControl::QueueDefault(version.clone()));
            }
            if self.annotation_version.as_ref() != Some(version) {
                self.annotation_version = Some(version.clone());
                let saved = details.and_then(|details| details.annotations.as_ref());
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
        control.map(|control| Action::StudioReview(Box::new(control)))
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

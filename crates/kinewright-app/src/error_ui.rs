use std::time::{Duration, Instant};

use eframe::egui;

use kinewright_core::{
    Incident, IncidentCode, IncidentFamily, IncidentObservation, IncidentSubject, LabelIncident,
    MediaError, TimelineRevision,
};

use crate::{
    app::KinewrightApp,
    incident_ui::incident_headline,
    theme::{self, type_size},
};

pub(crate) struct ErrorEntry {
    elapsed: Duration,
    source: &'static str,
    message: String,
}

pub(crate) struct ErrorLog {
    started: Instant,
    entries: Vec<ErrorEntry>,
}

impl Default for ErrorLog {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            entries: Vec::new(),
        }
    }
}

impl ErrorLog {
    /// Append one audit line.
    ///
    /// Module-private (`IN1b` §5.3 rule 17): after Part B the only writer in
    /// the whole crate is [`KinewrightApp::note_incident`], and a `pub(crate)`
    /// writer is a sink the next slice would quietly start using. The three
    /// `error_log.push` bypasses that used to reach past the sink are gone
    /// (`IN1b` §5.1 rule 14).
    fn push(&mut self, source: &'static str, message: impl Into<String>) {
        self.entries.push(ErrorEntry {
            elapsed: self.started.elapsed(),
            source,
            message: message.into(),
        });
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Count entries written under one source label.
    ///
    /// Test-only: IN1 §5.2 rule 15's two-counts test asserts the number of
    /// `"Incident"`-source entries rather than `len()`, because the log may
    /// also hold an environment-dependent audio line. The production log
    /// exposes no per-source read, and Part A changes nothing about how the
    /// log works (IN1 §4.6 rule 41); this accessor ships in no build.
    #[cfg(test)]
    pub(crate) fn count_with_source(&self, source: &str) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.source == source)
            .count()
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// A panel worker's failure, typed when the seam is typed (`IN2B` §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkerError {
    /// A string with no typed error behind it: a proof error flattened at
    /// the excluded seventh seam, a worker panic, or a containment
    /// collapse. Notes `panel_worker_error` — never a session (§5 rule 4).
    Untyped(String),
    /// A typed media failure. Notes `from_media_error` — per allowlist.
    /// (`MediaError::Cancelled` travels as `Media` and maps to *no note*,
    /// §7 rule 7 — the one `Media` arm that is not a failure.)
    Media(MediaError),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Renders interpolate the same text as today: the string, or
            // the typed error's own rendering.
            Self::Untyped(message) => formatter.write_str(message),
            Self::Media(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<MediaError> for WorkerError {
    fn from(error: MediaError) -> Self {
        Self::Media(error)
    }
}

/// The `WorkerError` five's shared body (`IN2B` §7 rule 2): the tag plus
/// `transient` over `from_media_error`/`plain`.
///
/// `Untyped(s)` notes `panel_worker_error` with the worker's string
/// verbatim behind the tag; `Media(Cancelled)` is the operator saying
/// "stop working", not a failure (§7 rule 7); every other `Media(e)`
/// routes through `from_media_error` with the tag prefixed. Every `Some`
/// arm marks provenance (§7 rule 2, N2/B-3). Pure: no clock, no document,
/// no log.
pub(crate) fn worker_error_observation(
    error: &WorkerError,
    subject: IncidentSubject,
    tag: &str,
    revision: TimelineRevision,
) -> Option<IncidentObservation> {
    match error {
        WorkerError::Untyped(message) => {
            let mut observation = IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::PanelWorkerError),
                subject,
                format!("{tag}{message}"),
                revision,
            );
            observation.transient = true;
            Some(observation)
        }
        WorkerError::Media(MediaError::Cancelled) => None,
        WorkerError::Media(error) => {
            let mut observation = IncidentObservation::from_media_error(error, subject, revision);
            observation.observed = format!("{tag}{}", observation.observed);
            observation.transient = true;
            Some(observation)
        }
    }
}

impl KinewrightApp {
    /// The single writer to [`ErrorLog`] after `IN1b` (§5.3 rule 17).
    ///
    /// It does exactly what `KinewrightApp::record_error` did, for an incident
    /// rather than for a string: the status bar takes the incident's headline,
    /// one audit line is written under the fixed source `"Incident"`, and the
    /// window pops open. The source is a constant because the log is now an
    /// audit view of the incident stream and nothing else — the per-label
    /// spellings the 17 source labels used are carried by the incident's own
    /// `code`, which says more than a label ever did.
    ///
    /// `&mut self` rather than a method on [`ErrorLog`]: `status` and
    /// `error_log_open` are `KinewrightApp` fields and a method on the log
    /// cannot reach them (`IN1b` §5.3 rule 17, N1/B5).
    pub(crate) fn note_incident(&mut self, incident: &Incident) {
        let headline = incident_headline(incident.code, incident.class);
        headline.clone_into(&mut self.status);
        self.error_log.push("Incident", headline);
        self.error_log_open = true;
    }

    /// Queue one observation for the incident router (`IN1b` §5.2 rule 16).
    ///
    /// Every migrated error path in the crate ends here or in one of the
    /// four shorthands below. The router drains `pending_observations` once per
    /// update, hands each one to `IncidentLog::observe`, and writes the audit
    /// line through [`Self::note_incident`] — so a site queues a *problem* and
    /// never a rendered line, and the dedup key collapses repeats that a
    /// scrolling log used to show once per frame.
    pub(crate) fn note_observation(&mut self, observation: IncidentObservation) {
        self.pending_observations.push(observation);
    }

    /// Queue a `Plain`-evidence observation stated against the focused
    /// project's live revision.
    ///
    /// The focused project is the right one by construction: the router opens
    /// every observation on the focused session's log (IN1 §5.2 rule 5), and
    /// per-project attribution is `IN1b` §13 D14, re-deferred.
    ///
    /// **The revision this stamps is deliberately the focused project's, even
    /// at a site that belongs to another** — `handle_relink_probe_response`
    /// and `dispatch_relink` both run for a `project_index` the person may not
    /// be looking at, and the three core drain arms stamp
    /// `self.projects[project_index].revision` instead. The disagreement is
    /// not a bug to fix here: the incident lands on the focused log either
    /// way, so the revision it records has to be that log's to mean anything,
    /// and making the whole path per-project is exactly D14. A site that wants
    /// the other revision passes an observation it built itself, through
    /// [`Self::note_observation`] (review-app-2 N1).
    pub(crate) fn note_plain(
        &mut self,
        code: IncidentCode,
        subject: IncidentSubject,
        observed: impl Into<String>,
    ) {
        let revision = self.focused().revision;
        self.note_observation(IncidentObservation::plain(
            code, subject, observed, revision,
        ));
    }

    /// [`Self::note_plain`] for a source label's placeholder code.
    pub(crate) fn note_label(
        &mut self,
        label: LabelIncident,
        subject: IncidentSubject,
        observed: impl Into<String>,
    ) {
        self.note_plain(IncidentCode::Label(label), subject, observed);
    }

    /// [`Self::note_label`] for the three open-time aggregates (`IN2B` §7
    /// rule 1, rows 9/10/11): they describe the just-completed open, so
    /// they persist dropped — never pinned `Open` against a later open.
    pub(crate) fn note_transient_label(
        &mut self,
        label: LabelIncident,
        subject: IncidentSubject,
        observed: impl Into<String>,
    ) {
        let revision = self.focused().revision;
        let mut observation =
            IncidentObservation::plain(IncidentCode::Label(label), subject, observed, revision);
        observation.transient = true;
        self.note_observation(observation);
    }

    /// [`Self::note_plain`] for the fourteen sites whose message begins
    /// "Core actor stopped" (`IN1b` §3.2 rule 15).
    ///
    /// Their label placeholder's body — "read the message, change what it
    /// names" — is false: nothing the person changes restarts a stopped
    /// actor. `operation_internal`'s body is the true one.
    pub(crate) fn note_actor_stopped(
        &mut self,
        subject: IncidentSubject,
        observed: impl Into<String>,
    ) {
        self.note_plain(
            IncidentCode::Operation(IncidentFamily::Internal),
            subject,
            observed,
        );
    }

    pub(crate) fn show_error_log(&mut self, ctx: &egui::Context) {
        if !self.error_log_open {
            return;
        }
        let mut clear = false;
        egui::Window::new("Error log")
            .open(&mut self.error_log_open)
            .default_width(620.0)
            .default_height(240.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{} error(s)", self.error_log.entries.len()));
                    if ui.button("Clear").clicked() {
                        clear = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for entry in &self.error_log.entries {
                            let total_seconds = entry.elapsed.as_secs();
                            let minutes = total_seconds / 60;
                            let seconds = total_seconds % 60;
                            ui.horizontal_wrapped(|ui| {
                                ui.monospace(format!(
                                    "+{minutes:02}:{seconds:02}.{:03}",
                                    entry.elapsed.subsec_millis()
                                ));
                                ui.label(
                                    egui::RichText::new(entry.source)
                                        .font(theme::semibold(type_size::BODY)),
                                );
                                ui.label(&entry.message);
                            });
                        }
                    });
            });
        if clear {
            self.error_log.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;

    /// Renders interpolate the same text as today: the untouched string, or
    /// the typed error's own rendering.
    #[test]
    fn worker_error_renders_the_string_or_the_typed_error() {
        assert_eq!(
            WorkerError::Untyped("the renderer fell over".to_owned()).to_string(),
            "the renderer fell over"
        );
        assert_eq!(
            WorkerError::Media(MediaError::Cancelled).to_string(),
            MediaError::Cancelled.to_string()
        );
        assert_eq!(
            WorkerError::from(MediaError::Cancelled),
            WorkerError::Media(MediaError::Cancelled)
        );
    }

    /// Every `.rs` file under this crate's `src`, read once, as
    /// `(path relative to `src`, contents)`.
    ///
    /// The walk is recursive so a sink hidden in a future subdirectory cannot
    /// slip past the gate.
    fn crate_sources() -> Vec<(String, String)> {
        fn walk(root: &Path, directory: &Path, into: &mut Vec<(String, String)>) {
            let entries = fs::read_dir(directory).expect("the crate's own source tree is readable");
            for entry in entries {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    walk(root, &path, into);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    let relative = path
                        .strip_prefix(root)
                        .expect("every file is under the root")
                        .to_string_lossy()
                        .replace('\\', "/");
                    let contents = fs::read_to_string(&path).expect("a readable source file");
                    into.push((relative, contents));
                }
            }
        }
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
        let mut sources = Vec::new();
        walk(root, root, &mut sources);
        sources
    }

    /// How many lines across `sources` contain `needle` and do not contain
    /// `excluding`, restricted to `only` when one is given.
    fn count_lines(
        sources: &[(String, String)],
        needle: &str,
        excluding: Option<&str>,
        only: Option<&str>,
    ) -> usize {
        sources
            .iter()
            .filter(|(path, _)| only.is_none_or(|wanted| path == wanted))
            .flat_map(|(_, contents)| contents.lines())
            .filter(|line| line.contains(needle))
            .filter(|line| excluding.is_none_or(|excluded| !line.contains(excluded)))
            .count()
    }

    /// `IN1b` §7 item 17 and §9 clause 6: the sink gate's four spelled greps,
    /// run over the crate's own sources, reading **0 / 0 / 1 / 1**.
    ///
    /// The mechanism is stated so the bundle cannot pass vacuously (the
    /// critic's N5): counts 1 and 2 are `== 0` and would pass on an empty
    /// read, so the file-count assertion and count 4's `== 1` are what make it
    /// honest. Each pattern is assembled at run time rather than written as a
    /// literal, because a literal in this file would be a line the gate counts.
    ///
    /// **What the gate does not claim** (`IN1b` §5.3 rule 22). The four
    /// patterns name `record_error`, `error_log.push` and `note_incident` and
    /// nothing else, so the five `note_*` shorthands beside `note_incident`
    /// are invisible to every one of them. That is correct today — each one
    /// funnels into `pending_observations` and reaches the person only through
    /// the router, so none is a sink — but it is correct by construction
    /// rather than by the gate, and a sixth shorthand that wrote `status` or
    /// the log directly would leave all four counts reading 0 / 0 / 1 / 1
    /// (review-app-2 N4). All four counts are also over the app crate only.
    /// `crates/kinewright-agent/src/server.rs` carries
    /// 277 `error_text`/`error_structured` call sites, every one of which
    /// reaches a person through the chat panel and only 22 of which carry a
    /// `code` key; they are a different population and are deferred by name as
    /// `IN1b` §13 D-B1. The app's own panel-local labels, its two crash-recovery
    /// modals, its LUT-store tooltips (D-B2) and its four retyped seams (D-B3,
    /// `WorkerError` after `IN2B` §6) are invisible here too. 0 / 0 / 1 / 1
    /// means *the log, the status bar and the chat transcript*, not *no
    /// untyped refusal reaches a person*.
    #[test]
    fn in1b_the_sink_gate_counts_are_zero_zero_one_one() {
        let sources = crate_sources();
        assert!(
            sources.len() >= 20,
            "the walk read {} files; a gate that reads nothing passes everything",
            sources.len()
        );
        assert!(
            sources.iter().any(|(path, _)| path == "error_ui.rs"),
            "the sink's own file is in the walk"
        );

        let record_error_call = concat!("record_error", "(");
        let record_error_definition = concat!("fn record_error", "(");
        assert_eq!(
            count_lines(
                &sources,
                record_error_call,
                Some(record_error_definition),
                None
            ),
            0,
            "grep 1: the symbol is gone, not wrapped — a deprecated wrapper is a \
             sink the next slice would use"
        );

        let log_push = concat!("error_log.push", "(");
        assert_eq!(
            count_lines(&sources, log_push, None, None)
                - count_lines(&sources, log_push, None, Some("error_ui.rs")),
            0,
            "grep 2: no file outside the sink's own module writes to the log"
        );
        assert_eq!(
            count_lines(&sources, log_push, None, Some("error_ui.rs")),
            1,
            "grep 3: exactly one writer, and it is `note_incident`'s body"
        );

        let note_incident_call = concat!("note_incident", "(");
        let note_incident_definition = concat!("fn note_incident", "(");
        assert_eq!(
            count_lines(
                &sources,
                note_incident_call,
                Some(note_incident_definition),
                None
            ),
            1,
            "grep 4: exactly one call site, in `audit_new_incident`"
        );
    }
}

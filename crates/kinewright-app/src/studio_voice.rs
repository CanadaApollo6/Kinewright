//! Spoken feedback retains its capture until the authenticated owner acknowledges it.
use crate::{
    app::KinewrightApp,
    recording::feedback_capture::{Capture, CaptureReservation, CapturedAudio},
};
use eframe::egui;
use kinewright_agent::writer_host::{HostClient, HostCommand, HostReply};
use kinewright_project::{
    receipts::IssuerId,
    studio_records::{CommentId, FeedbackEvidenceKind, NoteId},
    voice_ingress::VoiceUpload,
};
use std::{
    io::Read as _,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, Instant},
};

type Reply = Receiver<Result<HostReply, String>>;
struct UploadReply {
    descriptor: VoiceUpload,
    result: Result<HostReply, String>,
}

pub(crate) fn recover_upload(
    client: &HostClient,
    descriptor: &mut VoiceUpload,
) -> Result<Option<HostReply>, String> {
    client
        .recover_voice_upload(descriptor)
        .map_err(|e| e.to_string())
}

#[derive(Default)]
pub(crate) struct Voice {
    state: State,
    project: Option<kinewright_project::bundle_records::ProjectId>,
    note: Option<NoteId>,
    comment: Option<CommentId>,
    issuer: Option<IssuerId>,
    message: String,
    text: String,
    edited: bool,
    pending_choice: Option<PendingChoice>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PendingChoice {
    envelope: kinewright_project::studio_records::MetadataEnvelope,
    note: kinewright_project::studio_records::NoteId,
    text_ref: kinewright_project::bundle_records::ObjectRef,
    comment: CommentId,
    text: String,
    draft_text: String,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeptVoice {
    comment: CommentId,
    capture: Option<crate::recording::feedback_capture::KeptCapture>,
    upload: Option<VoiceUpload>,
    status: Option<kinewright_agent::writer_host::VoiceStatus>,
    text: String,
    edited: bool,
    pending_choice: Option<PendingChoice>,
}
impl KeptVoice {
    pub(crate) fn holds_work(&self) -> bool {
        self.capture.is_some() || self.edited || self.pending_choice.is_some()
    }
}
#[derive(Default)]
enum State {
    #[default]
    Idle,
    Recording(Capture),
    Unprepared(Box<CapturedAudio>),
    Upload {
        captured: Box<CapturedAudio>,
        upload: VoiceUpload,
        log: Vec<u8>,
        reply: Option<Receiver<UploadReply>>,
    },
    Saved {
        residue: Option<Box<CapturedAudio>>,
        status: kinewright_agent::writer_host::VoiceStatus,
        reply: Option<Reply>,
        next_poll: Instant,
    },
}
fn upload(
    client: HostClient,
    captured: &CapturedAudio,
    descriptor: VoiceUpload,
    log: Vec<u8>,
    settled: Option<HostReply>,
) -> Result<Receiver<UploadReply>, String> {
    let path = captured.path.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("studio-voice-upload".into())
        .spawn(move || {
            let result = (|| {
                if let Some(reply) = settled {
                    return Ok(reply);
                }
                let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                client
                    .acknowledge_voice(descriptor.clone(), file.chain(std::io::Cursor::new(log)))
                    .map_err(|e| e.to_string())
            })();
            let _ = tx.send(UploadReply { descriptor, result });
        })
        .map_err(|e| e.to_string())?;
    Ok(rx)
}
impl KinewrightApp {
    fn keep_voice_editor(&mut self) -> Result<(), String> {
        let voice = &self.focused().studio.voice;
        let (Some(note), Some(comment)) = (voice.note.clone(), voice.comment.clone()) else {
            return Ok(());
        };
        if voice.project.as_ref() != self.focused().studio.feedback.keeping.project() {
            return Err("Spoken draft belongs to another project".into());
        }
        let text = voice.text.clone();
        let edited = voice.edited;
        let pending_choice = voice.pending_choice.clone();
        let status = match &voice.state {
            State::Saved { status, .. } => Some(status.clone()),
            _ => None,
        };
        let feedback = &mut self.focused_mut().studio.feedback;
        if !feedback.voices.contains_key(&note) && feedback.voices.len() >= 128 {
            return Err("128 kept spoken notes maximum".into());
        }
        let kept = feedback
            .voices
            .entry(note.clone())
            .or_insert_with(|| KeptVoice {
                comment,
                capture: None,
                upload: None,
                status: None,
                text: String::new(),
                edited: false,
                pending_choice: None,
            });
        kept.text = text;
        kept.edited = edited;
        kept.pending_choice = pending_choice;
        if status.is_some() {
            kept.status = status;
        }
        feedback.active_voice = Some(note);
        feedback.changed = true;
        feedback.retain_for_dispatch()
    }
    fn keep_voice_capture(
        &mut self,
        captured: &CapturedAudio,
        upload: Option<&VoiceUpload>,
    ) -> Result<(), String> {
        self.keep_voice_editor()?;
        let binding = captured.kept_binding().map_err(|e| e.to_string())?;
        let feedback = &mut self.focused_mut().studio.feedback;
        let kept = feedback
            .voices
            .get_mut(captured.note_id())
            .ok_or("Voice draft missing")?;
        kept.capture = Some(binding);
        kept.upload = upload.cloned();
        feedback.retain_for_dispatch()
    }
    fn dispatch_voice_upload(
        &mut self,
        client: HostClient,
        captured: &CapturedAudio,
        mut descriptor: VoiceUpload,
        log: Vec<u8>,
    ) -> Result<Receiver<UploadReply>, String> {
        let settled = recover_upload(&client, &mut descriptor)?;
        self.keep_voice_capture(captured, Some(&descriptor))?;
        upload(client, captured, descriptor, log, settled)
    }
    fn restore_spoken_note(&mut self, note: &NoteId) -> Result<(), String> {
        let kept = self
            .focused()
            .studio
            .feedback
            .voices
            .get(note)
            .cloned()
            .ok_or("Kept spoken note missing")?;
        let comment = kept.comment.clone();
        if kept.status.is_some()
            && !self
                .focused()
                .studio
                .feedback
                .drafts
                .iter()
                .any(|draft| draft.comment == comment)
        {
            self.with_feedback(|feedback, port| -> Result<(), String> {
                feedback.refresh(&*port)?;
                let saved = feedback
                    .saved
                    .iter()
                    .find(|saved| saved.comment == comment)
                    .cloned()
                    .ok_or("Saved spoken note missing")?;
                feedback.revise(&saved);
                Ok(())
            })??;
        }
        self.focused_mut().studio.feedback.active_voice = Some(note.clone());
        self.focused_mut().studio.feedback.active = Some(comment);
        let voice = &mut self.focused_mut().studio.voice;
        voice.comment = Some(kept.comment);
        voice.note = Some(note.clone());
        voice.text = kept.text;
        voice.edited = kept.edited;
        voice.pending_choice = kept.pending_choice;
        voice.issuer = voice
            .pending_choice
            .as_ref()
            .map(|p| p.envelope.id.issuer.clone())
            .or_else(|| kept.upload.as_ref().map(|u| u.envelope.id.issuer.clone()));
        let state = if let Some(binding) = kept.capture {
            let context = self
                .focused()
                .ja1
                .as_ref()
                .ok_or("Project unavailable")?
                .studio_context(&binding.anchor.snapshot)
                .map_err(|e| e.to_string())?;
            let captured = Box::new(binding.restore(&context).map_err(|e| e.to_string())?);
            if let Some(descriptor) = kept.upload {
                let log = captured.playhead_bytes().map_err(|e| e.to_string())?;
                let reply = self.dispatch_voice_upload(
                    self.voice_client()?,
                    &captured,
                    descriptor.clone(),
                    log.clone(),
                );
                let reply = match reply {
                    Ok(reply) => Some(reply),
                    Err(why) => {
                        self.focused_mut().studio.voice.message = why;
                        None
                    }
                };
                State::Upload {
                    captured,
                    upload: descriptor,
                    log,
                    reply,
                }
            } else {
                self.prepare_voice_upload(captured)
            }
        } else if let Some(status) = kept.status {
            State::Saved {
                residue: None,
                status,
                reply: None,
                next_poll: Instant::now(),
            }
        } else {
            State::Idle
        };
        self.focused_mut().studio.voice.state = state;
        Ok(())
    }
    fn acknowledge_spoken_capture(
        &mut self,
        captured: Box<CapturedAudio>,
        status: kinewright_agent::writer_host::VoiceStatus,
    ) -> Result<State, (Box<CapturedAudio>, String)> {
        let attached = self
            .attach_voice(
                &self.focused().studio.voice.comment.clone().unwrap(),
                status.voice.clone(),
                status.voice.chosen_transcript_revision.clone(),
            )
            .and_then(|()| {
                let feedback = &mut self.focused_mut().studio.feedback;
                let kept = feedback
                    .voices
                    .get_mut(&status.voice.note_id)
                    .ok_or("Voice draft missing")?;
                kept.status = Some(status.clone());
                feedback.retain_for_dispatch()
            });
        if let Err(why) = attached {
            return Err((captured, why));
        }
        let residue = match captured.cleanup_acknowledged() {
            Ok(()) => {
                let feedback = &mut self.focused_mut().studio.feedback;
                let kept = feedback.voices.get_mut(&status.voice.note_id).unwrap();
                kept.capture = None;
                kept.upload = None;
                if let Err(why) = feedback.persist() {
                    self.focused_mut().studio.voice.message = why;
                }
                None
            }
            Err(why) => {
                self.focused_mut().studio.voice.message =
                    format!("Audio saved. Capture cleanup needs retry: {why}");
                Some(captured)
            }
        };
        if let Ok(client) = self.voice_client() {
            let _ = client.request(HostCommand::StartTranscription {
                note: status.voice.note_id.clone(),
            });
        }
        Ok(State::Saved {
            residue,
            status,
            reply: None,
            next_poll: Instant::now(),
        })
    }
    fn prepare_voice_upload(&mut self, captured: Box<CapturedAudio>) -> State {
        let prepared = (|| -> Result<_, String> {
            self.keep_voice_capture(&captured, None)?;
            let client = self.voice_client()?;
            let grant = client
                .request(HostCommand::IssueMetadataIssuer {
                    issuer: self.focused().studio.voice.issuer.clone(),
                })
                .map_err(|e| e.to_string())?;
            if let Some(error) = grant.error {
                return Err(error.message);
            }
            let grant = grant.metadata_issuer.ok_or("Voice envelope missing")?;
            self.focused_mut().studio.voice.issuer = Some(grant.envelope.id.issuer.clone());
            let (descriptor, log) = captured
                .prepare_upload(grant.envelope, grant.root_generation)
                .map_err(|e| e.to_string())?;
            let rx =
                self.dispatch_voice_upload(client, &captured, descriptor.clone(), log.clone())?;
            Ok((descriptor, log, rx))
        })();
        match prepared {
            Ok((descriptor, log, rx)) => {
                self.focused_mut().studio.voice.message = "Saving audio…".into();
                State::Upload {
                    captured,
                    upload: descriptor,
                    log,
                    reply: Some(rx),
                }
            }
            Err(why) => {
                self.focused_mut().studio.voice.message =
                    format!("Capture kept at {}. {why}", captured.path.display());
                // Keep the complete draft and its reservations even when envelope acquisition fails.
                State::Unprepared(captured)
            }
        }
    }

    fn voice_client(&self) -> Result<HostClient, String> {
        self.focused()
            .ja1
            .as_ref()
            .and_then(crate::ja1_attachment::Ja1Attachment::host_client)
            .ok_or_else(|| {
                "Save and open this project with its writer host before recording a note.".into()
            })
    }
    pub(crate) fn voice_press(&mut self) {
        if let State::Recording(capture) = &self.focused().studio.voice.state {
            capture.stop();
            return;
        }
        let result = (|| -> Result<(), String> {
            self.voice_client()?;
            if matches!(
                self.focused().studio.voice.state,
                State::Upload { .. } | State::Unprepared(_)
            ) {
                return Err("The recorded note is still waiting for acknowledgment. Retry its upload first.".into());
            }
            let microphone = self
                .record_dialog
                .microphone
                .clone()
                .or_else(|| {
                    self.record_dialog
                        .devices
                        .as_ref()
                        .and_then(|d| d.audio.first().cloned())
                })
                .ok_or("Choose a microphone in Record before recording a spoken note.")?;
            self.begin_spoken_note(crate::recording::FeedbackInput::Microphone(microphone))?;
            Ok(())
        })();
        if let Err(why) = result {
            self.focused_mut().studio.voice.message = why;
            self.focused_mut().studio.tab = crate::studio::Tab::Notes;
        }
    }
    fn begin_spoken_note(&mut self, input: crate::recording::FeedbackInput) -> Result<(), String> {
        if self.focused().studio.voice.pending_choice.is_some() {
            return Err(
                "Retry the pending transcript choice before recording another note.".into(),
            );
        }
        self.keep_voice_editor()?;
        let feedback = &mut self.focused_mut().studio.feedback;
        while feedback.voices.len() >= 128 {
            let settled = feedback.voices.iter().find(|(_, draft)| !draft.holds_work()).map(|(note, _)| note.clone()).ok_or("128 unfinished spoken notes maximum; finish an earlier note before recording another")?;
            feedback.voices.remove(&settled);
        }
        feedback.retain_for_dispatch()?;
        let comment = self.new_feedback_note()?;
        let anchor = self.voice_anchor(&comment)?;
        let context = match self
            .focused()
            .studio
            .selection
            .clone()
            .filter(|context| context.snapshot.descriptor.key == anchor.snapshot)
        {
            Some(context) => context,
            None => std::sync::Arc::new(
                self.focused()
                    .ja1
                    .as_ref()
                    .ok_or("Project unavailable")?
                    .studio_context(&anchor.snapshot)
                    .map_err(|e| e.to_string())?,
            ),
        };
        let pinned = context.pin_snapshot().map_err(|e| e.to_string())?;
        let budget = CaptureReservation::reserve_context(&context).map_err(|e| e.to_string())?;
        let directory = kinewright_media::default_data_dir().join("feedback-captures");
        let capture = Capture::start_input(budget, input, directory, anchor, pinned)?;
        let project = self.focused().studio.feedback.keeping.project().cloned();
        let voice = &mut self.focused_mut().studio.voice;
        voice.project = project;
        voice.note = Some(capture.note_id().clone());
        voice.comment = Some(comment);
        voice.text.clear();
        voice.edited = false;
        voice.state = State::Recording(capture);
        voice.message = "Recording at the displayed picture. Press Stop when finished.".into();
        self.keep_voice_editor()
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one frame advances the current capture, upload or ASR state while retaining its recovery carrier"
    )]
    pub(crate) fn voice_frame(&mut self) {
        let project = self.focused().studio.feedback.keeping.project().cloned();
        if self.focused().studio.voice.project != project {
            let note = self.focused().studio.feedback.active_voice.clone();
            self.focused_mut().studio.voice = Voice {
                project,
                ..Voice::default()
            };
            if let Some(note) = note
                && let Err(why) = self.restore_spoken_note(&note)
            {
                self.focused_mut().studio.voice.message = why;
            }
        }
        let mut keep = false;
        let state = std::mem::take(&mut self.focused_mut().studio.voice.state);
        let next = match state {
            State::Recording(capture) => match capture.finish() {
                Err(capture) => State::Recording(*capture),
                Ok(captured) => self.prepare_voice_upload(Box::new(captured)),
            },
            State::Upload {
                captured,
                upload: mut descriptor,
                log,
                mut reply,
            } => {
                let received = reply.as_ref().map(|rx| {
                    rx.try_recv().map(|r| {
                        descriptor = r.descriptor;
                        r.result
                    })
                });
                match received {
                    Some(Ok(Ok(response)))
                        if response.error.is_none() && response.voice.is_some() =>
                    {
                        let status = response.voice.unwrap();
                        match self.acknowledge_spoken_capture(captured, status) {
                            Ok(state) => state,
                            Err((captured, why)) => {
                                self.focused_mut().studio.voice.message = why;
                                State::Upload {
                                    captured,
                                    upload: descriptor,
                                    log,
                                    reply: None,
                                }
                            }
                        }
                    }
                    Some(Ok(response)) => {
                        self.focused_mut().studio.voice.message = match response {
                            Ok(r) => r.error.map_or_else(
                                || "No acknowledgment; capture is kept. Retry upload.".into(),
                                |e| e.message,
                            ),
                            Err(why) => format!("Capture is kept. Retry upload. {why}"),
                        };
                        State::Upload {
                            captured,
                            upload: descriptor,
                            log,
                            reply: None,
                        }
                    }
                    Some(Err(TryRecvError::Disconnected)) => {
                        self.focused_mut().studio.voice.message =
                            "Upload worker stopped. The capture is kept; retry upload.".into();
                        State::Upload {
                            captured,
                            upload: descriptor,
                            log,
                            reply: None,
                        }
                    }
                    _ => State::Upload {
                        captured,
                        upload: descriptor,
                        log,
                        reply: reply.take(),
                    },
                }
            }
            State::Saved {
                mut residue,
                mut status,
                mut reply,
                mut next_poll,
            } => {
                if residue
                    .as_ref()
                    .is_some_and(|capture| capture.cleanup_acknowledged().is_ok())
                {
                    residue = None;
                    if let Some(kept) = self
                        .focused_mut()
                        .studio
                        .feedback
                        .voices
                        .get_mut(&status.voice.note_id)
                    {
                        kept.capture = None;
                        kept.upload = None;
                        keep = true;
                    }
                }
                if let Some(rx) = &reply {
                    match rx.try_recv() {
                        Ok(Ok(response)) => {
                            reply = None;
                            if let Some(updated) = response.voice {
                                status = updated;
                                keep = true;
                                if !self.focused().studio.voice.edited {
                                    self.focused_mut()
                                        .studio
                                        .voice
                                        .text
                                        .clone_from(&status.text);
                                }
                            }
                            if let Some(error) = response.error {
                                self.focused_mut().studio.voice.message = error.message;
                            }
                        }
                        Ok(Err(why)) => {
                            reply = None;
                            self.focused_mut().studio.voice.message = why;
                        }
                        Err(TryRecvError::Disconnected) => reply = None,
                        Err(TryRecvError::Empty) => (),
                    }
                }
                if reply.is_none()
                    && Instant::now() >= next_poll
                    && status.voice.original_asr_ref.is_none()
                {
                    if let Ok(client) = self.voice_client() {
                        let note = status.voice.note_id.clone();
                        let (tx, rx) = std::sync::mpsc::sync_channel(1);
                        if std::thread::Builder::new()
                            .name("studio-voice-status".into())
                            .spawn(move || {
                                let _ = tx.send(
                                    client
                                        .request(HostCommand::VoiceStatus { note: note.clone() })
                                        .and_then(|reply| {
                                            if reply
                                                .voice
                                                .as_ref()
                                                .is_some_and(|s| s.progress == "Audio saved")
                                            {
                                                client.request(HostCommand::StartTranscription {
                                                    note,
                                                })
                                            } else {
                                                Ok(reply)
                                            }
                                        })
                                        .map_err(|e| e.to_string()),
                                );
                            })
                            .is_ok()
                        {
                            reply = Some(rx);
                        }
                    }
                    next_poll = Instant::now() + Duration::from_millis(500);
                }
                State::Saved {
                    residue,
                    status,
                    reply,
                    next_poll,
                }
            }
            State::Unprepared(captured) => State::Unprepared(captured),
            State::Idle => State::Idle,
        };
        self.focused_mut().studio.voice.state = next;
        if keep && let Err(why) = self.keep_voice_editor() {
            self.focused_mut().studio.voice.message = why;
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one panel keeps capture, transcript and retry actions next to their visible state"
    )]
    pub(crate) fn voice_controls(&mut self, ui: &mut egui::Ui) {
        let mut stop = false;
        let mut retry = false;
        let mut choose = false;
        let mut transcribe = None;
        let mut edited = false;
        let mut selected = None;
        let drafts = self
            .focused()
            .studio
            .feedback
            .voices
            .iter()
            .map(|(note, kept)| {
                (
                    note.clone(),
                    kept.status.as_ref().map_or(0, |status| status.anchor.frame),
                )
            })
            .collect::<Vec<_>>();
        let can_select = !matches!(
            self.focused().studio.voice.state,
            State::Recording(_) | State::Upload { .. } | State::Unprepared(_)
        ) && self.focused().studio.voice.pending_choice.is_none();
        if drafts.len() > 1 {
            ui.add_enabled_ui(can_select, |ui| {
                egui::ComboBox::from_id_salt("spoken-note")
                    .selected_text("Open a spoken note")
                    .show_ui(ui, |ui| {
                        for (index, (note, frame)) in drafts.iter().enumerate() {
                            if ui
                                .selectable_label(
                                    self.focused().studio.voice.note.as_ref() == Some(note),
                                    format!("Spoken note {} · frame {frame}", index + 1),
                                )
                                .clicked()
                            {
                                selected = Some(note.clone());
                            }
                        }
                    });
            });
        }
        if let Some(note) = selected {
            let result = self
                .keep_voice_editor()
                .and_then(|()| self.restore_spoken_note(&note));
            if let Err(why) = result {
                self.focused_mut().studio.voice.message = why;
            }
        }
        ui.group(|ui| {
            let voice = &mut self.focused_mut().studio.voice;
            ui.label("Spoken note");
            if !voice.message.is_empty() {
                ui.small(&voice.message);
            }
            match &voice.state {
                State::Recording(_) => {
                    stop = ui.button("Stop recording").clicked();
                }
                State::Unprepared(_) => {
                    retry = ui.button("Retry upload").clicked();
                }
                State::Upload { reply, .. } => {
                    retry = ui
                        .add_enabled(reply.is_none(), egui::Button::new("Retry upload"))
                        .clicked();
                }
                State::Saved { status, .. } => {
                    ui.small(format!(
                        "Frame {} · {}",
                        status.anchor.frame, status.progress
                    ));
                    if !status.complete {
                        ui.small(
                            status
                                .capture_error
                                .as_deref()
                                .unwrap_or("Capture is incomplete; recorded audio is saved."),
                        );
                    }
                    edited = ui
                        .add_enabled(
                            voice.pending_choice.is_none(),
                            egui::TextEdit::multiline(&mut voice.text)
                                .desired_rows(2)
                                .hint_text("Edit the transcript, or type the note here."),
                        )
                        .changed();
                    voice.edited |= edited;
                    choose = ui
                        .button(if voice.pending_choice.is_some() {
                            "Retry transcript choice"
                        } else {
                            "Choose this transcript"
                        })
                        .clicked();
                    if ui.button("Transcribe saved audio").clicked() {
                        transcribe = Some(status.voice.note_id.clone());
                    }
                }
                State::Idle => {
                    stop = ui.button("Record spoken note").clicked();
                }
            }
        });
        if edited && let Err(why) = self.keep_voice_editor() {
            self.focused_mut().studio.voice.message = why;
        }
        if stop {
            self.voice_press();
        }
        if retry {
            let state = std::mem::take(&mut self.focused_mut().studio.voice.state);
            self.focused_mut().studio.voice.state = match state {
                State::Unprepared(captured) => self.prepare_voice_upload(captured),
                other => other,
            };
            let state = std::mem::take(&mut self.focused_mut().studio.voice.state);
            self.focused_mut().studio.voice.state = match state {
                State::Upload {
                    captured,
                    upload: descriptor,
                    log,
                    reply: None,
                } => {
                    let reply = self.voice_client().and_then(|client| {
                        self.dispatch_voice_upload(
                            client,
                            &captured,
                            descriptor.clone(),
                            log.clone(),
                        )
                    });
                    let reply = match reply {
                        Ok(reply) => Some(reply),
                        Err(why) => {
                            self.focused_mut().studio.voice.message = why;
                            None
                        }
                    };
                    State::Upload {
                        captured,
                        upload: descriptor,
                        log,
                        reply,
                    }
                }
                state => state,
            };
        }
        if let Some(note) = transcribe
            && let Ok(client) = self.voice_client()
        {
            let _ = client.request(HostCommand::StartTranscription { note });
        }
        if choose && let Err(why) = self.choose_spoken_transcript() {
            self.focused_mut().studio.voice.message = why;
        }
    }
    pub(crate) fn stage_saved_voice(&mut self, batch: bool) -> Result<(), String> {
        self.with_feedback(|feedback, port| feedback.refresh(&*port))??;
        let note = self
            .focused()
            .studio
            .feedback
            .saved
            .iter()
            .find(|note| note.revision.voice_ref.is_some())
            .cloned()
            .ok_or("No saved voice note in this bundle")?;
        let voice_ref = note.revision.voice_ref.as_ref().ok_or("Voice missing")?;
        let response = self
            .voice_client()?
            .request(HostCommand::VoiceStatus {
                note: voice_ref.note_id.clone(),
            })
            .map_err(|e| e.to_string())?;
        if let Some(error) = response.error {
            return Err(error.message);
        }
        let status = response.voice.ok_or("Saved voice status missing")?;
        let project = self.focused().studio.feedback.keeping.project().cloned();
        let voice = &mut self.focused_mut().studio.voice;
        voice.project = project;
        voice.text.clone_from(&status.text);
        voice.note = Some(status.voice.note_id.clone());
        voice.comment = Some(note.comment);
        voice.message = "Saved voice note · chosen transcript".into();
        voice.state = State::Saved {
            residue: None,
            status,
            reply: None,
            next_poll: Instant::now() + Duration::from_secs(60),
        };
        if batch {
            let batch = self
                .focused()
                .studio
                .feedback
                .batches
                .first()
                .cloned()
                .ok_or("No submitted batch")?;
            self.with_feedback(|feedback, port| feedback.reopen(&*port, &batch))??;
        }
        Ok(())
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one retained choice spans admission, pre-dispatch persistence, exact replay and settlement"
    )]
    fn choose_spoken_transcript(&mut self) -> Result<(), String> {
        self.keep_voice_editor()?;
        let client = self.voice_client()?;
        if self.focused().studio.voice.pending_choice.is_none() {
            let (note, text, comment) = {
                let voice = &self.focused().studio.voice;
                let State::Saved { status, .. } = &voice.state else {
                    return Err("Audio is not acknowledged yet".into());
                };
                (
                    status.voice.note_id.clone(),
                    voice.text.clone(),
                    voice.comment.clone().ok_or("Voice draft missing")?,
                )
            };
            let admitted = client
                .request(HostCommand::AdmitFeedbackEvidence {
                    kind: FeedbackEvidenceKind::Text,
                    bytes: text.as_bytes().to_vec(),
                })
                .map_err(|e| e.to_string())?;
            if let Some(error) = admitted.error {
                return Err(error.message);
            }
            let text_ref = admitted
                .feedback_evidence
                .ok_or("Transcript was not admitted")?;
            let issued = client
                .request(HostCommand::IssueMetadataIssuer {
                    issuer: self.focused().studio.voice.issuer.clone(),
                })
                .map_err(|e| e.to_string())?;
            if let Some(error) = issued.error {
                return Err(error.message);
            }
            let envelope = issued
                .metadata_issuer
                .ok_or("Transcript envelope missing")?
                .envelope;
            let draft_text = self
                .focused()
                .studio
                .feedback
                .drafts
                .iter()
                .find(|d| d.comment == comment)
                .ok_or("Voice draft missing")?
                .text
                .clone();
            self.focused_mut().studio.voice.pending_choice = Some(PendingChoice {
                envelope,
                note,
                text_ref,
                comment,
                text,
                draft_text,
            });
        }
        let pending = self
            .focused()
            .studio
            .voice
            .pending_choice
            .clone()
            .ok_or("Transcript choice missing")?;
        self.keep_voice_editor()?;
        let admitted = client
            .request(HostCommand::AdmitFeedbackEvidence {
                kind: FeedbackEvidenceKind::Text,
                bytes: pending.text.as_bytes().to_vec(),
            })
            .map_err(|e| e.to_string())?;
        if let Some(error) = admitted.error {
            return Err(error.message);
        }
        if admitted.feedback_evidence.as_ref() != Some(&pending.text_ref) {
            return Err("Transcript evidence changed".into());
        }
        let response = client
            .request(HostCommand::ChooseTranscript {
                envelope: pending.envelope,
                note: pending.note,
                text_ref: pending.text_ref.clone(),
            })
            .map_err(|e| e.to_string())?;
        if let Some(error) = response.error {
            if !matches!(error.code.as_str(), "unsettled" | "durability_unknown") {
                self.focused_mut().studio.voice.pending_choice = None;
                self.keep_voice_editor()?;
            }
            return Err(error.message);
        }
        let status = response
            .voice
            .ok_or("Transcript choice was not acknowledged")?;
        self.attach_voice(
            &pending.comment,
            status.voice.clone(),
            Some(pending.text_ref),
        )?;
        if let Some(draft) = self
            .focused_mut()
            .studio
            .feedback
            .draft_mut(&pending.comment)
            && draft.text == pending.draft_text
        {
            draft.text = pending.text;
        }
        let voice = &mut self.focused_mut().studio.voice;
        voice.pending_choice = None;
        voice.edited = false;
        voice.message =
            "Transcript chosen. Save the note, then select its revision for Revise cut.".into();
        let residue = match std::mem::take(&mut voice.state) {
            State::Saved { residue, .. } => residue,
            _ => None,
        };
        voice.state = State::Saved {
            residue,
            status,
            reply: None,
            next_poll: Instant::now() + Duration::from_secs(1),
        };
        self.keep_voice_editor()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{
        app::in1_tests::{in1_harness, in1_shutdown},
        studio::tests::{fixtures, wait_frame},
    };
    use kinewright_core::{Playback, TimeCode};
    use kinewright_media::test_support::{SpeechClip, normalized_words, run_ffmpeg};
    use std::sync::Arc;

    static VOICE_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn hosted_case(
        run: impl FnOnce(
            &mut KinewrightApp,
            &crate::studio::tests::Fixtures,
            &Arc<kinewright_media::FfmpegMediaEngine>,
        ),
    ) {
        let _serial = VOICE_TEST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut retire = crate::studio::tests::Retire::default();
        let mut fixture = fixtures();
        let document = fixture.contexts[0].document.clone();
        let key = fixture.contexts[0].snapshot.descriptor.key.clone();
        fixture.contexts.clear();
        let (mut app, engine) = in1_harness(document);
        retire.arm(&engine);
        app.frames = engine.controls().frames();
        app.media_events = engine.controls().events();
        let mut attachment =
            crate::ja1_attachment::Ja1Attachment::new(fixture.owners.remove(0)).unwrap();
        attachment
            .launch_host(
                "fix1-voice".into(),
                kinewright_agent::writer_host::SessionRoots {
                    read: vec![fixture.root.root().into()],
                    write: vec![fixture.root.root().into()],
                },
            )
            .unwrap();
        let context = Arc::new(attachment.studio_context(&key).unwrap());
        app.focused_mut().ja1 = Some(attachment);
        app.focused_mut().install_ja1_view();
        app.install_studio_context(context).unwrap();
        app.focused_mut().studio.feedback.directory_override =
            Some(fixture.root.path("kept-feedback"));
        wait_frame(&mut app, &egui::Context::default(), &key, TimeCode::ZERO);
        app.feedback_frame();
        run(&mut app, &fixture, &engine);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.focused_mut().ja1.as_mut().unwrap().close() {
            assert!(Instant::now() < deadline, "FIX1_CLOSE_HOST");
            std::thread::sleep(Duration::from_millis(10));
        }
        in1_shutdown(&mut app);
        drop(app);
        drop(engine);
        retire.wait();
    }
    fn recorded(
        app: &mut KinewrightApp,
        fixture: &crate::studio::tests::Fixtures,
    ) -> Box<CapturedAudio> {
        let source = fixture.root.path("quiet.wav");
        if !source.exists() {
            run_ffmpeg(
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "anullsrc=r=16000:cl=mono",
                    "-t",
                    "0.25",
                    "-c:a",
                    "pcm_s16le",
                ],
                &source,
            );
        }
        app.begin_spoken_note(crate::recording::FeedbackInput::Wav(source))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let State::Recording(capture) =
                std::mem::take(&mut app.focused_mut().studio.voice.state)
            else {
                panic!("recording");
            };
            match capture.finish() {
                Ok(captured) => return Box::new(captured),
                Err(capture) => app.focused_mut().studio.voice.state = State::Recording(*capture),
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn send_capture(app: &mut KinewrightApp, captured: Box<CapturedAudio>, refuse: bool) {
        let client = app.voice_client().unwrap();
        let grant = client
            .request(HostCommand::IssueMetadataIssuer { issuer: None })
            .unwrap()
            .metadata_issuer
            .unwrap();
        let (mut descriptor, log) = captured
            .prepare_upload(grant.envelope, grant.root_generation)
            .unwrap();
        if refuse {
            descriptor.root_generation += 99;
        }
        app.keep_voice_capture(&captured, Some(&descriptor))
            .unwrap();
        let reply = upload(client, &captured, descriptor.clone(), log.clone(), None).unwrap();
        app.focused_mut().studio.voice.state = State::Upload {
            captured,
            upload: descriptor,
            log,
            reply: Some(reply),
        };
    }
    fn wait_saved(app: &mut KinewrightApp) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            app.voice_frame();
            if matches!(app.focused().studio.voice.state, State::Saved { .. }) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "FIX1_ACK {}",
                app.focused().studio.voice.message
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn wait_refused(app: &mut KinewrightApp) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            app.voice_frame();
            if matches!(
                app.focused().studio.voice.state,
                State::Upload { reply: None, .. }
            ) {
                return;
            }
            assert!(Instant::now() < deadline, "FIX1_REFUSAL");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn reopen(
        app: &mut KinewrightApp,
        fixture: &crate::studio::tests::Fixtures,
        engine: &Arc<kinewright_media::FfmpegMediaEngine>,
    ) {
        let mut attachment = app.focused_mut().ja1.take().unwrap();
        let path = attachment.project_path().unwrap().clone();
        let key = app
            .focused()
            .studio
            .selection
            .as_ref()
            .unwrap()
            .snapshot
            .descriptor
            .key
            .clone();
        let document = app
            .focused()
            .studio
            .selection
            .as_ref()
            .unwrap()
            .document
            .clone();
        app.focused_mut().studio.feedback.persist().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !attachment.close() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(attachment);
        let (replacement, _) =
            crate::app::in1_tests::in1_harness_with_engine(document, Arc::clone(engine));
        let mut old = std::mem::replace(app, replacement);
        in1_shutdown(&mut old);
        drop(old);
        let owner = kinewright_project::editing_session::Ja1EditingSession::open(&path).unwrap();
        let mut attachment = crate::ja1_attachment::Ja1Attachment::new(owner).unwrap();
        attachment
            .launch_host(
                "fix1-voice-reopened".into(),
                kinewright_agent::writer_host::SessionRoots {
                    read: vec![fixture.root.root().into()],
                    write: vec![fixture.root.root().into()],
                },
            )
            .unwrap();
        let context = Arc::new(attachment.studio_context(&key).unwrap());
        app.frames = engine.controls().frames();
        app.media_events = engine.controls().events();
        app.focused_mut().ja1 = Some(attachment);
        app.focused_mut().install_ja1_view();
        app.install_studio_context(context).unwrap();
        app.focused_mut().studio.feedback.directory_override =
            Some(fixture.root.path("kept-feedback"));
        wait_frame(app, &egui::Context::default(), &key, TimeCode::ZERO);
        app.feedback_frame();
        app.voice_frame();
    }
    #[test]
    fn fix1_acknowledgment_keeps_typed_text_live_and_reopened() {
        hosted_case(|app, fixture, engine| {
            let captured = recorded(app, fixture);
            let comment = app.focused().studio.voice.comment.clone().unwrap();
            send_capture(app, captured, true);
            let typed = "Keep these exact words.\nCafé, chart 42, and the title.  ";
            app.focused_mut()
                .studio
                .feedback
                .draft_mut(&comment)
                .unwrap()
                .text = typed.into();
            app.focused_mut().studio.feedback.persist().unwrap();
            wait_refused(app);
            let State::Upload {
                captured,
                upload: descriptor,
                log,
                ..
            } = std::mem::take(&mut app.focused_mut().studio.voice.state)
            else {
                panic!("upload")
            };
            assert!(captured.path.exists());
            let reply = app
                .dispatch_voice_upload(
                    app.voice_client().unwrap(),
                    &captured,
                    descriptor.clone(),
                    log.clone(),
                )
                .unwrap();
            app.focused_mut().studio.voice.state = State::Upload {
                captured,
                upload: descriptor,
                log,
                reply: Some(reply),
            };
            wait_saved(app);
            assert_eq!(
                app.focused_mut()
                    .studio
                    .feedback
                    .draft_mut(&comment)
                    .unwrap()
                    .text
                    .as_bytes(),
                typed.as_bytes(),
                "FIX1_ACK_KEEPS_TYPED_TEXT"
            );
            reopen(app, fixture, engine);
            assert_eq!(
                app.focused_mut()
                    .studio
                    .feedback
                    .draft_mut(&comment)
                    .unwrap()
                    .text
                    .as_bytes(),
                typed.as_bytes(),
                "FIX1_REOPEN_KEEPS_TYPED_TEXT"
            );
        });
    }
    #[test]
    fn fix1_transcript_edit_survives_refused_choice_and_another_note() {
        hosted_case(|app, fixture, engine| {
            let captured = recorded(app, fixture);
            send_capture(app, captured, false);
            wait_saved(app);
            let note = app.focused().studio.voice.note.clone().unwrap();
            let corrected = "Keep my correction verbatim.\nWarm amber, not orange.  ";
            app.focused_mut().studio.voice.text = corrected.into();
            app.focused_mut().studio.voice.edited = true;
            app.keep_voice_editor().unwrap();
            let client = app.voice_client().unwrap();
            let text_ref = client
                .request(HostCommand::AdmitFeedbackEvidence {
                    kind: FeedbackEvidenceKind::Text,
                    bytes: corrected.as_bytes().to_vec(),
                })
                .unwrap()
                .feedback_evidence
                .unwrap();
            let mut envelope = client
                .request(HostCommand::IssueMetadataIssuer { issuer: None })
                .unwrap()
                .metadata_issuer
                .unwrap()
                .envelope;
            envelope.owner_epoch += 1;
            let comment = app.focused().studio.voice.comment.clone().unwrap();
            app.focused_mut().studio.voice.pending_choice = Some(PendingChoice {
                envelope,
                note: note.clone(),
                text_ref,
                comment,
                text: corrected.into(),
                draft_text: String::new(),
            });
            assert!(app.choose_spoken_transcript().is_err());
            assert!(app.focused().studio.voice.pending_choice.is_none());
            reopen(app, fixture, engine);
            assert_eq!(
                app.focused().studio.voice.text.as_bytes(),
                corrected.as_bytes(),
                "FIX1_TRANSCRIPT_REOPEN"
            );
            let second = recorded(app, fixture);
            assert_eq!(
                app.focused().studio.feedback.voices[&note].text.as_bytes(),
                corrected.as_bytes(),
                "FIX1_TRANSCRIPT_NEW_NOTE"
            );
            send_capture(app, second, false);
            wait_saved(app);
            app.restore_spoken_note(&note).unwrap();
            assert_eq!(
                app.focused().studio.voice.text.as_bytes(),
                corrected.as_bytes()
            );
        });
    }
    #[test]
    fn fix1_pending_choice_keeps_original_envelope_and_intervening_text() {
        hosted_case(|app, fixture, engine| {
            let captured = recorded(app, fixture);
            send_capture(app, captured, false);
            wait_saved(app);
            let note = app.focused().studio.voice.note.clone().unwrap();
            let comment = app.focused().studio.voice.comment.clone().unwrap();
            let original = "Typed before choosing.  ";
            let later = "Typed after the pending choice.\nKeep these bytes.  ";
            let correction = "My precise transcript correction.";
            app.focused_mut()
                .studio
                .feedback
                .draft_mut(&comment)
                .unwrap()
                .text = original.into();
            let client = app.voice_client().unwrap();
            let text_ref = client
                .request(HostCommand::AdmitFeedbackEvidence {
                    kind: FeedbackEvidenceKind::Text,
                    bytes: correction.as_bytes().to_vec(),
                })
                .unwrap()
                .feedback_evidence
                .unwrap();
            let envelope = client
                .request(HostCommand::IssueMetadataIssuer { issuer: None })
                .unwrap()
                .metadata_issuer
                .unwrap()
                .envelope;
            let exact = serde_json::to_vec(&envelope).unwrap();
            app.focused_mut().studio.voice.text = correction.into();
            app.focused_mut().studio.voice.edited = true;
            app.focused_mut().studio.voice.pending_choice = Some(PendingChoice {
                envelope,
                note: note.clone(),
                text_ref,
                comment: comment.clone(),
                text: correction.into(),
                draft_text: original.into(),
            });
            app.keep_voice_editor().unwrap();
            let disk: serde_json::Value = serde_json::from_slice(
                &std::fs::read(fixture.root.path("kept-feedback").join(format!(
                        "{}.json",
                        app.focused()
                            .studio
                            .feedback
                            .keeping
                            .project()
                            .unwrap()
                            .as_str()
                    )))
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                serde_json::to_vec(&disk["voices"][note.as_str()]["pending_choice"]["envelope"])
                    .unwrap(),
                serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(&exact).unwrap())
                    .unwrap(),
                "FIX1_CHOICE_KEPT_BEFORE_DISPATCH"
            );
            app.focused_mut()
                .studio
                .feedback
                .draft_mut(&comment)
                .unwrap()
                .text = later.into();
            app.choose_spoken_transcript().unwrap();
            assert_eq!(
                app.focused_mut()
                    .studio
                    .feedback
                    .draft_mut(&comment)
                    .unwrap()
                    .text
                    .as_bytes(),
                later.as_bytes(),
                "FIX1_CHOICE_KEEPS_INTERVENING_TEXT"
            );
            reopen(app, fixture, engine);
            assert_eq!(
                app.focused_mut()
                    .studio
                    .feedback
                    .draft_mut(&comment)
                    .unwrap()
                    .text
                    .as_bytes(),
                later.as_bytes()
            );
        });
    }
    #[test]
    fn fix1_uncertain_choice_reopens_original_envelope() {
        hosted_case(|app, fixture, engine| {
            let captured = recorded(app, fixture);
            send_capture(app, captured, false);
            wait_saved(app);
            let client = app.voice_client().unwrap();
            let correction = "Keep the uncertain correction exactly.  ";
            let text_ref = client
                .request(HostCommand::AdmitFeedbackEvidence {
                    kind: FeedbackEvidenceKind::Text,
                    bytes: correction.as_bytes().to_vec(),
                })
                .unwrap()
                .feedback_evidence
                .unwrap();
            let envelope = client
                .request(HostCommand::IssueMetadataIssuer { issuer: None })
                .unwrap()
                .metadata_issuer
                .unwrap()
                .envelope;
            let exact = serde_json::to_vec(&envelope).unwrap();
            app.focused_mut().studio.voice.text = correction.into();
            app.focused_mut().studio.voice.edited = true;
            app.focused_mut().studio.voice.pending_choice = Some(PendingChoice {
                envelope,
                note: app.focused().studio.voice.note.clone().unwrap(),
                text_ref,
                comment: app.focused().studio.voice.comment.clone().unwrap(),
                text: correction.into(),
                draft_text: String::new(),
            });
            assert!(app.focused_mut().ja1.as_mut().unwrap().close());
            assert!(app.choose_spoken_transcript().is_err());
            reopen(app, fixture, engine);
            assert_eq!(
                app.focused().studio.voice.text.as_bytes(),
                correction.as_bytes(),
                "FIX1_UNCERTAIN_TEXT_REOPEN"
            );
            assert_eq!(
                serde_json::to_vec(
                    &app.focused()
                        .studio
                        .voice
                        .pending_choice
                        .as_ref()
                        .unwrap()
                        .envelope
                )
                .unwrap(),
                exact,
                "FIX1_UNCERTAIN_ENVELOPE_REOPEN"
            );
        });
    }
    #[test]
    fn fix1_full_voice_drafts_refuse_before_capture() {
        hosted_case(|app, _fixture, _engine| {
            let comment = CommentId::fresh().unwrap();
            for _ in 0..128 {
                app.focused_mut().studio.feedback.voices.insert(
                    NoteId::fresh().unwrap(),
                    KeptVoice {
                        comment: comment.clone(),
                        capture: None,
                        upload: None,
                        status: None,
                        text: "Unfinished correction".into(),
                        edited: true,
                        pending_choice: None,
                    },
                );
            }
            let drafts = app.focused().studio.feedback.drafts.len();
            let result = app.begin_spoken_note(crate::recording::FeedbackInput::Wav(
                std::path::PathBuf::from("not-started.wav"),
            ));
            assert!(result.is_err(), "FIX1_FULL_DRAFTS_REFUSE");
            assert!(
                matches!(app.focused().studio.voice.state, State::Idle),
                "FIX1_FULL_DRAFTS_NO_CAPTURE"
            );
            assert_eq!(app.focused().studio.feedback.drafts.len(), drafts);
            assert_eq!(app.focused().studio.feedback.voices.len(), 128);
        });
    }
    #[test]
    fn fix1_pre_ack_capture_reopens_and_retries_same_bytes() {
        hosted_case(|app, fixture, engine| {
            let captured = recorded(app, fixture);
            let path = captured.path.clone();
            let bytes = std::fs::read(&path).unwrap();
            send_capture(app, captured, true);
            wait_refused(app);
            let note = app.focused().studio.voice.note.clone().unwrap();
            let kept =
                serde_json::to_vec(&app.focused().studio.feedback.voices[&note].capture).unwrap();
            let upload = app.focused().studio.feedback.voices[&note]
                .upload
                .clone()
                .unwrap();
            reopen(app, fixture, engine);
            assert_eq!(
                serde_json::to_vec(&app.focused().studio.feedback.voices[&note].capture).unwrap(),
                kept
            );
            wait_saved(app);
            let State::Saved { status, .. } = &app.focused().studio.voice.state else {
                panic!("saved")
            };
            assert_eq!(status.voice.note_id, note);
            assert_eq!(status.voice.audio_ref, upload.audio);
            assert_eq!(status.voice.playhead_log_ref, Some(upload.log));
            assert_eq!(
                kinewright_media::sha256_bytes(&bytes),
                status.voice.audio_ref.sha256
            );
            assert!(!path.exists());
            assert_eq!(
                app.voice_client()
                    .unwrap()
                    .request(HostCommand::ListFeedback)
                    .unwrap()
                    .feedback_index
                    .unwrap()
                    .notes
                    .len(),
                1
            );
        });
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one real-model witness retains the complete capture, publication, edit, batch and cleanup chain"
    )]
    fn c9_studio_real_spoken_note_edit_and_batch() {
        let _serial = VOICE_TEST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let model = kinewright_media::default_data_dir()
            .join("models/whisper")
            .join(kinewright_media::WHISPER_MODEL_NAME);
        if kinewright_media::sha256_file(&model).ok().as_deref()
            != Some(kinewright_media::WHISPER_MODEL_SHA256)
        {
            assert!(
                std::env::var_os("KINEWRIGHT_C9_CONTROL_REQUIRED").is_none(),
                "C9_REAL_MODEL_REQUIRED"
            );
            eprintln!(
                "NOT RUN c9_studio_real_spoken_note_edit_and_batch: pinned Whisper small is not installed"
            );
            return;
        }
        let mut retire = crate::studio::tests::Retire::default();
        let mut fixture = fixtures();
        let context = Arc::clone(&fixture.contexts[0]);
        let key = context.snapshot.descriptor.key.clone();
        let (mut app, engine) = in1_harness(context.document.clone());
        retire.arm(&engine);
        app.frames = engine.controls().frames();
        app.media_events = engine.controls().events();
        drop(context);
        fixture.contexts.clear();
        let mut attachment =
            crate::ja1_attachment::Ja1Attachment::new(fixture.owners.remove(0)).unwrap();
        attachment
            .launch_host(
                "c8-studio-control".into(),
                kinewright_agent::writer_host::SessionRoots {
                    read: vec![fixture.root.root().into()],
                    write: vec![fixture.root.root().into()],
                },
            )
            .unwrap();
        let fresh_context = attachment.studio_context(&key);
        eprintln!(
            "C8_HOST_READER_RESULT {:?}",
            fresh_context
                .as_ref()
                .map(|_| "context")
                .map_err(ToString::to_string)
        );
        let context = Arc::new(fresh_context.expect("C8_AUTHENTICATED_HOST_SNAPSHOT_READ_PORT"));
        app.focused_mut().ja1 = Some(attachment);
        app.focused_mut().install_ja1_view();
        app.install_studio_context(context).unwrap();
        let ctx = egui::Context::default();
        wait_frame(&mut app, &ctx, &key, TimeCode::ZERO);
        app.feedback_frame();
        let speech = SpeechClip::plain(
            "c8-studio-note",
            "Please make this chart warmer.",
            "KINEWRIGHT_C8_CONTROL_AUDIO",
        );
        let wav = fixture.root.path("spoken.wav");
        run_ffmpeg(
            &[
                "-i",
                speech.mp4.to_str().unwrap(),
                "-vn",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-c:a",
                "pcm_s16le",
            ],
            &wav,
        );
        app.begin_spoken_note(crate::recording::FeedbackInput::Wav(wav))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(240);
        let mut capture_path = None;
        eprintln!(
            "C9_G11_MODEL_SHA256 {}",
            kinewright_media::WHISPER_MODEL_SHA256
        );
        loop {
            app.voice_frame();
            if let State::Upload { captured, .. } = &app.focused().studio.voice.state {
                capture_path = Some(captured.path.clone());
            }
            if let State::Saved { status, .. } = &app.focused().studio.voice.state {
                if !status.text.is_empty() {
                    assert_eq!(status.anchor.snapshot, key, "C8_STUDIO_PINNED_VERSION");
                    assert_eq!(status.anchor.frame, 0, "C8_STUDIO_PINNED_FRAME");
                    assert!(
                        normalized_words(&status.text)
                            .iter()
                            .any(|word| word == "warmer"),
                        "C8_REAL_WHISPER_TEXT {}",
                        status.text
                    );
                    eprintln!("C8_REAL_WHISPER_TEXT {}", status.text);
                    break;
                }
                assert!(
                    !status.progress.contains("not installed"),
                    "C8_REAL_MODEL_REQUIRED {}",
                    status.progress
                );
            }
            assert!(
                Instant::now() < deadline,
                "C8_STUDIO_DEADLINE {}",
                app.focused().studio.voice.message
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let voice = &mut app.focused_mut().studio.voice;
        voice.text = "Make this chart warmer and keep the title.".into();
        voice.edited = true;
        app.choose_spoken_transcript().unwrap();
        let comment = app.focused().studio.voice.comment.clone().unwrap();
        let outcome = app
            .with_feedback(|feedback, port| feedback.save(port, &comment))
            .unwrap();
        assert!(
            matches!(outcome, crate::studio_feedback::Outcome::Saved(_)),
            "C8_STUDIO_SAVE {outcome:?}"
        );
        let reference = app
            .focused()
            .studio
            .feedback
            .saved
            .iter()
            .find(|n| n.revision.comment_id == comment)
            .unwrap()
            .reference
            .clone();
        app.with_feedback(|feedback, port| {
            feedback.toggle(&reference);
            feedback.submit(port)
        })
        .unwrap();
        let client = app.voice_client().unwrap();
        let index = client
            .request(HostCommand::ListFeedback)
            .unwrap()
            .feedback_index
            .unwrap();
        assert_eq!(index.batches.len(), 1, "C8_STUDIO_BATCH");
        let batch = index.batches.keys().next().unwrap().clone();
        let read = client
            .request(HostCommand::ReadSubmittedBatch { batch })
            .unwrap();
        let revision = &read.submitted_revisions[0];
        assert!(revision.voice_ref.is_some(), "C8_STUDIO_BATCH_VOICE");
        assert!(
            revision.transcript_revision.is_some(),
            "C8_STUDIO_BATCH_TRANSCRIPT"
        );
        assert_eq!(revision.anchor.snapshot, key);
        let State::Saved {
            residue, status, ..
        } = &app.focused().studio.voice.state
        else {
            panic!("saved voice")
        };
        assert!(residue.is_none(), "C8_CAPTURE_FILES_CLEANED");
        let capture_path = capture_path.expect("capture traversed the upload state");
        assert!(
            !capture_path.exists() && !capture_path.with_extension("log").exists(),
            "C9_ACKNOWLEDGED_CAPTURE_FILES_DELETED"
        );
        eprintln!(
            "C8_STUDIO_VOICE {:?}, batch {:?}",
            status.voice.note_id,
            index.batches.keys().next()
        );
        let source = app.focused().ja1.as_ref().unwrap().project_path().unwrap();
        assert!(
            app.focused_mut().ja1.as_mut().unwrap().close(),
            "C9_CONTROL_CLOSE_REAPS_HOST"
        );
        in1_shutdown(&mut app);
        drop(app);
        drop(engine);
        retire.wait();
        if let Some(export) = std::env::var_os("KINEWRIGHT_C9_CONTROL_EXPORT") {
            let export = std::path::PathBuf::from(export);
            std::fs::create_dir(&export).unwrap();
            let destination = export.join(source.file_name().unwrap());
            std::fs::copy(&source, &destination).unwrap();
            copy_control_objects(
                &source.with_extension("kinewright-assets"),
                &destination.with_extension("kinewright-assets"),
            );
            eprintln!("C9_CONTROL_BUNDLE {}", destination.display());
        }
    }
    fn copy_control_objects(source: &std::path::Path, destination: &std::path::Path) {
        std::fs::create_dir(destination).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let to = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_control_objects(&entry.path(), &to);
            } else if entry.file_type().unwrap().is_file() {
                std::fs::copy(entry.path(), to).unwrap();
            } else {
                panic!("control bundle has a non-regular object");
            }
        }
    }
}

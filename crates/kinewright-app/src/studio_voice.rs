//! Spoken feedback retains its capture until the authenticated owner acknowledges it.
use crate::{
    app::KinewrightApp,
    recording::feedback_capture::{Capture, CaptureReservation, CapturedAudio},
};
use eframe::egui;
use kinewright_agent::writer_host::{HostClient, HostCommand, HostReply};
use kinewright_project::{
    receipts::IssuerId,
    studio_records::{CommentId, FeedbackEvidenceKind},
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
    comment: Option<CommentId>,
    issuer: Option<IssuerId>,
    message: String,
    text: String,
    edited: bool,
    pending_choice: Option<PendingChoice>,
}
#[derive(Clone)]
struct PendingChoice {
    envelope: kinewright_project::studio_records::MetadataEnvelope,
    note: kinewright_project::studio_records::NoteId,
    text_ref: kinewright_project::bundle_records::ObjectRef,
    comment: CommentId,
    text: String,
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
    mut descriptor: VoiceUpload,
    log: Vec<u8>,
) -> Result<Receiver<UploadReply>, String> {
    let path = captured.path.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("studio-voice-upload".into())
        .spawn(move || {
            let result = recover_upload(&client, &mut descriptor).and_then(|settled| {
                if let Some(reply) = settled {
                    return Ok(reply);
                }
                let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                client
                    .acknowledge_voice(descriptor.clone(), file.chain(std::io::Cursor::new(log)))
                    .map_err(|e| e.to_string())
            });
            let _ = tx.send(UploadReply { descriptor, result });
        })
        .map_err(|e| e.to_string())?;
    Ok(rx)
}
impl KinewrightApp {
    fn prepare_voice_upload(&mut self, captured: Box<CapturedAudio>) -> State {
        let prepared = (|| -> Result<_, String> {
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
            let rx = upload(client, &captured, descriptor.clone(), log.clone())?;
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
        let voice = &mut self.focused_mut().studio.voice;
        voice.comment = Some(comment);
        voice.text.clear();
        voice.edited = false;
        voice.state = State::Recording(capture);
        voice.message = "Recording at the displayed picture. Press Stop when finished.".into();
        Ok(())
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one frame advances the current capture, upload or ASR state while retaining its recovery carrier"
    )]
    pub(crate) fn voice_frame(&mut self) {
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
                        match captured.cleanup_acknowledged() {
                            Ok(()) => {
                                let comment = self.focused().studio.voice.comment.clone();
                                if let Some(comment) = comment
                                    && let Err(why) = self.attach_voice(
                                        &comment,
                                        status.voice.clone(),
                                        status.voice.chosen_transcript_revision.clone(),
                                        status.text.clone(),
                                    )
                                {
                                    self.focused_mut().studio.voice.message = why;
                                }
                                if let Ok(client) = self.voice_client() {
                                    let _ = client.request(HostCommand::StartTranscription {
                                        note: status.voice.note_id.clone(),
                                    });
                                }
                                State::Saved {
                                    residue: None,
                                    status,
                                    reply: None,
                                    next_poll: Instant::now(),
                                }
                            }
                            Err(error) => {
                                self.focused_mut().studio.voice.message =
                                    format!("Audio saved. Capture cleanup needs retry: {error}");
                                State::Saved {
                                    residue: Some(captured),
                                    status,
                                    reply: None,
                                    next_poll: Instant::now(),
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
                }
                if let Some(rx) = &reply {
                    match rx.try_recv() {
                        Ok(Ok(response)) => {
                            reply = None;
                            if let Some(updated) = response.voice {
                                status = updated;
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
    }
    pub(crate) fn voice_controls(&mut self, ui: &mut egui::Ui) {
        let mut stop = false;
        let mut retry = false;
        let mut choose = false;
        let mut transcribe = None;
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
                    voice.edited |= ui
                        .add_enabled(
                            voice.pending_choice.is_none(),
                            egui::TextEdit::multiline(&mut voice.text)
                                .desired_rows(2)
                                .hint_text("Edit the transcript, or type the note here."),
                        )
                        .changed();
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
        if stop {
            self.voice_press();
        }
        if retry {
            let state = std::mem::take(&mut self.focused_mut().studio.voice.state);
            self.focused_mut().studio.voice.state = match state {
                State::Unprepared(captured) => self.prepare_voice_upload(captured),
                other => other,
            };
            if let Ok(client) = self.voice_client() {
                let voice = &mut self.focused_mut().studio.voice;
                if let State::Upload {
                    captured,
                    upload: descriptor,
                    log,
                    reply,
                } = &mut voice.state
                {
                    match upload(client, captured, descriptor.clone(), log.clone()) {
                        Ok(rx) => *reply = Some(rx),
                        Err(why) => voice.message = why,
                    }
                }
            }
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
        let voice = &mut self.focused_mut().studio.voice;
        voice.text.clone_from(&status.text);
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
    fn choose_spoken_transcript(&mut self) -> Result<(), String> {
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
            self.focused_mut().studio.voice.pending_choice = Some(PendingChoice {
                envelope,
                note,
                text_ref,
                comment,
                text,
            });
        }
        let pending = self
            .focused()
            .studio
            .voice
            .pending_choice
            .clone()
            .ok_or("Transcript choice missing")?;
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
            pending.text,
        )?;
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
        Ok(())
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

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one real-model witness retains the complete capture, publication, edit, batch and cleanup chain"
    )]
    fn c9_studio_real_spoken_note_edit_and_batch() {
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

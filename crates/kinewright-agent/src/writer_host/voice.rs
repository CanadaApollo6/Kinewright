//! Voice commands share the authenticated owner and C5's per-note publication slot.
use super::*;
use kinewright_media::voice_note::{OriginalVoiceAsr, VoiceTask, VoiceTranscript};
use kinewright_project::{
    studio_records::{FeedbackAnchor, JobId, MetadataEnvelope, NoteId, VoiceRef},
    voice_ingress::VoiceUpload,
    voice_store::{JobKey, VoiceNoteHandle},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VoiceReceipt {
    Absent,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStatus {
    pub voice: VoiceRef,
    pub anchor: FeedbackAnchor,
    pub complete: bool,
    pub capture_error: Option<String>,
    pub progress: String,
    pub text: String,
}
pub(super) struct Job {
    pub(super) note: VoiceNoteHandle,
    pub(super) key: JobKey,
    pub(super) task: Option<VoiceTask<JobKey>>,
    pub(super) completion: Option<VoiceTranscript>,
    pub(super) envelope: MetadataEnvelope,
    pub(super) progress: String,
}

impl HostClient {
    /// Settle the original command before rebinding a capture to a replacement owner.
    pub fn recover_voice_upload(
        &self,
        descriptor: &mut VoiceUpload,
    ) -> Result<Option<HostReply>, HostError> {
        let reply = self.request(HostCommand::VoiceReceipt {
            upload: descriptor.clone(),
        })?;
        if let Some(error) = reply.error {
            return Err(error.into());
        }
        match reply.voice_receipt {
            Some(VoiceReceipt::Committed) => Ok(Some(reply)),
            Some(VoiceReceipt::Absent) => {
                let reply = self.request(HostCommand::IssueMetadataIssuer {
                    issuer: Some(descriptor.envelope.id.issuer.clone()),
                })?;
                if let Some(error) = reply.error {
                    return Err(error.into());
                }
                let grant = reply
                    .metadata_issuer
                    .ok_or_else(|| HostRefusal::new("voice_envelope", "No voice retry envelope"))?;
                if grant.envelope.owner_epoch != descriptor.envelope.owner_epoch
                    || grant.root_generation != descriptor.root_generation
                {
                    descriptor.envelope = grant.envelope;
                    descriptor.root_generation = grant.root_generation;
                }
                Ok(None)
            }
            None => Err(HostRefusal::new(
                "unsettled",
                "Original voice receipt is not settled; capture is kept.",
            )
            .into()),
        }
    }
    /// The body is a stream of WAV then log bytes; neither belongs in a command envelope.
    pub fn acknowledge_voice(
        &self,
        upload: VoiceUpload,
        stream: impl Read + Send + 'static,
    ) -> Result<HostReply, HostError> {
        let request = HostRequest {
            editor: self.editor.clone(),
            operation: None,
            protocol: HOST_PROTOCOL,
            project: self.endpoint().project.clone(),
            roots: self.bootstrap.roots.clone(),
            command: HostCommand::AcknowledgeVoice { upload },
        };
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| HostError::Protocol(e.to_string()))?
            .post(format!("http://{}/voice", self.endpoint().address))
            .header("authorization", self.bootstrap.auth.bearer())
            .header("x-kinewright-project", &self.endpoint().project)
            .header("x-kinewright-protocol", "1")
            .header("x-kinewright-voice", serde_json::to_string(&request)?)
            .body(reqwest::blocking::Body::new(stream))
            .send()
            .map_err(|e| HostError::Protocol(e.to_string()))?;
        if !response.status().is_success() {
            return Err(HostRefusal::new(
                "voice_ingress",
                "Voice upload was refused; the capture is still on this computer.",
            )
            .into());
        }
        let mut bytes = Vec::new();
        response
            .take((HOST_BODY_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > HOST_BODY_BYTES {
            return Err(HostRefusal::new("capacity", "voice reply bytes").into());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}

pub(super) async fn upload_request(
    State(state): State<Arc<HttpState>>,
    request: Request,
) -> Response {
    let bearer = match check_host_headers(request.headers(), &state) {
        Ok(b) => b,
        Err(s) => return s.into_response(),
    };
    let Some(metadata) = request
        .headers()
        .get("x-kinewright-voice")
        .and_then(|h| h.to_str().ok())
        .filter(|s| s.len() <= 16 * 1024)
        .and_then(|s| serde_json::from_str::<HostRequest>(s).ok())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let HostCommand::AcknowledgeVoice { upload } = &metadata.command else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if upload.validate().is_err() {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let length = upload.audio.byte_length + upload.log.byte_length;
    let Ok(permit) = Arc::clone(&state.ingress).try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    // GUI retention proof performs blocking callback IO, like ordinary host
    // commands. It must not run on the async HTTP executor.
    let admission_state = Arc::clone(&state);
    let admission_bearer = bearer.clone();
    let admission_request = metadata.clone();
    let admission = tokio::task::spawn_blocking(move || -> Result<_, HostError> {
        let mut owner = admission_state
            .owner
            .lock()
            .map_err(|_| HostRefusal::new("owner_unavailable", "Voice owner unavailable"))?;
        let (_, _, roots) = owner.authorize_request(Some(&admission_bearer), &admission_request)?;
        owner.prove_retention()?;
        for pin in &owner.pins {
            if !roots.permits(&pin.anchor, true) {
                return Err(HostRefusal::roots("voice project write root").into());
            }
        }
        Ok(owner.owner()?.reserve_voice_capture()?)
    })
    .await;
    let reservation = match admission {
        Ok(Ok(reservation)) => reservation,
        Ok(Err(error)) => {
            return json_response(&HostReply {
                error: Some(error.refusal()),
                ..HostReply::default()
            });
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let bytes = match tokio::time::timeout(
        Duration::from_secs(15),
        to_bytes(request.into_body(), usize::try_from(length).unwrap_or(0)),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
    };
    if bytes.len() as u64 != length {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let Ok(mut owner) = state.owner.lock() else {
            return HostReply {
                error: Some(HostRefusal::new(
                    "owner_unavailable",
                    "Voice owner unavailable",
                )),
                ..HostReply::default()
            };
        };
        owner.voice_payload = Some((bytes.to_vec(), reservation));
        let reply = owner.execute_bearer(Some(&bearer), metadata);
        owner.voice_payload = None;
        reply
    })
    .await;
    match result {
        Ok(reply) => json_response(&reply),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

impl HostCommandOwner {
    pub(super) fn voice_command(
        &mut self,
        command: HostCommand,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        match command {
            HostCommand::VoiceReceipt { upload } => {
                use kinewright_project::receipts::ReceiptLookup;
                reply.voice_receipt = Some(match self.owner()?.voice_receipt(&upload)? {
                    ReceiptLookup::Absent => VoiceReceipt::Absent,
                    ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_) => {
                        let status = self.voice_status(&upload.note)?;
                        if status.voice.audio_ref != upload.audio
                            || status.voice.playhead_log_ref.as_ref() != Some(&upload.log)
                        {
                            return Err(BundleError::HistoryChanged.into());
                        }
                        reply.voice = Some(status);
                        VoiceReceipt::Committed
                    }
                    ReceiptLookup::IdPayloadConflict => {
                        return Err(HostRefusal::new(
                            "id_payload_conflict",
                            "Original voice command payload changed",
                        )
                        .into());
                    }
                    _ => return Err(BundleError::Unsettled.into()),
                });
            }
            HostCommand::AcknowledgeVoice { upload } => {
                let (bytes, reservation) = self.voice_payload.take().ok_or_else(|| {
                    HostRefusal::new("stream_required", "Use the bounded voice upload endpoint")
                })?;
                let voice = self
                    .owner_mut()?
                    .acknowledge_voice(&upload, &*bytes, reservation)?;
                // The publisher's receipt is the acknowledgment boundary; creating a job view
                // must not turn an already committed capture into a failed acknowledgment.
                let log: kinewright_media::voice_note::PlayheadLog<FeedbackAnchor> =
                    serde_json::from_slice(
                        &bytes[usize::try_from(upload.audio.byte_length)
                            .map_err(|_| BundleError::Capacity("voice length"))?..],
                    )?;
                reply.voice = Some(
                    if voice.original_asr_ref.is_some()
                        || voice.chosen_transcript_revision.is_some()
                    {
                        self.voice_status(&voice.note_id)?
                    } else {
                        VoiceStatus {
                            voice,
                            anchor: log.samples[0].displayed.clone(),
                            complete: log.capture_status.as_ref().is_some_and(|s| s.complete),
                            capture_error: log.capture_status.and_then(|s| s.capture_error),
                            progress: "Audio saved".into(),
                            text: String::new(),
                        }
                    },
                );
            }
            HostCommand::StartTranscription { note } => {
                if self
                    .voice_jobs
                    .values()
                    .filter(|job| job.task.is_some() || job.completion.is_some())
                    .count()
                    >= 2
                {
                    return Err(BundleError::Capacity("voice inference and pending slot").into());
                }
                if self.voice_jobs.contains_key(&note) {
                    self.poll_voice(&note)?;
                }
                let handle = self.owner()?.voice_note(&note)?;
                let voice = handle.snapshot()?.voice;
                if voice.original_asr_ref.is_none()
                    && !self
                        .voice_jobs
                        .get(&note)
                        .is_some_and(|j| j.task.is_some() || j.completion.is_some())
                {
                    let issuer = self.owner_mut()?.issue_metadata_issuer()?;
                    let envelope = self.owner()?.metadata_envelope(
                        kinewright_project::receipts::CommandId {
                            issuer,
                            sequence: 1,
                        },
                    )?;
                    let key = self.owner()?.start_voice_job(&handle, &voice, 0)?;
                    let task = match self.owner()?.spawn_voice_job(
                        &handle,
                        self.voice_data_dir.clone(),
                        &key,
                    ) {
                        Ok(task) => task,
                        Err(error) => {
                            handle.cancel();
                            return Err(error.into());
                        }
                    };
                    self.voice_jobs.insert(
                        note.clone(),
                        Job {
                            note: handle,
                            key,
                            task: Some(task),
                            completion: None,
                            envelope,
                            progress: "Transcribing…".into(),
                        },
                    );
                }
                reply.voice = Some(self.voice_status(&note)?);
            }
            HostCommand::VoiceStatus { note } => {
                self.poll_voice(&note)?;
                reply.voice = Some(self.voice_status(&note)?);
            }
            HostCommand::ChooseTranscript {
                envelope,
                note,
                text_ref,
            } => {
                self.owner_mut()?
                    .choose_voice_transcript(&envelope, &note, text_ref)?;
                reply.voice = Some(self.voice_status(&note)?);
            }
            HostCommand::ReadVoiceRange {
                revision,
                start_micros,
                end_micros,
            } => {
                let feedback = self.owner()?.read_feedback(&revision)?;
                let voice = feedback.revision.voice_ref.as_ref().ok_or_else(|| {
                    HostRefusal::new("voice_missing", "This revision has no voice note")
                })?;
                let allowed =
                    voice
                        .range
                        .clone()
                        .unwrap_or(kinewright_project::studio_records::AudioRange {
                            start_micros: 0,
                            end_micros: voice.duration_micros,
                        });
                if start_micros < allowed.start_micros
                    || end_micros > allowed.end_micros
                    || start_micros >= end_micros
                {
                    return Err(HostRefusal::new(
                        "private_range",
                        "The requested audio is outside this revision's selected range",
                    )
                    .into());
                }
                let audio = self.owner()?.store()?.read_evidence(&voice.audio_ref)?;
                reply.voice_audio = Some(
                    kinewright_media::voice_note::voice_range_wav(&audio, start_micros, end_micros)
                        .map_err(|e| HostError::Protocol(e.to_string()))?,
                );
            }
            _ => return Err(HostRefusal::new("voice_command", "Unsupported voice command").into()),
        }
        Ok(())
    }
    fn poll_voice(&mut self, note: &NoteId) -> Result<(), HostError> {
        let Some(mut job) = self.voice_jobs.remove(note) else {
            return Ok(());
        };
        let result = (|| -> Result<(), HostError> {
            if let Some(task) = &mut job.task {
                match task.try_complete() {
                    Ok(Some(done)) => {
                        job.task = None;
                        match done.result {
                            Ok(t) => {
                                job.progress = match &t { VoiceTranscript::AwaitingModel => "Whisper small is not installed. Audio is saved; install the model to transcribe.".into(), VoiceTranscript::NoSpeech => "No speech detected. Audio is saved.".into(), VoiceTranscript::Ready(_) => "Saving transcript…".into() };
                                job.completion = Some(t);
                            }
                            Err(e) => job.progress = e.to_string(),
                        }
                    }
                    Ok(None) => (),
                    Err(e) => {
                        job.task = None;
                        job.progress = e.to_string();
                    }
                }
            }
            self.owner_mut()?.retry_voice(&job.note)?;
            if job.completion.is_some() {
                self.owner_mut()?.accept_voice_asr(
                    &job.note,
                    &job.envelope,
                    &job.key,
                    &mut job.completion,
                )?;
            }
            if self
                .owner()?
                .voice_note(note)?
                .snapshot()?
                .voice
                .original_asr_ref
                .is_some()
            {
                job.progress = "Transcript ready".into();
            }
            Ok(())
        })();
        self.voice_jobs.insert(note.clone(), job);
        if matches!(&result, Err(HostError::Refused(e)) if e.code == "unsettled") {
            return Ok(());
        }
        result
    }
    fn voice_status(&self, note: &NoteId) -> Result<VoiceStatus, HostError> {
        let snapshot = self.owner()?.voice_note(note)?.snapshot()?;
        let voice = &snapshot.voice;
        let mut text = String::new();
        if let Some(reference) = &voice.chosen_transcript_revision {
            text = String::from_utf8(self.owner()?.store()?.read_evidence(reference)?.to_vec())
                .map_err(|e| HostError::Protocol(e.to_string()))?;
        } else if let Some(reference) = &voice.original_asr_ref {
            let asr: OriginalVoiceAsr<
                kinewright_project::bundle_records::ProjectId,
                NoteId,
                JobId,
                FeedbackAnchor,
            > = serde_json::from_slice(&self.owner()?.store()?.read_evidence(reference)?)?;
            if let VoiceTranscript::Ready(words) = asr.transcript {
                text = words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
        }
        Ok(VoiceStatus {
            progress: self.voice_jobs.get(note).map_or_else(
                || {
                    if voice.original_asr_ref.is_some() {
                        "Transcript ready"
                    } else {
                        "Audio saved"
                    }
                    .into()
                },
                |j| j.progress.clone(),
            ),
            text,
            voice: snapshot.voice,
            anchor: snapshot.anchor,
            complete: snapshot.complete,
            capture_error: snapshot.capture_error,
        })
    }
}

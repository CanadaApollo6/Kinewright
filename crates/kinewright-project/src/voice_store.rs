//! Owner-held voice jobs and pending publication, shared by every view of a note.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{ObjectRef, ProjectId, RecordKind},
    receipts::{ReceiptLookup, ReceiptResult},
    storage_budget::StorageReservation,
    store::StorePin,
    studio_records::{
        FeedbackAnchor, JobId, MetadataChange, MetadataEnvelope, MetadataResult, MetadataScope,
        MetadataUpdate, NoteId, VoiceRef,
    },
};
use kinewright_media::voice_note::{PlayheadLog, VoiceJob, VoiceJobKey, VoiceTranscript};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

pub type JobKey = VoiceJobKey<ProjectId, NoteId, JobId>;
pub(crate) type VoiceSlots = Mutex<BTreeMap<NoteId, Arc<Mutex<VoiceNoteSlot>>>>;

/// Raw metadata notes need no capture log. A capture view initializes its state lazily.
pub(crate) struct VoiceNoteSlot {
    note: NoteId,
    state: Option<VoiceNoteState>,
}
/// Unforgeable outside this module; borrowed only while the note slot is locked.
pub(crate) struct VoicePublication<'a> {
    note: &'a NoteId,
}
impl<'a> VoicePublication<'a> {
    pub(crate) fn note(self) -> &'a NoteId {
        self.note
    }
}

#[derive(Clone)]
pub struct PendingVoiceWrite {
    pub envelope: MetadataEnvelope,
    pub update: MetadataUpdate,
}
#[derive(Clone)]
enum NoteWrite {
    Exact {
        pending: Box<PendingVoiceWrite>,
        pin: Arc<StorePin>,
        expected_choice: Option<ObjectRef>,
    },
    Refused {
        voice: Box<VoiceRef>,
        pin: Arc<StorePin>,
        expected_choice: Option<ObjectRef>,
    },
}
pub(crate) struct VoiceNoteState {
    project: ProjectId,
    owner_epoch: u64,
    anchor: FeedbackAnchor,
    voice: VoiceRef,
    job: Option<VoiceJob<JobKey>>,
    expected_chosen: Option<ObjectRef>,
    pending_write: Option<NoteWrite>,
    pending_asr: Option<PendingAsr>,
    complete: bool,
    capture_error: Option<String>,
    _pin: StorePin,
    _charge: StorageReservation,
}
type OriginalAsr =
    kinewright_media::voice_note::OriginalVoiceAsr<ProjectId, NoteId, JobId, FeedbackAnchor>;
enum PendingAsr {
    Unvalidated(Box<OriginalAsr>),
    Completion(Box<OriginalAsr>),
    Sealed {
        reference: ObjectRef,
        _pin: StorePin,
    },
}
fn evidence(
    owner: &mut ProjectBundle,
    bytes: &[u8],
    dependencies: &[ObjectRef],
) -> Result<crate::store::SealedObject, BundleError> {
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    owner
        .store_mut()?
        .admit_blob(&mut &*bytes, &reference, dependencies)
}
pub fn save_voice(
    owner: &mut ProjectBundle,
    envelope: &MetadataEnvelope,
    voice: &VoiceRef,
    pending: &mut Option<PendingVoiceWrite>,
) -> Result<VoiceRef, BundleError> {
    save_voice_with(owner, envelope, voice, pending, None)
}
fn save_voice_with(
    owner: &mut ProjectBundle,
    envelope: &MetadataEnvelope,
    voice: &VoiceRef,
    pending: &mut Option<PendingVoiceWrite>,
    publication: Option<VoicePublication<'_>>,
) -> Result<VoiceRef, BundleError> {
    if let Some(original) = pending {
        if &original.envelope != envelope
            || original.update.change != MetadataChange::SaveVoice(voice.clone())
        {
            return Err(BundleError::Unsettled);
        }
    } else {
        *pending = Some(PendingVoiceWrite {
            envelope: envelope.clone(),
            update: MetadataUpdate {
                expected_root_generation: owner
                    .root()
                    .ok_or(BundleError::HistoryBarrierInvariant)?
                    .generation,
                change: MetadataChange::SaveVoice(voice.clone()),
            },
        });
    }
    let original = pending
        .as_ref()
        .ok_or(BundleError::HistoryBarrierInvariant)?;
    let lookup = match publication {
        Some(publication) => owner.voice_metadata_command(
            &original.envelope,
            MetadataScope::Feedback,
            &original.update,
            publication,
        ),
        None => owner.metadata_command(
            &original.envelope,
            MetadataScope::Feedback,
            &original.update,
        ),
    }?;
    let (ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt)) = lookup else {
        return Err(BundleError::Unsettled);
    };
    let index = owner.feedback_index()?;
    let retained = index.notes.get(&voice.note_id);
    if !matches!(receipt.result, ReceiptResult::Metadata { result: MetadataResult::Voice(ref id) } if id == &voice.note_id)
        || retained.is_none_or(|saved| {
            saved.audio_ref != voice.audio_ref
                || saved.playhead_log_ref != voice.playhead_log_ref
                || saved.duration_micros != voice.duration_micros
        })
    {
        return Err(BundleError::HistoryChanged);
    }
    *pending = None;
    retained
        .cloned()
        .ok_or(BundleError::HistoryBarrierInvariant)
}
impl VoiceNoteState {
    /// Settle the original command after an unknown reply, even if its job completed.
    pub(crate) fn retry_pending(&mut self, owner: &mut ProjectBundle) -> Result<bool, BundleError> {
        if owner.project_id() != &self.project || owner.store().owner_epoch() != self.owner_epoch {
            return Err(BundleError::HistoryChanged);
        }
        if self.pending_asr.is_some() {
            return self.publish_asr(owner, None);
        }
        let Some(pending) = self.pending_write.clone() else {
            return Ok(false);
        };
        let (envelope, voice) = match pending {
            NoteWrite::Exact { pending, .. } => {
                let MetadataChange::SaveVoice(voice) = pending.update.change else {
                    return Err(BundleError::HistoryChanged);
                };
                (pending.envelope, voice)
            }
            NoteWrite::Refused {
                voice: wanted,
                expected_choice,
                ..
            } => {
                let mut voice = owner
                    .feedback_index()?
                    .notes
                    .get(&self.voice.note_id)
                    .cloned()
                    .ok_or(BundleError::HistoryChanged)?;
                if voice.audio_ref != self.voice.audio_ref
                    || voice.playhead_log_ref != self.voice.playhead_log_ref
                    || voice.duration_micros != self.voice.duration_micros
                {
                    return Err(BundleError::HistoryChanged);
                }
                if voice.chosen_transcript_revision != expected_choice {
                    self.pending_write = None;
                    self.voice = voice;
                    return Err(BundleError::HistoryChanged);
                }
                voice.chosen_transcript_revision = wanted.chosen_transcript_revision;
                let envelope = fresh_envelope(owner)?;
                (envelope, voice)
            }
        };
        self.voice = self.write_voice(owner, &envelope, &voice)?;
        Ok(true)
    }
    pub(crate) fn reopen(owner: &ProjectBundle, note: &NoteId) -> Result<Self, BundleError> {
        let index = owner.feedback_index()?;
        let voice = index
            .notes
            .get(note)
            .cloned()
            .ok_or(BundleError::HistoryChanged)?;
        let log_ref = voice
            .playhead_log_ref
            .as_ref()
            .ok_or_else(|| BundleError::Unsupported("voice has no retained anchor log".into()))?;
        let bytes = owner.store().read_evidence(log_ref)?;
        let log: PlayheadLog<FeedbackAnchor> =
            serde_json::from_slice(&bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        log.validate(voice.duration_micros)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let anchor = log.samples[0].displayed.clone();
        let snapshot = owner.read_snapshot(&anchor.snapshot)?;
        let pin = owner.store().pin(&[
            voice.audio_ref.clone(),
            log_ref.clone(),
            snapshot.descriptor.checkpoint_ref.clone(),
        ])?;
        let slot_bytes = serde_json::to_vec(&(&voice, &anchor, &log.capture_status))
            .map_err(|error| BundleError::Corrupt(error.to_string()))?
            .len() as u64
            + std::mem::size_of::<Self>() as u64;
        let charge = owner.store().reserve(0, slot_bytes, 0, false)?;
        Ok(Self {
            project: owner.project_id().clone(),
            owner_epoch: owner.store().owner_epoch(),
            anchor,
            voice,
            job: None,
            expected_chosen: None,
            pending_write: None,
            pending_asr: None,
            complete: log
                .capture_status
                .as_ref()
                .is_some_and(|status| status.complete),
            capture_error: log.capture_status.as_ref().map_or_else(
                || Some("Capture completion status was not retained by this older note".into()),
                |status| status.capture_error.clone(),
            ),
            _pin: pin,
            _charge: charge,
        })
    }
    pub(crate) fn start_job(
        &mut self,
        owner: &ProjectBundle,
        transcript_revision: u64,
    ) -> Result<JobKey, BundleError> {
        if self.pending_write.is_some() || self.pending_asr.is_some() {
            return Err(BundleError::Unsettled);
        }
        if self.job.as_ref().is_some_and(VoiceJob::is_active) {
            return Err(BundleError::Unsettled);
        }
        if owner.project_id() != &self.project
            || owner.store().owner_epoch() != self.owner_epoch
            || owner.feedback_index()?.notes.get(&self.voice.note_id) != Some(&self.voice)
        {
            return Err(BundleError::HistoryChanged);
        }
        let key = JobKey {
            project: self.project.clone(),
            note: self.voice.note_id.clone(),
            job: JobId::fresh()?,
            audio_sha256: self.voice.audio_ref.sha256.clone(),
            transcript_revision,
        };
        self.job = Some(VoiceJob::new(key.clone()));
        self.expected_chosen = self.voice.chosen_transcript_revision.clone();
        Ok(key)
    }
    pub(crate) fn spawn_job(
        &self,
        owner: &ProjectBundle,
        data_dir: PathBuf,
        key: &JobKey,
    ) -> Result<kinewright_media::voice_note::VoiceTask<JobKey>, BundleError> {
        let Some(job) = &self.job else {
            return Err(BundleError::HistoryChanged);
        };
        if owner.project_id() != &self.project
            || owner.store().owner_epoch() != self.owner_epoch
            || job.key() != key
            || owner.feedback_index()?.notes.get(&self.voice.note_id) != Some(&self.voice)
        {
            return Err(BundleError::HistoryChanged);
        }
        let audio = owner.store().read_evidence(&self.voice.audio_ref)?;
        kinewright_media::voice_note::spawn_voice_transcription(
            key.clone(),
            audio,
            data_dir,
            job.cancellation(),
        )
        .map_err(|e| BundleError::Corrupt(e.to_string()))
    }
    pub(crate) fn cancel(&self) {
        if let Some(job) = &self.job {
            job.cancel();
        }
    }
    pub(crate) fn choose_transcript(
        &mut self,
        owner: &mut ProjectBundle,
        envelope: &MetadataEnvelope,
        reference: ObjectRef,
    ) -> Result<(), BundleError> {
        if self.pending_write.is_some() || self.pending_asr.is_some() {
            return Err(BundleError::Unsettled);
        }
        if owner.project_id() != &self.project
            || owner.store().owner_epoch() != self.owner_epoch
            || owner.feedback_index()?.notes.get(&self.voice.note_id) != Some(&self.voice)
        {
            return Err(BundleError::HistoryChanged);
        }
        let text = owner.store().read_evidence(&reference)?;
        if text.len() > kinewright_media::voice_note::MAX_VOICE_TEXT_BYTES {
            return Err(BundleError::Capacity("voice transcript text"));
        }
        std::str::from_utf8(&text).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        self.cancel();
        let mut voice = self.voice.clone();
        voice.chosen_transcript_revision = Some(reference);
        self.voice = self.write_voice(owner, envelope, &voice)?;
        Ok(())
    }
    pub(crate) fn accept_asr(
        &mut self,
        owner: &mut ProjectBundle,
        envelope: &MetadataEnvelope,
        key: &JobKey,
        current: &JobKey,
        completion: &mut Option<VoiceTranscript>,
    ) -> Result<bool, BundleError> {
        if self.pending_write.is_some() || self.pending_asr.is_some() {
            return Err(BundleError::Unsettled);
        }
        if owner.project_id() != &self.project || owner.store().owner_epoch() != self.owner_epoch {
            return Ok(false);
        }
        let transcript = completion.as_ref().ok_or(BundleError::HistoryChanged)?;
        transcript
            .validate(self.voice.duration_micros)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if !matches!(transcript, VoiceTranscript::Ready(_)) && !self.live_note_for_asr(owner)? {
            return Ok(false);
        }
        let log = self
            .voice
            .playhead_log_ref
            .as_ref()
            .ok_or(BundleError::HistoryChanged)?
            .clone();
        let Some(transcript) = self
            .job
            .as_mut()
            .and_then(|job| job.accept(key, current, completion.take()?))
        else {
            return Ok(false);
        };
        if let VoiceTranscript::Ready(_) = &transcript {
            let original = kinewright_media::voice_note::OriginalVoiceAsr {
                version: 1,
                job: key.clone(),
                anchor: self.anchor.clone(),
                playhead_log_sha256: log.sha256.clone(),
                model_sha256: kinewright_media::WHISPER_MODEL_SHA256.into(),
                transcript,
            };
            self.pending_asr = Some(PendingAsr::Unvalidated(Box::new(original)));
            return self.publish_asr(owner, Some(envelope));
        }
        Ok(true)
    }
    fn live_note_for_asr(&self, owner: &ProjectBundle) -> Result<bool, BundleError> {
        Ok(owner
            .feedback_index()?
            .notes
            .get(&self.voice.note_id)
            .is_some_and(|saved| {
                saved.audio_ref == self.voice.audio_ref
                    && saved.chosen_transcript_revision == self.expected_chosen
                    && saved.original_asr_ref.is_none()
            }))
    }
    fn publish_asr(
        &mut self,
        owner: &mut ProjectBundle,
        envelope: Option<&MetadataEnvelope>,
    ) -> Result<bool, BundleError> {
        if matches!(self.pending_asr, Some(PendingAsr::Unvalidated(_))) {
            if !self.live_note_for_asr(owner)? {
                self.pending_asr = None;
                return Ok(false);
            }
            let Some(PendingAsr::Unvalidated(original)) = self.pending_asr.take() else {
                return Err(BundleError::HistoryBarrierInvariant);
            };
            self.pending_asr = Some(PendingAsr::Completion(original));
        }
        if let Some(PendingAsr::Completion(original)) = &self.pending_asr {
            let log = self
                .voice
                .playhead_log_ref
                .as_ref()
                .ok_or(BundleError::HistoryChanged)?;
            let asr = evidence(
                owner,
                &serde_json::to_vec(original).map_err(|e| BundleError::Corrupt(e.to_string()))?,
                &[self.voice.audio_ref.clone(), log.clone()],
            )?;
            let reference = asr.reference.clone();
            self.pending_asr = Some(PendingAsr::Sealed {
                reference,
                _pin: asr.into_pin(),
            });
        }
        let Some(PendingAsr::Sealed { reference, .. }) = &self.pending_asr else {
            return Ok(false);
        };
        let reference = reference.clone();
        let (envelope, voice) = if let Some(NoteWrite::Exact { pending, .. }) = &self.pending_write
        {
            let MetadataChange::SaveVoice(voice) = &pending.update.change else {
                return Err(BundleError::HistoryChanged);
            };
            (pending.envelope.clone(), voice.clone())
        } else {
            let mut voice = owner
                .feedback_index()?
                .notes
                .get(&self.voice.note_id)
                .cloned()
                .ok_or(BundleError::HistoryChanged)?;
            if voice.audio_ref != self.voice.audio_ref
                || voice.playhead_log_ref != self.voice.playhead_log_ref
                || voice.duration_micros != self.voice.duration_micros
                || voice
                    .original_asr_ref
                    .as_ref()
                    .is_some_and(|saved| saved != &reference)
            {
                return Err(BundleError::HistoryChanged);
            }
            if voice.original_asr_ref.as_ref() == Some(&reference) {
                self.voice = voice;
                self.pending_write = None;
                self.pending_asr = None;
                return Ok(true);
            }
            voice.original_asr_ref = Some(reference);
            let envelope = match envelope {
                Some(envelope) => envelope.clone(),
                None => fresh_envelope(owner)?,
            };
            (envelope, voice)
        };
        let voice = self.write_voice(owner, &envelope, &voice)?;
        self.voice = voice;
        self.pending_asr = None;
        Ok(true)
    }
}

/// A view of one note's owner-held slot. Dropping views does not retire pending evidence.
pub struct VoiceNoteHandle {
    state: Weak<Mutex<VoiceNoteSlot>>,
}
pub struct VoiceNoteSnapshot {
    pub anchor: FeedbackAnchor,
    pub voice: VoiceRef,
    pub complete: bool,
    pub capture_error: Option<String>,
}
fn lock(state: &Mutex<VoiceNoteSlot>) -> Result<MutexGuard<'_, VoiceNoteSlot>, BundleError> {
    state.try_lock().map_err(|error| match error {
        std::sync::TryLockError::WouldBlock => BundleError::Unsettled,
        std::sync::TryLockError::Poisoned(_) => BundleError::HistoryBarrierInvariant,
    })
}
impl ProjectBundle {
    fn voice_slot(&self, note: &NoteId) -> Result<Arc<Mutex<VoiceNoteSlot>>, BundleError> {
        let mut slots = self
            .voice_slots
            .try_lock()
            .map_err(|_| BundleError::Unsettled)?;
        slots.retain(|_, slot| {
            Arc::strong_count(slot) != 1
                || Arc::weak_count(slot) != 0
                || slot.try_lock().map_or(true, |slot| {
                    slot.state.as_ref().is_some_and(|state| {
                        state.job.is_some()
                            || state.pending_write.is_some()
                            || state.pending_asr.is_some()
                    })
                })
        });
        if let Some(slot) = slots.get(note) {
            return Ok(Arc::clone(slot));
        }
        if slots.len() as u64 >= self.store.limits().comments {
            return Err(BundleError::Capacity("voice note slots"));
        }
        let slot = Arc::new(Mutex::new(VoiceNoteSlot {
            note: note.clone(),
            state: None,
        }));
        slots.insert(note.clone(), Arc::clone(&slot));
        Ok(slot)
    }
    pub fn voice_note(&self, note: &NoteId) -> Result<VoiceNoteHandle, BundleError> {
        let slot = self.voice_slot(note)?;
        let mut locked = lock(&slot)?;
        if let Some(live) = &mut locked.state {
            if live.pending_write.is_none() && live.pending_asr.is_none() {
                live.voice = self
                    .feedback_index()?
                    .notes
                    .get(note)
                    .cloned()
                    .ok_or(BundleError::HistoryChanged)?;
            }
        } else {
            locked.state = Some(VoiceNoteState::reopen(self, note)?);
        }
        Ok(VoiceNoteHandle {
            state: Arc::downgrade(&slot),
        })
    }
    /// Try every slot. A failed derived ASR may be discarded with this owner; audio/log remain rooted.
    pub fn settle_voice_notes(&mut self) -> Result<(), BundleError> {
        let slots: Vec<_> = self
            .voice_slots
            .try_lock()
            .map_err(|_| BundleError::Unsettled)?
            .values()
            .cloned()
            .collect();
        let mut first_error = None;
        for state in slots {
            let result = lock(&state).and_then(|mut slot| {
                slot.state
                    .as_mut()
                    .map_or(Ok(false), |state| state.retry_pending(self))
            });
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}
/// Public metadata (also `WriterHost`) and capture acknowledgment enter the same slot as views.
pub(crate) fn metadata_command(
    owner: &mut ProjectBundle,
    envelope: &MetadataEnvelope,
    scope: MetadataScope,
    update: &MetadataUpdate,
) -> Result<ReceiptLookup, BundleError> {
    let MetadataChange::SaveVoice(voice) = &update.change else {
        return Err(BundleError::HistoryBarrierInvariant);
    };
    let slot = owner.voice_slot(&voice.note_id)?;
    let mut slot = lock(&slot)?;
    if slot.state.as_ref().is_some_and(|state| {
        state.pending_asr.is_some() || matches!(state.pending_write, Some(NoteWrite::Exact { .. }))
    }) {
        return Err(BundleError::Unsettled);
    }
    let result = owner.voice_metadata_command(
        envelope,
        scope,
        update,
        VoicePublication { note: &slot.note },
    );
    if result.is_ok()
        && let Some(state) = &mut slot.state
    {
        // A refused choice keeps its old expectation until retry detects the conflict.
        if state.pending_write.is_none() {
            let current = owner
                .feedback_index()?
                .notes
                .get(&voice.note_id)
                .cloned()
                .ok_or(BundleError::HistoryChanged)?;
            if state.voice != current {
                state.cancel();
                state.voice = current;
            }
        }
    }
    result
}
impl Drop for ProjectBundle {
    fn drop(&mut self) {
        let _ = self.settle_voice_notes();
    }
}
impl VoiceNoteHandle {
    fn with_state<T>(
        &self,
        action: impl FnOnce(&mut VoiceNoteState) -> Result<T, BundleError>,
    ) -> Result<T, BundleError> {
        let state = self.state.upgrade().ok_or(BundleError::HistoryChanged)?;
        let mut slot = lock(&state)?;
        action(slot.state.as_mut().ok_or(BundleError::HistoryChanged)?)
    }
    pub fn snapshot(&self) -> Result<VoiceNoteSnapshot, BundleError> {
        self.with_state(|state| {
            Ok(VoiceNoteSnapshot {
                anchor: state.anchor.clone(),
                voice: state.voice.clone(),
                complete: state.complete,
                capture_error: state.capture_error.clone(),
            })
        })
    }
    pub fn pending_write(&self) -> Result<Option<PendingVoiceWrite>, BundleError> {
        self.with_state(|state| {
            Ok(match &state.pending_write {
                Some(NoteWrite::Exact { pending, .. }) => Some((**pending).clone()),
                _ => None,
            })
        })
    }
    pub fn retry_pending(&self, owner: &mut ProjectBundle) -> Result<bool, BundleError> {
        self.with_state(|state| state.retry_pending(owner))
    }
    pub fn start_job(
        &self,
        owner: &ProjectBundle,
        expected: &VoiceRef,
        revision: u64,
    ) -> Result<JobKey, BundleError> {
        self.with_state(|state| {
            state.ready_for_view(expected)?;
            state.start_job(owner, revision)
        })
    }
    pub fn spawn_job(
        &self,
        owner: &ProjectBundle,
        data_dir: PathBuf,
        key: &JobKey,
    ) -> Result<kinewright_media::voice_note::VoiceTask<JobKey>, BundleError> {
        self.with_state(|state| state.spawn_job(owner, data_dir, key))
    }
    pub fn cancel(&self) {
        let _ = self.with_state(|state| {
            state.cancel();
            Ok(())
        });
    }
    pub fn choose_transcript(
        &self,
        owner: &mut ProjectBundle,
        envelope: &MetadataEnvelope,
        expected: &VoiceRef,
        reference: ObjectRef,
    ) -> Result<(), BundleError> {
        self.with_state(|state| {
            state.ready_for_view(expected)?;
            state.choose_transcript(owner, envelope, reference)
        })
    }
    /// The caller retains `completion` on contention; it is taken only inside the locked slot.
    pub fn accept_asr(
        &self,
        owner: &mut ProjectBundle,
        envelope: &MetadataEnvelope,
        key: &JobKey,
        current: &JobKey,
        completion: &mut Option<VoiceTranscript>,
    ) -> Result<bool, BundleError> {
        self.with_state(|state| state.accept_asr(owner, envelope, key, current, completion))
    }
    #[cfg(feature = "test-util")]
    pub fn hold_slot_probe(
        &self,
        entered: &std::sync::Barrier,
        release: &std::sync::Barrier,
    ) -> Result<(), BundleError> {
        self.with_state(|_| {
            entered.wait();
            release.wait();
            Ok(())
        })
    }
    #[cfg(feature = "test-util")]
    pub fn retry_asr_with_envelope_probe(
        &self,
        owner: &mut ProjectBundle,
        envelope: &MetadataEnvelope,
    ) -> Result<bool, BundleError> {
        self.with_state(|state| state.publish_asr(owner, Some(envelope)))
    }
    #[cfg(feature = "test-util")]
    pub fn retain_reply_probe(
        &self,
        owner: &ProjectBundle,
        pending: PendingVoiceWrite,
        retain_asr: bool,
    ) -> Result<(), BundleError> {
        self.with_state(|state| {
            let MetadataChange::SaveVoice(voice) = &pending.update.change else {
                return Err(BundleError::HistoryChanged);
            };
            if retain_asr {
                let reference = voice
                    .original_asr_ref
                    .clone()
                    .ok_or(BundleError::HistoryChanged)?;
                state.pending_asr = Some(PendingAsr::Sealed {
                    reference: reference.clone(),
                    _pin: owner.store().pin(&[reference])?,
                });
            }
            state.pending_write = Some(NoteWrite::Exact {
                pin: pin_chosen_text(owner, voice)?,
                expected_choice: state.voice.chosen_transcript_revision.clone(),
                pending: Box::new(pending),
            });
            Ok(())
        })
    }
}
impl VoiceNoteState {
    fn write_voice(
        &mut self,
        owner: &mut ProjectBundle,
        envelope: &MetadataEnvelope,
        voice: &VoiceRef,
    ) -> Result<VoiceRef, BundleError> {
        let (mut pending, pin, expected_choice) = match &self.pending_write {
            Some(NoteWrite::Exact {
                pending,
                pin,
                expected_choice,
            }) => (
                Some((**pending).clone()),
                Arc::clone(pin),
                expected_choice.clone(),
            ),
            Some(NoteWrite::Refused {
                pin,
                expected_choice,
                ..
            }) => (None, Arc::clone(pin), expected_choice.clone()),
            None => (
                None,
                pin_chosen_text(owner, voice)?,
                self.voice.chosen_transcript_revision.clone(),
            ),
        };
        // Already inside this slot: the capability avoids re-locking through the public router.
        let result = save_voice_with(
            owner,
            envelope,
            voice,
            &mut pending,
            Some(VoicePublication {
                note: &self.voice.note_id,
            }),
        );
        self.pending_write = pending.map(|pending| NoteWrite::Exact {
            pending: Box::new(pending),
            pin: Arc::clone(&pin),
            expected_choice: expected_choice.clone(),
        });
        if result
            .as_ref()
            .is_err_and(|error| definitely_refused(owner, error))
        {
            self.pending_write = Some(NoteWrite::Refused {
                voice: Box::new(voice.clone()),
                pin,
                expected_choice,
            });
        }
        result
    }
    fn ready_for_view(&self, expected: &VoiceRef) -> Result<(), BundleError> {
        if self.pending_write.is_some() || self.pending_asr.is_some() {
            return Err(BundleError::Unsettled);
        }
        if &self.voice != expected {
            return Err(BundleError::HistoryChanged);
        }
        Ok(())
    }
}

fn definitely_refused(owner: &ProjectBundle, error: &BundleError) -> bool {
    !matches!(
        error,
        BundleError::DurabilityUnknown
            | BundleError::DurabilityUnknownAt(_)
            | BundleError::Unsettled
    ) && owner.publication_state() != crate::bundle::PublicationState::Unknown
}
fn fresh_envelope(owner: &mut ProjectBundle) -> Result<MetadataEnvelope, BundleError> {
    Ok(MetadataEnvelope {
        id: crate::receipts::CommandId {
            issuer: owner.issue_metadata_issuer()?,
            sequence: 1,
        },
        project_id: owner.project_id().clone(),
        target_branch: owner.manual_branch_id().clone(),
        owner_epoch: owner.store().owner_epoch(),
        expected_head: owner.manual_head().cloned(),
    })
}

fn pin_chosen_text(owner: &ProjectBundle, voice: &VoiceRef) -> Result<Arc<StorePin>, BundleError> {
    // Audio/log are acknowledged roots; an unpublished ASR has its own completion pin.
    let roots: Vec<_> = voice.chosen_transcript_revision.iter().cloned().collect();
    Ok(Arc::new(owner.store().pin(&roots)?))
}

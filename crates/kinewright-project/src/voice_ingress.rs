//! Bounded capture admission on the existing voice publisher.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, RecordKind},
    editing_session::Ja1EditingSession,
    storage_budget::StorageReservation,
    studio_records::{FeedbackAnchor, MetadataEnvelope, NoteId, VoiceRef},
    voice_store::{JobKey, PendingVoiceWrite, VoiceNoteHandle},
};
use kinewright_media::voice_note::{PlayheadLog, VoiceTask, VoiceTranscript};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::PathBuf};

pub const AUDIO_BYTES: u64 = kinewright_media::voice_note::MAX_VOICE_AUDIO_BYTES;
pub const LOG_BYTES: u64 = 8 << 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceUpload {
    pub envelope: MetadataEnvelope,
    pub note: NoteId,
    pub root_generation: u64,
    pub duration_micros: u64,
    pub audio: ObjectRef,
    pub log: ObjectRef,
}
impl VoiceUpload {
    pub fn validate(&self) -> Result<(), BundleError> {
        for (reference, limit) in [(&self.audio, AUDIO_BYTES), (&self.log, LOG_BYTES)] {
            reference.validate()?;
            if reference.record_kind != RecordKind::OwnedEvidence || reference.byte_length > limit {
                return Err(BundleError::Capacity("voice stream bytes"));
            }
        }
        Ok(())
    }
    #[must_use]
    pub fn voice(&self) -> VoiceRef {
        VoiceRef {
            note_id: self.note.clone(),
            audio_ref: self.audio.clone(),
            duration_micros: self.duration_micros,
            range: None,
            playhead_log_ref: Some(self.log.clone()),
            original_asr_ref: None,
            chosen_transcript_revision: None,
        }
    }
    #[must_use]
    pub fn update(&self) -> crate::studio_records::MetadataUpdate {
        crate::studio_records::MetadataUpdate {
            expected_root_generation: self.root_generation,
            change: crate::studio_records::MetadataChange::SaveVoice(self.voice()),
        }
    }
}

impl Ja1EditingSession {
    pub fn voice_receipt(
        &self,
        upload: &VoiceUpload,
    ) -> Result<crate::receipts::ReceiptLookup, BundleError> {
        upload.validate()?;
        let owner = self.active_bundle()?;
        if owner.project_id() != &upload.envelope.project_id {
            return Err(BundleError::HistoryChanged);
        }
        owner.receipt_lookup(&upload.envelope.receipt_envelope(&upload.update())?)
    }
    pub fn reserve_voice_capture(&self) -> Result<StorageReservation, BundleError> {
        self.active_bundle()?.store().reserve(
            AUDIO_BYTES + LOG_BYTES,
            3 * (AUDIO_BYTES + LOG_BYTES),
            0,
            true,
        )
    }
    pub fn voice_note(&self, note: &NoteId) -> Result<VoiceNoteHandle, BundleError> {
        self.active_bundle()?.voice_note(note)
    }
    pub fn start_voice_job(
        &self,
        note: &VoiceNoteHandle,
        voice: &VoiceRef,
        revision: u64,
    ) -> Result<JobKey, BundleError> {
        note.start_job(self.active_bundle()?, voice, revision)
    }
    pub fn spawn_voice_job(
        &self,
        note: &VoiceNoteHandle,
        directory: PathBuf,
        key: &JobKey,
    ) -> Result<VoiceTask<JobKey>, BundleError> {
        note.spawn_job(self.active_bundle()?, directory, key)
    }
    pub fn retry_voice(&mut self, note: &VoiceNoteHandle) -> Result<bool, BundleError> {
        note.retry_pending(self.active_bundle_mut()?)
    }
    pub fn accept_voice_asr(
        &mut self,
        note: &VoiceNoteHandle,
        envelope: &MetadataEnvelope,
        key: &JobKey,
        completion: &mut Option<VoiceTranscript>,
    ) -> Result<bool, BundleError> {
        note.accept_asr(self.active_bundle_mut()?, envelope, key, key, completion)
    }
    pub fn choose_voice_transcript(
        &mut self,
        envelope: &MetadataEnvelope,
        note: &NoteId,
        text: ObjectRef,
    ) -> Result<(), BundleError> {
        let owner = self.active_bundle_mut()?;
        if let Some(receipt) = owner
            .receipts
            .detailed
            .iter()
            .chain(&owner.receipts.retired)
            .find(|r| r.id == envelope.id)
        {
            let mut voice = owner
                .feedback_index()?
                .notes
                .get(note)
                .cloned()
                .ok_or(BundleError::HistoryChanged)?;
            voice.chosen_transcript_revision = Some(text.clone());
            let update = crate::studio_records::MetadataUpdate {
                expected_root_generation: receipt.generation - 1,
                change: crate::studio_records::MetadataChange::SaveVoice(voice),
            };
            return match owner.receipt_lookup(&envelope.receipt_envelope(&update)?)? {
                crate::receipts::ReceiptLookup::Committed(_)
                | crate::receipts::ReceiptLookup::Retired(_) => Ok(()),
                _ => Err(BundleError::HistoryChanged),
            };
        }
        // A committed old-epoch choice was settled above. An absent one must
        // never enter the new owner's per-note publication slot.
        if envelope.owner_epoch != owner.store().owner_epoch() {
            return Err(BundleError::HistoryChanged);
        }
        let handle = owner.voice_note(note)?;
        handle.retry_pending(owner)?;
        let voice = handle.snapshot()?.voice;
        handle.choose_transcript(owner, envelope, &voice, text)
    }
    /// The reservation and both complete, validated streams precede any object admission.
    pub fn acknowledge_voice(
        &mut self,
        upload: &VoiceUpload,
        mut stream: impl Read,
        reservation: StorageReservation,
    ) -> Result<VoiceRef, BundleError> {
        upload.validate()?;
        let _reservation = reservation;
        let length = upload.audio.byte_length + upload.log.byte_length;
        let mut bytes = Vec::new();
        stream.by_ref().take(length + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != length {
            return Err(BundleError::Corrupt(
                "truncated or oversized voice stream".into(),
            ));
        }
        let (wav, log_bytes) = bytes.split_at(
            usize::try_from(upload.audio.byte_length)
                .map_err(|_| BundleError::Capacity("voice length"))?,
        );
        if kinewright_media::sha256_bytes(wav) != upload.audio.sha256
            || kinewright_media::sha256_bytes(log_bytes) != upload.log.sha256
        {
            return Err(BundleError::Corrupt("voice stream digest".into()));
        }
        let duration = kinewright_media::voice_note::voice_audio_duration(wav)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let log: PlayheadLog<FeedbackAnchor> =
            serde_json::from_slice(log_bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        log.validate(duration)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let owner = self.active_bundle_mut()?;
        if owner.project_id() != &upload.envelope.project_id {
            return Err(BundleError::HistoryChanged);
        }
        let mut dependencies = Vec::new();
        let mut pins = Vec::new();
        for sample in &log.samples {
            let anchor = &sample.displayed;
            let snapshot = owner.read_snapshot(&anchor.snapshot)?;
            let document = owner
                .store()
                .read_record::<kinewright_core::Document>(&snapshot.descriptor.document_ref)?;
            if anchor.frame < 0
                || anchor.frame >= document.duration.0
                || anchor.fps.numerator != document.fps.numerator()
                || anchor.fps.denominator != document.fps.denominator()
            {
                return Err(BundleError::HistoryChanged);
            }
            if !dependencies.contains(&snapshot.descriptor.checkpoint_ref) {
                dependencies.push(snapshot.descriptor.checkpoint_ref.clone());
            }
            pins.push(snapshot);
        }
        if duration != upload.duration_micros {
            return Err(BundleError::Corrupt("voice duration binding".into()));
        }
        let voice = upload.voice();
        let audio = owner
            .store_mut()?
            .admit_blob(&mut &*wav, &upload.audio, &[])?;
        let log = owner
            .store_mut()?
            .admit_blob(&mut &*log_bytes, &upload.log, &dependencies)?;
        let mut pending = Some(PendingVoiceWrite {
            envelope: upload.envelope.clone(),
            update: upload.update(),
        });
        let saved = crate::voice_store::save_voice(owner, &upload.envelope, &voice, &mut pending)?;
        drop((audio, log, pins));
        Ok(saved)
    }
}

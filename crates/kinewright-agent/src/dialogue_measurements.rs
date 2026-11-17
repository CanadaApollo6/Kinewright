//! The dialogue tools' cache adapter. Measurement stays on the media worker.
use crate::dialogue_tools::{BoundDialogue, DialogueMeasurements, MediaIdentity, Refusal};
use kinewright_core::{
    Command, Core, Event, MediaAsset, Query, QueryResult, TranscriptStatus,
    dialogue::{Settings, Span, Word},
};
use kinewright_media::{SessionMedia, dialogue_cache::DialogueCacheRead};
use serde::Serialize;
use std::{
    collections::VecDeque,
    mem::size_of,
    sync::{Arc, Mutex, OnceLock},
};

const RESIDENT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MeasurementStatus {
    Queued,
    Running { progress_percent: Option<u8> },
    Failed { reason: String },
}
pub(crate) fn failed(reason: impl AsRef<str>) -> Refusal {
    Refusal::MeasurementPending {
        status: MeasurementStatus::Failed {
            reason: reason.as_ref().chars().take(128).collect(),
        },
    }
}

pub(crate) struct UnmeasuredDialogue;
impl DialogueMeasurements for UnmeasuredDialogue {
    fn audio_stream(&self, _: &MediaIdentity) -> Result<u32, Refusal> {
        Err(Refusal::NotMeasured)
    }
    fn get(&self, _: &MediaIdentity, _: u32) -> Result<Arc<BoundDialogue>, Refusal> {
        Err(Refusal::NotMeasured)
    }
}
fn pending(status: TranscriptStatus) -> Refusal {
    let status = match status {
        TranscriptStatus::Queued | TranscriptStatus::NotRequested => MeasurementStatus::Queued,
        TranscriptStatus::Transcribing { progress_percent } => MeasurementStatus::Running {
            progress_percent: Some(progress_percent),
        },
        TranscriptStatus::Hashing => MeasurementStatus::Running {
            progress_percent: None,
        },
        TranscriptStatus::DownloadingModel {
            downloaded_bytes,
            total_bytes,
            ..
        } => MeasurementStatus::Running {
            progress_percent: total_bytes.filter(|total| *total > 0).map(|total| {
                u8::try_from((u128::from(downloaded_bytes) * 100 / u128::from(total)).min(100))
                    .expect("percentage")
            }),
        },
        TranscriptStatus::Failed(reason) => return failed(reason),
        TranscriptStatus::Cancelled => return failed("measurement cancelled"),
        TranscriptStatus::Ready(_) | TranscriptStatus::NoSpeech => {
            return failed("cached words are unavailable; request transcription again");
        }
    };
    Refusal::MeasurementPending { status }
}

struct Resident {
    entries: VecDeque<(Arc<BoundDialogue>, usize)>,
    bytes: usize,
    limit: usize,
}
impl Default for Resident {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            limit: RESIDENT_BYTES,
        }
    }
}
fn shared_resident() -> Arc<Mutex<Resident>> {
    static RESIDENT: OnceLock<Arc<Mutex<Resident>>> = OnceLock::new();
    Arc::clone(RESIDENT.get_or_init(|| Arc::new(Mutex::new(Resident::default()))))
}
impl Resident {
    fn insert(
        &mut self,
        value: BoundDialogue,
        bytes: usize,
    ) -> Result<Arc<BoundDialogue>, Refusal> {
        if bytes > self.limit {
            return Err(failed("measurement exceeds resident capacity"));
        }
        while self.bytes > self.limit - bytes {
            let Some(index) = self
                .entries
                .iter()
                .position(|(value, _)| Arc::strong_count(value) == 1)
            else {
                return Err(failed(
                    "measurement cache is busy; retry after running tools finish",
                ));
            };
            let (_, cost) = self.entries.remove(index).expect("resident index");
            self.bytes -= cost;
        }
        let value = Arc::new(value);
        self.entries.push_back((Arc::clone(&value), bytes));
        self.bytes += bytes;
        Ok(value)
    }
}

/// One immutable bound value per actual cache result, shared by running tools.
pub struct CachedDialogueMeasurements {
    core: Core,
    media: Arc<SessionMedia>,
    resident: Arc<Mutex<Resident>>,
}
impl CachedDialogueMeasurements {
    #[cfg(test)]
    pub(crate) fn test_resident_limit(&self, limit: Option<usize>) -> usize {
        let mut resident = self.resident.lock().unwrap();
        if let Some(limit) = limit {
            resident.limit = limit;
        }
        resident.bytes
    }
    #[must_use]
    pub fn new(core: Core, media: Arc<SessionMedia>) -> Self {
        Self {
            core,
            media,
            resident: shared_resident(),
        }
    }
    #[cfg(test)]
    pub(crate) fn test_new(core: Core, media: Arc<SessionMedia>) -> Self {
        Self {
            core,
            media,
            resident: Arc::new(Mutex::new(Resident::default())),
        }
    }
    fn asset(&self, identity: &MediaIdentity) -> Result<MediaAsset, Refusal> {
        let Event::QueryResult(QueryResult::Document(document)) = self
            .core
            .request(Command::Query(Query::Document))
            .map_err(|e| failed(e.to_string()))?
        else {
            return Err(failed("document unavailable"));
        };
        document
            .media_pool
            .iter()
            .find(|asset| MediaIdentity::of(asset).as_ref() == Ok(identity))
            .cloned()
            .ok_or(Refusal::Identity)
    }
}
fn budget(value: &BoundDialogue, word_capacity: usize, key_capacity: usize) -> usize {
    let dialogue = value.dialogue();
    let thresholds = dialogue.thresholds();
    let (mut dips, mut in_dip) = (0_usize, false);
    let (mut sound_components, mut in_sound) = (0_usize, false);
    for hop in dialogue.profile().hops() {
        let dip = thresholds
            .dip_dbfs_hundredths
            .is_some_and(|limit| hop.rms_dbfs_hundredths <= limit);
        if dip && !in_dip {
            dips += 1;
        }
        in_dip = dip;
        let sound = hop.rms_dbfs_hundredths > thresholds.quiet_rms_max;
        if sound && !in_sound {
            sound_components += 1;
        }
        in_sound = sound;
    }
    // Charge payloads and conservative capacities for Core's five run vectors.
    // Gaps retain at most the quiet and valley runs before deduplication.
    let quiet = dialogue.quiet_runs().len();
    let valleys = dialogue.valleys().len();
    let runs = [quiet, valleys, quiet + valleys, dips, sound_components]
        .into_iter()
        .map(|count| (count * 2).max(4))
        .sum::<usize>();
    size_of::<BoundDialogue>()
        + 2 * size_of::<usize>()
        + 64
        + 64
        + key_capacity
        + dialogue.profile().len() * 5
        + runs * size_of::<Span>()
        + word_capacity * size_of::<Word>()
        + dialogue
            .words()
            .iter()
            .map(|w| w.text.capacity())
            .sum::<usize>()
}
impl DialogueMeasurements for CachedDialogueMeasurements {
    fn audio_stream(&self, identity: &MediaIdentity) -> Result<u32, Refusal> {
        self.media
            .dialogue_audio_stream(&self.asset(identity)?)
            .map_err(|e| failed(e.to_string()))
    }
    fn get(&self, identity: &MediaIdentity, stream: u32) -> Result<Arc<BoundDialogue>, Refusal> {
        let asset = self.asset(identity)?;
        if self
            .media
            .dialogue_audio_stream(&asset)
            .map_err(|e| failed(e.to_string()))?
            != stream
        {
            return Err(Refusal::Stream);
        }
        let read = self
            .media
            .dialogue_cache(&asset, stream)
            .map_err(|e| failed(e.to_string()))?;
        let DialogueCacheRead::Ready {
            recogniser_key,
            profile,
            words,
        } = read
        else {
            let DialogueCacheRead::Pending(status) = read else {
                unreachable!()
            };
            return Err(pending(status));
        };
        let mut resident = self
            .resident
            .lock()
            .map_err(|_| failed("measurement cache lock failed"))?;
        if let Some(index) = resident.entries.iter().position(|(v, _)| {
            v.identity() == identity
                && v.stream() == stream
                && v.recogniser_key() == recogniser_key
                && v.dialogue().profile() == &profile
                && v.dialogue().words() == words
        }) {
            let entry = resident.entries.remove(index).expect("resident index");
            let result = Arc::clone(&entry.0);
            resident.entries.push_back(entry);
            return Ok(result);
        }
        let word_capacity = words.capacity();
        let key_capacity = recogniser_key.capacity();
        let value = BoundDialogue::new(
            MediaIdentity {
                sha256: identity.sha256.clone(),
                byte_length: identity.byte_length,
            },
            stream,
            recogniser_key,
            profile,
            words,
            Settings::default(),
        )?;
        let bytes = budget(&value, word_capacity, key_capacity);
        resident.insert(value, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dq_media_resident_budget_counts_profile_and_word_storage() {
        let text_bytes = 1_048_576;
        let hops = 60_000;
        let words = vec![Word {
            text: "x".repeat(text_bytes),
            start_micros: 100_000,
            end_micros: 200_000,
        }];
        let word_capacity = words.capacity();
        let key = "fixture".to_owned();
        let key_capacity = key.capacity();
        let profile = kinewright_core::dialogue::SoundProfile::from_hops(
            10_000,
            600_000_000,
            (0..hops).map(|i| kinewright_core::dialogue::Hop {
                rms_dbfs_hundredths: if i == 0 { -2000 } else { -6000 },
                peak_dbfs_hundredths: if i == 0 { -2000 } else { -6000 },
                voice: i == 0,
            }),
        )
        .unwrap();
        let value = BoundDialogue::new(
            MediaIdentity {
                sha256: "a".repeat(64),
                byte_length: 10,
            },
            0,
            key,
            profile,
            words,
            Settings::default(),
        )
        .unwrap();
        let bytes = budget(&value, word_capacity, key_capacity);
        assert!(
            bytes >= text_bytes + hops * 5,
            "DQ_MEDIA_RESIDENT_COUNTS_PROFILE_AND_WORDS"
        );
        let mut resident = Resident {
            limit: text_bytes / 2,
            ..Resident::default()
        };
        assert!(
            matches!(
                resident.insert(value, bytes),
                Err(Refusal::MeasurementPending { .. })
            ),
            "DQ_MEDIA_OVERSIZE_VALUE_REFUSED"
        );
        assert_eq!(resident.bytes, 0);
    }
    #[test]
    fn dq_media_measurement_status_is_typed_and_failure_is_short() {
        for (input, expected) in [
            (
                TranscriptStatus::Queued,
                serde_json::json!({"state":"queued"}),
            ),
            (
                TranscriptStatus::Hashing,
                serde_json::json!({"state":"running","progress_percent":null}),
            ),
            (
                TranscriptStatus::Transcribing {
                    progress_percent: 42,
                },
                serde_json::json!({"state":"running","progress_percent":42}),
            ),
            (
                TranscriptStatus::DownloadingModel {
                    model_name: "pinned".into(),
                    downloaded_bytes: 1,
                    total_bytes: Some(4),
                },
                serde_json::json!({"state":"running","progress_percent":25}),
            ),
        ] {
            assert_eq!(
                serde_json::to_value(pending(input)).unwrap()["status"],
                expected
            );
        }
        let Refusal::MeasurementPending {
            status: MeasurementStatus::Failed { reason },
        } = failed("é".repeat(200))
        else {
            panic!("typed failure")
        };
        assert_eq!(
            reason.chars().count(),
            128,
            "DQ_MEDIA_FAILURE_REASON_128_CHARACTERS"
        );
    }
}

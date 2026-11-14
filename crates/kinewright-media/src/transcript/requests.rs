//! Recognition admission and publication share one authority/path state lock.
//! A replaced request can never publish over its successor or an authored sidecar.
use super::{
    Arc, AssetTranscript, CancellationRegistry, ExportCancellation, Identity, Path, StatusReporter,
    TranscriptStatus,
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Recipe {
    pub identity: Identity,
    pub language: Option<String>,
}
enum Entry {
    Registered(TranscriptStatus),
    Requested {
        recipe: Recipe,
        cancellation: ExportCancellation,
        status: TranscriptStatus,
    },
}
impl Entry {
    fn status(&self) -> &TranscriptStatus {
        match self {
            Self::Registered(status) | Self::Requested { status, .. } => status,
        }
    }
}
#[derive(Clone)]
pub(super) struct Requests {
    states: StatusReporter<Entry>,
    cancellations: CancellationRegistry,
}
impl Requests {
    pub fn new() -> Self {
        Self {
            states: StatusReporter::new(),
            cancellations: CancellationRegistry::default(),
        }
    }
    pub fn admit(&self, path: &Path, recipe: &Recipe) -> Option<ExportCancellation> {
        self.states
            .change(path, |previous| {
                let blocks = match &previous {
                    Some(Entry::Registered(_)) => true,
                    Some(Entry::Requested {
                        recipe: current,
                        status,
                        ..
                    }) if current == recipe => matches!(
                        status,
                        TranscriptStatus::Queued
                            | TranscriptStatus::Hashing
                            | TranscriptStatus::DownloadingModel { .. }
                            | TranscriptStatus::Transcribing { .. }
                            | TranscriptStatus::Ready(_)
                            | TranscriptStatus::NoSpeech
                    ),
                    _ => false,
                };
                if blocks {
                    return (previous, None);
                }
                let Some(cancellation) = self.cancellations.replace(path) else {
                    return (previous, None);
                };
                // The worker and the status table each retain the immutable request
                // recipe after the admitting call returns.
                (
                    Some(Entry::Requested {
                        recipe: recipe.clone(),
                        cancellation: cancellation.clone(),
                        status: TranscriptStatus::Queued,
                    }),
                    Some(cancellation),
                )
            })
            .flatten()
    }
    pub fn update(&self, path: &Path, cancellation: &ExportCancellation, status: TranscriptStatus) {
        self.states.change(path, |previous| match previous {
            Some(Entry::Requested {
                recipe,
                cancellation: current,
                ..
            }) if &current == cancellation => {
                // Cancellation suppresses late success/progress, while an explicit
                // failure still explains a refused job to the person.
                let status = if current.is_cancelled()
                    && !matches!(
                        &status,
                        TranscriptStatus::Failed(_) | TranscriptStatus::Cancelled
                    ) {
                    TranscriptStatus::Cancelled
                } else {
                    status
                };
                (
                    Some(Entry::Requested {
                        recipe,
                        cancellation: current,
                        status,
                    }),
                    (),
                )
            }
            previous => (previous, ()),
        });
    }
    pub fn status(&self, path: &Path) -> TranscriptStatus {
        self.states
            .read(path, |entry| {
                entry.map_or(TranscriptStatus::NotRequested, |entry| {
                    entry.status().clone()
                })
            })
            .unwrap_or(TranscriptStatus::NotRequested)
    }
    pub fn register(&self, path: &Path, transcript: AssetTranscript) {
        self.register_status(path, TranscriptStatus::Ready(Arc::new(transcript)));
    }
    pub fn no_audio(&self, path: &Path) {
        self.register_status(path, TranscriptStatus::NoSpeech);
    }
    fn register_status(&self, path: &Path, status: TranscriptStatus) {
        self.states.change(path, |_| {
            self.cancellations.cancel(path);
            (Some(Entry::Registered(status)), ())
        });
    }
    pub fn cancel(&self, path: &Path) -> bool {
        self.states
            .change(path, |previous| match previous {
                Some(Entry::Requested {
                    recipe,
                    cancellation,
                    status,
                }) => {
                    let cancelled = self.cancellations.cancel(path);
                    let status = if cancelled {
                        TranscriptStatus::Cancelled
                    } else {
                        status
                    };
                    (
                        Some(Entry::Requested {
                            recipe,
                            cancellation,
                            status,
                        }),
                        cancelled,
                    )
                }
                previous => (previous, false),
            })
            .unwrap_or(false)
    }
    pub fn finish(&self, path: &Path, cancellation: &ExportCancellation) {
        self.cancellations.finish(path, cancellation);
    }
    pub fn cancel_all(&self) {
        self.cancellations.cancel_all();
    }
    pub fn shutdown(&self) {
        self.cancellations.shutdown();
    }
    pub fn retire(&self, authority: kinewright_core::LegacyMediaIdentity) {
        self.cancellations.retire(authority);
        self.states.retire(authority);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizer;
    use kinewright_core::{AssetId, Rational};
    fn recipe(language: Option<&str>) -> Recipe {
        Recipe {
            identity: Identity::for_model(recognizer::model_for_language(language)),
            language: language.map(str::to_owned),
        }
    }
    fn ready() -> TranscriptStatus {
        TranscriptStatus::Ready(Arc::new(AssetTranscript {
            asset: AssetId(1),
            content_sha256: "0".repeat(64),
            source_fps: Rational::new(25, 1).unwrap(),
            words: Vec::new(),
        }))
    }
    #[test]
    fn dq1_new_language_replaces_a_ready_model_and_ignores_the_old_job() {
        let requests = Requests::new();
        let path = Path::new("fixture.wav");
        let english = recipe(None);
        let old = requests.admit(path, &english).unwrap();
        requests.update(path, &old, ready());
        assert!(requests.admit(path, &english).is_none());
        let french = recipe(Some("fr"));
        assert_eq!(french.identity.model_id, recognizer::PARAKEET_MODEL.id);
        let current = requests
            .admit(path, &french)
            .expect("DQ1_LANGUAGE_REPLACES_READY");
        assert!(old.is_cancelled());
        requests.update(path, &old, ready());
        assert!(
            matches!(requests.status(path), TranscriptStatus::Queued),
            "DQ1_STALE_REQUEST_CANNOT_PUBLISH"
        );
        requests.finish(path, &old);
        assert!(
            requests.cancel(path),
            "old completion cannot unregister its successor"
        );
        requests.update(path, &current, ready());
        assert!(matches!(requests.status(path), TranscriptStatus::Cancelled));
        assert!(
            requests.admit(path, &french).is_some(),
            "cancelled request can retry"
        );
    }
    #[test]
    fn dq1_authored_registration_cannot_be_overwritten_by_a_running_job() {
        let requests = Requests::new();
        let path = Path::new("authored.wav");
        let token = requests.admit(path, &recipe(None)).unwrap();
        let TranscriptStatus::Ready(transcript) = ready() else {
            unreachable!()
        };
        requests.register(path, (*transcript).clone());
        requests.update(
            path,
            &token,
            TranscriptStatus::Failed("late failure".into()),
        );
        assert!(matches!(requests.status(path), TranscriptStatus::Ready(_)));
        assert!(!requests.cancel(path));
        assert!(requests.admit(path, &recipe(Some("fr"))).is_none());
    }
    #[test]
    fn dq1_cancelled_job_preserves_an_explicit_failure_and_blocks_late_success() {
        let requests = Requests::new();
        let path = Path::new("refused.wav");
        let token = requests.admit(path, &recipe(None)).unwrap();
        token.cancel();
        requests.update(
            path,
            &token,
            TranscriptStatus::Failed("permission refused".into()),
        );
        assert!(
            matches!(requests.status(path), TranscriptStatus::Failed(reason) if reason == "permission refused"),
            "DQ1_CANCELLED_JOB_RETAINS_FAILURE"
        );
        requests.update(path, &token, ready());
        assert!(
            matches!(requests.status(path), TranscriptStatus::Cancelled),
            "DQ1_CANCELLED_JOB_CANNOT_PUBLISH_SUCCESS"
        );
    }
}

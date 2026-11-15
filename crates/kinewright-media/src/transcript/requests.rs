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
    Rejected(TranscriptStatus),
    Requested {
        recipe: Recipe,
        cancellation: ExportCancellation,
        status: TranscriptStatus,
        active: bool,
    },
}
impl Entry {
    fn status(&self) -> &TranscriptStatus {
        match self {
            Self::Registered(status) | Self::Rejected(status) | Self::Requested { status, .. } => {
                status
            }
        }
    }
    fn evictable(&self) -> bool {
        !matches!(self, Self::Requested { active: true, .. })
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
            .change(path, Entry::evictable, |previous| {
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
                        active: true,
                    }),
                    Some(cancellation),
                )
            })
            .flatten()
    }
    pub fn update(&self, path: &Path, cancellation: &ExportCancellation, status: TranscriptStatus) {
        self.states
            .change(path, Entry::evictable, |previous| match previous {
                Some(Entry::Requested {
                    recipe,
                    cancellation: current,
                    active,
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
                            active,
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
    pub fn evidence_recipe(&self, path: &Path) -> Option<Result<Arc<AssetTranscript>, Recipe>> {
        self.states
            .read(path, |entry| match entry {
                Some(Entry::Registered(TranscriptStatus::Ready(words))) => {
                    Some(Ok(Arc::clone(words)))
                }
                Some(Entry::Requested { recipe, .. }) => Some(Err(recipe.clone())),
                _ => None,
            })
            .flatten()
    }
    pub fn admit_cached_miss(&self, path: &Path) -> Option<(Recipe, ExportCancellation)> {
        self.states
            .change(path, Entry::evictable, |previous| {
                let recipe = match &previous {
                    None => Recipe {
                        identity: Identity::english(),
                        language: None,
                    },
                    Some(Entry::Requested {
                        recipe,
                        active: false,
                        status: TranscriptStatus::Ready(_) | TranscriptStatus::NoSpeech,
                        ..
                    }) => recipe.clone(),
                    _ => return (previous, None),
                };
                let Some(cancellation) = self.cancellations.replace(path) else {
                    return (previous, None);
                };
                (
                    Some(Entry::Requested {
                        recipe: recipe.clone(),
                        cancellation: cancellation.clone(),
                        status: TranscriptStatus::Queued,
                        active: true,
                    }),
                    Some((recipe, cancellation)),
                )
            })
            .flatten()
    }
    pub fn invalidate_finished(&self, path: &Path) {
        self.states.change(path, Entry::evictable, |entry| {
            (
                entry.filter(|entry| {
                    !entry.evictable() || !matches!(entry.status(), TranscriptStatus::NoSpeech)
                }),
                (),
            )
        });
    }
    pub fn register(&self, path: &Path, transcript: AssetTranscript) {
        self.register_status(path, TranscriptStatus::Ready(Arc::new(transcript)));
    }
    pub fn no_audio(&self, path: &Path) {
        self.register_status(path, TranscriptStatus::NoSpeech);
    }
    fn register_status(&self, path: &Path, status: TranscriptStatus) {
        self.states.change(path, Entry::evictable, |_| {
            self.cancellations.cancel(path);
            (Some(Entry::Registered(status)), ())
        });
    }
    pub fn reject(&self, path: &Path, error: &crate::recognizer::Error) {
        self.states.change(path, Entry::evictable, |previous| {
            if matches!(previous, Some(Entry::Registered(_))) {
                return (previous, ());
            }
            self.cancellations.cancel(path);
            (
                Some(Entry::Rejected(TranscriptStatus::Failed(error.to_string()))),
                (),
            )
        });
    }
    pub fn cancel(&self, path: &Path) -> bool {
        self.states
            .change(path, Entry::evictable, |previous| match previous {
                Some(Entry::Requested {
                    recipe,
                    cancellation,
                    status,
                    active,
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
                            active,
                        }),
                        cancelled,
                    )
                }
                previous => (previous, false),
            })
            .unwrap_or(false)
    }
    pub fn finish(&self, path: &Path, cancellation: &ExportCancellation) {
        self.states.change(path, Entry::evictable, |mut previous| {
            if let Some(Entry::Requested {
                cancellation: current,
                active,
                ..
            }) = &mut previous
                && current == cancellation
            {
                *active = false;
            }
            self.cancellations.finish(path, cancellation);
            (previous, ())
        });
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
            identity: Identity::for_model(recognizer::model_for_language(language).unwrap()),
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
    fn dq_media_cache_miss_refreshes_completed_recipe_without_replacing_live_or_authored_words() {
        let requests = Requests::new();
        let path = Path::new("missing-cache.wav");
        let french = recipe(Some("fr"));
        let first = requests.admit(path, &french).unwrap();
        requests.update(path, &first, ready());
        requests.finish(path, &first);
        let (refreshed, second) = requests.admit_cached_miss(path).unwrap();
        assert!(refreshed == french, "DQ_MEDIA_CACHE_REFRESH_RETAINS_RECIPE");
        assert!(matches!(requests.status(path), TranscriptStatus::Queued));
        assert!(requests.admit_cached_miss(path).is_none());
        assert!(
            !second.is_cancelled(),
            "DQ_MEDIA_CACHE_MISS_DOES_NOT_CANCEL_ACTIVE_JOB"
        );
        let TranscriptStatus::Ready(authored) = ready() else {
            unreachable!()
        };
        requests.register(path, (*authored).clone());
        assert!(
            requests.admit_cached_miss(path).is_none(),
            "DQ_MEDIA_AUTHORED_WORDS_RETAINED"
        );
        assert!(matches!(requests.status(path), TranscriptStatus::Ready(_)));
        let missing = Path::new("never-requested.wav");
        assert!(requests.admit_cached_miss(missing).is_some());
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
    #[test]
    fn dq1_live_request_completion_survives_status_capacity_pressure() {
        let requests = Requests::new();
        let recipe = recipe(None);
        let entries = (0..4097)
            .map(|index| {
                let path = std::path::PathBuf::from(format!("live-{index}.wav"));
                let token = requests.admit(&path, &recipe).unwrap();
                (path, token)
            })
            .collect::<Vec<_>>();
        for (path, token) in &entries {
            assert!(!token.is_cancelled());
            assert!(
                matches!(requests.status(path), TranscriptStatus::Queued),
                "DQ1_LIVE_STATUS_NOT_EVICTED"
            );
        }
        requests.update(&entries[0].0, &entries[0].1, ready());
        requests.finish(&entries[0].0, &entries[0].1);
        assert!(
            matches!(requests.status(&entries[0].0), TranscriptStatus::Ready(_)),
            "DQ1_LIVE_COMPLETION_RETAINED"
        );
        let terminal = (0..4097)
            .map(|index| std::path::PathBuf::from(format!("finished-{index}.wav")))
            .collect::<Vec<_>>();
        for path in &terminal {
            requests.no_audio(path);
        }
        assert!(
            terminal
                .iter()
                .filter(|path| matches!(requests.status(path), TranscriptStatus::NoSpeech))
                .count()
                <= 4096
        );
        for (path, _) in entries.iter().skip(1) {
            assert!(
                matches!(requests.status(path), TranscriptStatus::Queued),
                "DQ1_TERMINAL_EVICTION_SKIPS_LIVE"
            );
        }
        requests.update(&entries[1].0, &entries[1].1, ready());
        requests.finish(&entries[1].0, &entries[1].1);
        assert!(
            matches!(requests.status(&entries[1].0), TranscriptStatus::Ready(_)),
            "DQ1_LIVE_COMPLETION_RETAINED"
        );
        requests.update(Path::new("unknown.wav"), &entries[1].1, ready());
        assert!(
            matches!(
                requests.status(Path::new("unknown.wav")),
                TranscriptStatus::NotRequested
            ),
            "DQ1_UNKNOWN_TOKEN_CANNOT_PUBLISH"
        );
    }
    #[test]
    fn dq1_failed_request_is_admitted_again_and_old_updates_are_ignored() {
        let requests = Requests::new();
        let path = Path::new("retry.wav");
        let recipe = recipe(None);
        let old = requests.admit(path, &recipe).unwrap();
        requests.update(
            path,
            &old,
            TranscriptStatus::Failed("temporary failure".into()),
        );
        requests.finish(path, &old);
        let current = requests
            .admit(path, &recipe)
            .expect("DQ1_FAILED_REQUEST_CAN_RETRY");
        assert_ne!(current, old);
        requests.update(path, &old, ready());
        assert!(matches!(requests.status(path), TranscriptStatus::Queued));
        requests.update(path, &current, ready());
        requests.finish(path, &current);
        assert!(matches!(requests.status(path), TranscriptStatus::Ready(_)));
    }
}

use super::*;
use crossbeam_channel::{Sender, bounded};
use std::collections::VecDeque;

struct Scripted {
    finals: VecDeque<Frame>,
    interims: VecDeque<Frame>,
    ready: bool,
    closed: Option<Sender<()>>,
}
impl Scripted {
    fn new(finals: Vec<Frame>) -> Self {
        Self {
            finals: finals.into(),
            interims: VecDeque::new(),
            ready: false,
            closed: None,
        }
    }
}
impl Backend for Scripted {
    fn feed(&mut self, _: &[f32]) -> Result<(), Error> {
        Ok(())
    }
    fn next(&mut self) -> Result<Option<Frame>, Error> {
        if self.ready {
            self.ready = false;
            Ok(self.finals.pop_front())
        } else {
            Ok(self.interims.pop_front())
        }
    }
    fn commit(&mut self, _: u64) -> Result<(), Error> {
        self.ready = true;
        Ok(())
    }
    fn finish(&mut self) -> Result<(), Error> {
        self.ready = true;
        Ok(())
    }
    fn close(&mut self) {
        if let Some(closed) = self.closed.take() {
            closed.send(()).unwrap();
        }
    }
}
fn final_word(text: &str, start_us: i64, end_us: i64, processed_us: i64) -> Frame {
    Frame {
        final_result: true,
        text: text.into(),
        words: vec![WindowWord {
            text: text.into(),
            start_us,
            end_us,
        }],
        processed_us,
    }
}
fn feed_seconds(session: &mut dyn Session, seconds: usize) {
    for _ in 0..seconds * 100 / 16 {
        session.feed(&[0.0; FEED_SAMPLES]).unwrap();
    }
}
#[test]
fn dq1_commits_keep_one_exact_source_clock() {
    let fake = Scripted::new(vec![
        final_word("Um,", 1_000_000, 2_000_000, 60_000_000),
        final_word("Later.", 2_000_000, 3_000_000, 60_000_000),
    ]);
    let mut session = Driver::new(fake, 7_000_000, ExportCancellation::default(), FEED_SAMPLES);
    feed_seconds(&mut session, 60);
    session.commit_at(67_000_000).unwrap();
    let Update::Committed {
        words: first,
        audio_processed_us: progress,
    } = session.step().unwrap().unwrap()
    else {
        panic!("final");
    };
    assert_eq!(first[0].start_us, 8_000_000);
    assert_eq!(progress, 67_000_000);
    feed_seconds(&mut session, 60);
    session.commit_at(127_000_000).unwrap();
    let Update::Committed {
        words: second,
        audio_processed_us: progress,
    } = session.step().unwrap().unwrap()
    else {
        panic!("final");
    };
    assert_eq!(
        (second[0].start_us, second[0].end_us),
        (69_000_000, 70_000_000),
        "DQ1_COMMIT_OFFSET"
    );
    assert_eq!(progress, 127_000_000);
    assert!(second[0].start_us > first[0].end_us);
}
#[test]
fn dq1_session_rejects_misuse_and_requires_a_final() {
    let mut session = Driver::new(
        Scripted::new(vec![]),
        0,
        ExportCancellation::default(),
        FEED_SAMPLES,
    );
    assert!(matches!(session.finish(), Err(Error::EmptyAudio)));
    assert!(matches!(session.commit_at(0), Err(Error::SessionOrder(_))));
    assert!(matches!(session.feed(&[]), Err(Error::EmptyAudio)));
    assert!(matches!(
        session.feed(&[f32::NAN]),
        Err(Error::InvalidAudio)
    ));
    session.feed(&[0.0; FEED_SAMPLES]).unwrap();
    assert!(matches!(session.commit_at(1), Err(Error::SessionOrder(_))));
    session.finish().unwrap();
    assert!(matches!(session.feed(&[0.0]), Err(Error::SessionOrder(_))));
    assert!(matches!(session.finish(), Err(Error::SessionOrder(_))));
    assert!(matches!(
        session.step(),
        Err(Error::SessionOrder("backend omitted requested final"))
    ));
}
#[test]
fn dq1_progress_is_monotone() {
    let mut fake = Scripted::new(vec![]);
    fake.interims
        .extend([5_000, 3_000, 8_000].map(|processed_us| Frame {
            final_result: false,
            text: "Um".into(),
            words: vec![],
            processed_us,
        }));
    let mut session = Driver::new(fake, 0, ExportCancellation::default(), FEED_SAMPLES);
    session.feed(&[0.0; FEED_SAMPLES]).unwrap();
    let progress = (0..3)
        .map(|_| session.step().unwrap().unwrap().audio_processed_us())
        .collect::<Vec<_>>();
    assert_eq!(progress, [5_000, 5_000, 8_000]);
}
#[test]
fn dq1_cancel_between_steps_closes_the_backend() {
    let (closed, closure) = bounded(1);
    let cancel = ExportCancellation::default();
    let mut fake = Scripted::new(vec![]);
    fake.closed = Some(closed);
    let mut session = Driver::new(fake, 0, cancel.clone(), FEED_SAMPLES);
    session.feed(&[0.0; FEED_SAMPLES]).unwrap();
    assert!(session.step().unwrap().is_none());
    cancel.cancel();
    assert!(matches!(session.step(), Err(Error::Cancelled)));
    closure.recv().unwrap();
    assert!(matches!(session.feed(&[0.0]), Err(Error::Cancelled)));
}
#[test]
fn dq1_pause_finder_silence_constant_and_short_input() {
    let samples = vec![0.0; 65 * 16_000];
    assert_eq!(
        pause::quiet_point(&samples, &PiecePolicy::default()).unwrap(),
        60 * 16_000
    );
    let samples = vec![1.0; 65 * 16_000];
    assert_eq!(
        pause::quiet_point(&samples, &PiecePolicy::default()).unwrap(),
        60 * 16_000
    );
    assert_eq!(pause::quiet_point(&[], &PiecePolicy::default()).unwrap(), 0);
    assert_eq!(
        pause::quiet_point(&[1.0; 10], &PiecePolicy::default()).unwrap(),
        10
    );
}
#[test]
fn dq1_pause_finder_selects_measured_quiet_region() {
    let mut samples = vec![0.5; 65 * 16_000];
    samples[56 * 16_000..57 * 16_000].fill(0.0);
    let cut = pause::quiet_point(&samples, &PiecePolicy::default()).unwrap();
    assert!((56 * 16_000..57 * 16_000).contains(&cut));
}
#[test]
fn dq1_every_identity_field_changes_the_cache_key() {
    let base = Identity::english();
    let mutations = identity_mutations();
    for mutate in mutations {
        // Each altered request owns an independent identity snapshot.
        let mut altered = base.clone();
        mutate(&mut altered);
        assert_ne!(base.cache_key(), altered.cache_key());
    }
}

pub(crate) fn interrupted_recognition(cancel: &ExportCancellation) -> Result<Recognition, Error> {
    struct Cancelling {
        cancel: ExportCancellation,
        closed: Option<Sender<()>>,
    }
    impl Backend for Cancelling {
        fn feed(&mut self, _: &[f32]) -> Result<(), Error> {
            Ok(())
        }
        fn next(&mut self) -> Result<Option<Frame>, Error> {
            self.cancel.cancel();
            Ok(Some(Frame {
                final_result: false,
                text: "partial words".into(),
                words: vec![],
                processed_us: 10_000,
            }))
        }
        fn commit(&mut self, _: u64) -> Result<(), Error> {
            panic!("must cancel before commit")
        }
        fn finish(&mut self) -> Result<(), Error> {
            panic!("must cancel before finish")
        }
        fn close(&mut self) {
            if let Some(closed) = self.closed.take() {
                closed.send(()).unwrap();
            }
        }
    }
    struct Model {
        identity: Identity,
        closed: Option<Sender<()>>,
    }
    impl Recognizer for Model {
        fn identity(&self) -> &Identity {
            &self.identity
        }
        fn supports_streaming(&self) -> bool {
            true
        }
        fn session(
            &mut self,
            origin: u64,
            cancel: ExportCancellation,
        ) -> Result<Box<dyn Session + '_>, Error> {
            Ok(Box::new(Driver::new(
                Cancelling {
                    cancel: cancel.clone(),
                    closed: self.closed.take(),
                },
                origin,
                cancel,
                FEED_SAMPLES,
            )))
        }
    }
    let (closed, closure) = bounded(1);
    let mut model = Model {
        identity: Identity::english(),
        closed: Some(closed),
    };
    let mut fed = false;
    let result = queue::drive(
        &mut model,
        cancel,
        |window| {
            if fed {
                Ok(false)
            } else {
                window.extend(std::iter::repeat_n(0.0, FEED_SAMPLES));
                fed = true;
                Ok(true)
            }
        },
        |_| {},
        true,
    );
    closure
        .try_recv()
        .expect("stream closed before cancelled result is published");
    result
}

pub(crate) fn identity_mutations() -> [fn(&mut Identity); 14] {
    [
        |v| v.model_id.push('x'),
        |v| v.file_sha256.push('x'),
        |v| v.runtime_version.push('x'),
        |v| v.build_key.push('x'),
        |v| v.lookahead -= 1,
        |v| v.threads = if v.threads == 1 { 2 } else { 1 },
        |v| v.feed_samples -= 1,
        |v| v.punctuation = false,
        |v| v.verbatim = false,
        |v| v.piece_policy.target_us += 1,
        |v| v.piece_policy.search_us += 1,
        |v| v.piece_policy.level_window_us += 1,
        |v| v.piece_policy.quiet_span_us += 1,
        |v| v.piece_policy.pause_revision += 1,
    ]
}

#[test]
fn dq1_pause_policy_changes_the_cut_on_one_fixed_signal() {
    let mut signal = vec![0.5; 65 * 16_000];
    signal[56 * 16_000..57 * 16_000].fill(0.0);
    let standard = PiecePolicy::default();
    let nearby = PiecePolicy {
        search_us: 1_000_000,
        ..PiecePolicy::default()
    };
    let first = pause::quiet_point(&signal, &standard).unwrap();
    let second = pause::quiet_point(&signal, &nearby).unwrap();
    assert!((56 * 16_000..57 * 16_000).contains(&first));
    assert_eq!(second, 60 * 16_000);
    assert_ne!(first, second);
    assert_eq!(nearby.buffer_samples().unwrap(), 61 * 16_000);
}
#[test]
fn dq1_pause_policy_rejects_zero_resolution_and_unbounded_search() {
    for policy in [
        PiecePolicy {
            level_window_us: 0,
            ..PiecePolicy::default()
        },
        PiecePolicy {
            quiet_span_us: 0,
            ..PiecePolicy::default()
        },
        PiecePolicy {
            target_us: 0,
            ..PiecePolicy::default()
        },
        PiecePolicy {
            search_us: 60_000_000,
            ..PiecePolicy::default()
        },
    ] {
        assert!(matches!(
            pause::quiet_point(&[0.0; 160], &policy),
            Err(Error::InvalidPolicy(_))
        ));
    }
}

struct CumulativeStream {
    frames: VecDeque<crate::nemo::Frame>,
}
impl NativeStream for CumulativeStream {
    fn feed(&mut self, _: &[f32]) -> Result<(), crate::nemo::Error> {
        Ok(())
    }
    fn step(&mut self) -> Result<Option<crate::nemo::Frame>, crate::nemo::Error> {
        Ok(self.frames.pop_front())
    }
    fn commit(&mut self) -> Result<(), crate::nemo::Error> {
        Ok(())
    }
    fn finish(&mut self) -> Result<(), crate::nemo::Error> {
        Ok(())
    }
}
#[test]
fn dq1_native_cumulative_stream_keeps_the_source_clock_after_commits() {
    let frames = [1_000_000_i64, 61_000_000].map(|start| crate::nemo::Frame {
        final_result: true,
        text: "Um,".into(),
        audio_processed_us: start + 59_000_000,
        words: vec![crate::nemo::Word {
            text: "Um,".into(),
            start_us: start,
            end_us: start + 1_000_000,
        }],
    });
    let backend = NativeBackend {
        stream: Some(CumulativeStream {
            frames: frames.into(),
        }),
        epoch_us: 0,
        pending: None,
    };
    let mut session = Driver::new(
        backend,
        7_000_000,
        ExportCancellation::default(),
        FEED_SAMPLES,
    );
    let mut progress = 7_000_000;
    for (position, expected_start) in [(67_000_000, 8_000_000), (127_000_000, 68_000_000)] {
        feed_seconds(&mut session, 60);
        session.commit_at(position).unwrap();
        let Update::Committed {
            words,
            audio_processed_us,
        } = session.step().expect("DQ1_NATIVE_EPOCH").unwrap()
        else {
            panic!("commit final");
        };
        assert_eq!(words[0].start_us, expected_start, "DQ1_NATIVE_EPOCH");
        assert_eq!(words[0].end_us, expected_start + 1_000_000);
        assert!(audio_processed_us >= progress);
        assert_eq!(audio_processed_us, position);
        progress = audio_processed_us;
    }
}
#[test]
fn dq1_identity_uses_the_configured_thread_constant() {
    assert_eq!(Identity::english().threads, 4);
}
#[test]
fn dq1_final_word_cannot_run_past_the_fed_audio() {
    let mut session = Driver::new(
        Scripted::new(vec![final_word("Late", 0, 160_001, 160_000)]),
        0,
        ExportCancellation::default(),
        FEED_SAMPLES,
    );
    session.feed(&[0.0; FEED_SAMPLES]).unwrap();
    session.finish().unwrap();
    assert!(
        matches!(
            session.step(),
            Err(Error::Clock("invalid or unordered word times"))
        ),
        "DQ1_WORD_END_BOUND"
    );
}
#[cfg(unix)]
#[test]
fn dq1_session_rechecks_weights_before_opening_the_backend() {
    use crate::test_support::TempDirectory;
    use std::{cell::Cell, fs};
    const FILE: ModelFile = ModelFile {
        name: "synthetic.gguf",
        url: "",
        bytes: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    };
    let data = TempDirectory::new("dq1-session-check");
    let path = data.path(FILE.name);
    fs::write(&path, b"abc").unwrap();
    let verified = model::verify(
        &FILE,
        &path,
        &data.path("receipts"),
        &ExportCancellation::default(),
    )
    .unwrap();
    fs::write(&path, b"abd").unwrap();
    let opened = Cell::new(false);
    let result = checked_session(&verified, || {
        opened.set(true);
        Ok(())
    });
    assert!(
        matches!(result, Err(Error::Model(ModelError::Changed))),
        "DQ1_SESSION_FILE_RECHECK {result:?}"
    );
    assert!(!opened.get(), "invalidated weights cannot open the backend");
}
#[derive(Default)]
struct Observed {
    options: Option<crate::nemo::Options>,
    feeds: Vec<usize>,
    commits: Vec<u64>,
}
// A native stream genuinely borrows its model's observation state for the session.
struct RecordingStream<'a> {
    observed: &'a mut Observed,
    samples: u64,
    ready: bool,
}
impl NativeStream for RecordingStream<'_> {
    fn feed(&mut self, samples: &[f32]) -> Result<(), crate::nemo::Error> {
        self.observed.feeds.push(samples.len());
        self.samples += u64::try_from(samples.len()).unwrap();
        Ok(())
    }
    fn step(&mut self) -> Result<Option<crate::nemo::Frame>, crate::nemo::Error> {
        if !std::mem::take(&mut self.ready) {
            return Ok(None);
        }
        let end = i64::try_from(samples_us(self.samples)).unwrap();
        Ok(Some(crate::nemo::Frame {
            final_result: true,
            text: "Um,".into(),
            audio_processed_us: end,
            words: vec![crate::nemo::Word {
                text: "Um,".into(),
                start_us: end - 500_000,
                end_us: end - 400_000,
            }],
        }))
    }
    fn commit(&mut self) -> Result<(), crate::nemo::Error> {
        self.observed.commits.push(samples_us(self.samples));
        self.ready = true;
        Ok(())
    }
    fn finish(&mut self) -> Result<(), crate::nemo::Error> {
        self.ready = true;
        Ok(())
    }
}
struct RecordingRecognizer {
    identity: Identity,
    observed: Observed,
}
impl Recognizer for RecordingRecognizer {
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn supports_streaming(&self) -> bool {
        true
    }
    fn session(
        &mut self,
        origin: u64,
        cancel: ExportCancellation,
    ) -> Result<Box<dyn Session + '_>, Error> {
        let feed = self.identity.feed_samples;
        let observed = &mut self.observed;
        let backend = NativeBackend::open(&self.identity, |options| {
            observed.options = Some(options);
            Ok(RecordingStream {
                observed,
                samples: 0,
                ready: false,
            })
        })?;
        Ok(Box::new(Driver::new(backend, origin, cancel, feed)))
    }
}
#[test]
fn dq1_backend_observes_the_keyed_options_feeds_and_policy_commits() {
    let mut identity = Identity::english();
    identity.punctuation = false;
    identity.verbatim = false;
    identity.feed_samples = 1_280;
    identity.piece_policy = PiecePolicy {
        target_us: 1_000_000,
        search_us: 200_000,
        ..PiecePolicy::default()
    };
    let mut model = RecordingRecognizer {
        identity,
        observed: Observed::default(),
    };
    let mut remaining = 4 * 16_000;
    let result = queue::drive(
        &mut model,
        &ExportCancellation::default(),
        |window| {
            if remaining == 0 {
                return Ok(false);
            }
            let count = remaining.min(FEED_SAMPLES);
            window.resize(window.len() + count, 0.5);
            remaining -= count;
            Ok(true)
        },
        |_| {},
        true,
    )
    .unwrap();
    assert_eq!(
        model.observed.options,
        Some(crate::nemo::Options {
            punctuation: false,
            verbatim: false
        }),
        "DQ1_OPTIONS_FROM_IDENTITY"
    );
    assert_eq!(
        model.observed.feeds.iter().copied().max(),
        Some(1_280),
        "DQ1_FEEDS_FROM_IDENTITY"
    );
    assert_eq!(
        model.observed.commits,
        [1_000_000, 2_000_000, 3_000_000],
        "DQ1_COMMITS_FROM_POLICY"
    );
    assert_eq!(result.words.len(), 4);
    assert_eq!(result.words[3].start_us, 3_500_000);
}
#[test]
fn dq1_cancel_during_a_backend_call_discards_the_result() {
    struct Cancels(ExportCancellation);
    impl Backend for Cancels {
        fn feed(&mut self, _: &[f32]) -> Result<(), Error> {
            Ok(())
        }
        fn next(&mut self) -> Result<Option<Frame>, Error> {
            self.0.cancel();
            Ok(Some(final_word("stale", 0, 80_000, 160_000)))
        }
        fn commit(&mut self, _: u64) -> Result<(), Error> {
            Ok(())
        }
        fn finish(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn close(&mut self) {}
    }
    let cancel = ExportCancellation::default();
    let mut session = Driver::new(Cancels(cancel.clone()), 0, cancel, FEED_SAMPLES);
    session.feed(&[0.0; FEED_SAMPLES]).unwrap();
    session.finish().unwrap();
    assert!(
        matches!(session.step(), Err(Error::Cancelled)),
        "no publication after an in-flight call was cancelled"
    );
}

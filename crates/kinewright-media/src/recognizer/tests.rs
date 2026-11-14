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
    let mut session = Driver::new(fake, 7_000_000, ExportCancellation::default());
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
    let mut session = Driver::new(Scripted::new(vec![]), 0, ExportCancellation::default());
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
    let mut session = Driver::new(fake, 0, ExportCancellation::default());
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
    let mut session = Driver::new(fake, 0, cancel.clone());
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
    assert_eq!(pause::quiet_point(&samples), 60 * 16_000);
    let samples = vec![1.0; 65 * 16_000];
    assert_eq!(pause::quiet_point(&samples), 60 * 16_000);
    assert_eq!(pause::quiet_point(&[]), 0);
    assert_eq!(pause::quiet_point(&[1.0; 10]), 10);
}
#[test]
fn dq1_pause_finder_selects_measured_quiet_region() {
    let mut samples = vec![0.5; 65 * 16_000];
    samples[56 * 16_000..57 * 16_000].fill(0.0);
    let cut = pause::quiet_point(&samples);
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

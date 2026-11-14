//! A process-wide serial model worker. Handles never cross threads. Queued
//! requests reuse the model; an empty queue releases it before waiting again.
use super::{
    BuiltinRecognizer, Error, FEED_SAMPLES, Identity, Recognition, Recognizer, SAMPLE_RATE,
    Session, TimingSource, Update, cancelled, pause, samples_us,
};
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use kinewright_core::ExportCancellation;
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

// Taking the slot drops the previous model before a replacement is loaded.
struct Models<C> {
    value: Option<(&'static str, C)>,
}
impl<C> Models<C> {
    fn new() -> Self {
        Self { value: None }
    }
    fn resolve(
        &mut self,
        key: &'static str,
        identity: &Identity,
        valid: impl FnOnce(&C) -> bool,
        load: impl FnOnce() -> Result<C, Error>,
    ) -> Result<&mut C, Error>
    where
        C: Recognizer,
    {
        let next = match self.value.take() {
            Some((previous_key, loaded))
                if previous_key == key && loaded.identity() == identity && valid(&loaded) =>
            {
                loaded
            }
            previous => {
                drop(previous);
                load()?
            }
        };
        Ok(&mut self.value.insert((key, next)).1)
    }
}

type Task<C, R> = Box<dyn FnOnce(&mut Option<C>) -> Result<R, Error> + Send>;
struct Work<C, R> {
    task: Task<C, R>,
    complete: Box<dyn FnOnce(Result<R, Error>) + Send>,
}
fn run<C, R>(receiver: &Receiver<Work<C, R>>) {
    while let Ok(mut work) = receiver.recv() {
        let mut current = None;
        loop {
            let Work { task, complete } = work;
            // A failed operation's context is discarded before any further job.
            let result = if let Ok(result) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(&mut current)))
            {
                result
            } else {
                drop(current.take());
                Err(Error::WorkerPanicked)
            };
            match receiver.try_recv() {
                Ok(next) => {
                    complete(result);
                    work = next;
                }
                Err(TryRecvError::Empty) => {
                    drop(current.take());
                    complete(result);
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    drop(current.take());
                    complete(result);
                    return;
                }
            }
        }
    }
}

type ModelWork = Work<Models<BuiltinRecognizer>, Recognized>;
fn sender() -> Result<&'static Sender<ModelWork>, Error> {
    static QUEUE: OnceLock<Result<Sender<ModelWork>, String>> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let (send, receive) = bounded(1);
            std::thread::Builder::new()
                .name("kinewright-recognition".into())
                .spawn(move || run(&receive))
                .map_err(|e| e.to_string())?;
            Ok(send)
        })
        .as_ref()
        .map_err(|_| Error::QueueClosed)
}

pub(crate) struct Recognized {
    pub identity: Identity,
    pub recognition: Recognition,
}
pub(crate) fn queued(
    data_root: PathBuf,
    path: PathBuf,
    receipts: PathBuf,
    cancel: ExportCancellation,
    identity: Identity,
    work: impl FnOnce(&mut dyn Recognizer) -> Result<Recognition, Error> + Send + 'static,
) -> Result<Recognized, Error> {
    cancelled(&cancel)?;
    let grant = crate::locator::capture();
    let (send, receive) = bounded(1);
    sender()?
        .send(Work {
            task: Box::new(move |current| {
                let operation = || -> Result<Recognized, Error> {
                    let _authority = grant.as_ref().map(crate::locator::LocatorAuthority::enter);
                    let _access = crate::locator::enter_retained(grant.as_ref())?;
                    cancelled(&cancel)?;
                    let spec = identity.model()?;
                    let models = current.get_or_insert_with(Models::new);
                    let loaded = models.resolve(
                        spec.id,
                        &identity,
                        |loaded| loaded.verified.path() == path && loaded.verified.check().is_ok(),
                        || {
                            BuiltinRecognizer::load_identity(
                                &path,
                                &receipts,
                                &cancel,
                                identity.clone(),
                                Some(&data_root),
                            )
                        },
                    )?;
                    let recognition = work(loaded)?;
                    cancelled(&cancel)?;
                    // The cache result owns this identity after the queued model is released.
                    Ok(Recognized {
                        identity: loaded.identity().clone(),
                        recognition,
                    })
                };
                operation()
            }),
            complete: Box::new(move |result| {
                let _ = send.send(result);
            }),
        })
        .map_err(|_| Error::QueueClosed)?;
    receive.recv().map_err(|_| Error::QueueClosed)?
}

fn drain(
    session: &mut dyn Session,
    words: &mut Vec<super::Word>,
    progress: &mut impl FnMut(u64),
) -> Result<(), Error> {
    while let Some(update) = session.step()? {
        progress(update.audio_processed_us());
        if let Update::Committed {
            words: committed, ..
        } = update
        {
            words.extend(committed);
        }
    }
    Ok(())
}

pub(crate) fn drive(
    recognizer: &mut dyn Recognizer,
    cancel: &ExportCancellation,
    mut audio: impl FnMut(&mut Vec<f32>) -> Result<bool, Error>,
    mut progress: impl FnMut(u64),
    commits: bool,
) -> Result<Recognition, Error> {
    recognizer.identity().validate()?;
    let policy = recognizer.identity().piece_policy;
    let feed_samples = recognizer.identity().feed_samples;
    let window_samples = policy.buffer_samples()?;
    let mut session = recognizer.session(0, cancel.clone())?;
    let mut words = Vec::new();
    let mut window = Vec::new();
    let mut eof = false;
    let mut fed = 0_u64;
    loop {
        while !eof && window.len() < window_samples {
            cancelled(cancel)?;
            if !audio(&mut window)? {
                eof = true;
            }
        }
        if window.is_empty() {
            break;
        }
        let cut = if commits && !eof {
            pause::quiet_point(&window, &policy)?
        } else {
            window.len()
        };
        for samples in window[..cut].chunks(feed_samples) {
            session.feed(samples)?;
            drain(session.as_mut(), &mut words, &mut progress)?;
        }
        fed += u64::try_from(cut).map_err(|_| Error::Clock("decoded sample count overflow"))?;
        if commits && !eof {
            session.commit_at(samples_us(fed))?;
            drain(session.as_mut(), &mut words, &mut progress)?;
        }
        window.drain(..cut);
    }
    session.finish()?;
    drain(session.as_mut(), &mut words, &mut progress)?;
    cancelled(cancel)?;
    Ok(Recognition {
        timing_source: TimingSource::TokenDurationsLagged,
        words,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "the data root is a security boundary distinct from the selected model directory"
)]
pub(crate) fn file(
    data_root: &Path,
    model_root: &Path,
    path: PathBuf,
    frames: kinewright_core::TimeCode,
    fps: kinewright_core::Rational,
    cancel: ExportCancellation,
    identity: Identity,
    progress: impl FnMut(u64) + Send + 'static,
) -> Result<Recognized, Error> {
    let request_cancel = cancel.clone();
    queued(
        data_root.to_path_buf(),
        model_root.join(identity.model()?.files[0].name),
        model_root.join("verified"),
        cancel,
        identity,
        move |recognizer| {
            let end = crate::clock::frame_to_samples(frames, SAMPLE_RATE, fps);
            let mut decoder = crate::audio::AudioDecoder::open(&path, SAMPLE_RATE, 1, 0, end)?;
            drive(
                recognizer,
                &request_cancel,
                |window| {
                    if let Some(chunk) = decoder.next_chunk()? {
                        window.extend(chunk);
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                },
                progress,
                true,
            )
        },
    )
}

pub(crate) fn samples(
    data_root: &Path,
    model_root: &Path,
    samples: Vec<f32>,
    cancel: ExportCancellation,
    identity: Identity,
) -> Result<Recognized, Error> {
    let request_cancel = cancel.clone();
    queued(
        data_root.to_path_buf(),
        model_root.join(identity.model()?.files[0].name),
        model_root.join("verified"),
        cancel,
        identity,
        move |recognizer| {
            let mut chunks = samples.chunks(FEED_SAMPLES);
            // Voice notes are already bounded PCM; only the rolling pause window owns a copy.
            drive(
                recognizer,
                &request_cancel,
                |window| {
                    if let Some(chunk) = chunks.next() {
                        window.extend_from_slice(chunk);
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                },
                |_| {},
                true,
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Context {
        dropped: Sender<()>,
    }
    impl Drop for Context {
        fn drop(&mut self) {
            let _ = self.dropped.send(());
        }
    }
    #[test]
    fn dq1_queued_job_waits_reuses_model_and_releases_before_idle_reply() {
        let (send, receive) = bounded::<Work<Context, ()>>(1);
        let worker = std::thread::spawn(move || run(&receive));
        let (arrived, arrival) = bounded(0);
        let (release, released) = bounded(0);
        let (second, second_started) = bounded(1);
        let (done, completed) = bounded(0);
        let (dropped, drop_observed) = bounded(1);
        send.send(Work {
            task: Box::new(move |current| {
                assert!(current.is_none());
                *current = Some(Context { dropped });
                arrived.send(()).unwrap();
                released.recv().unwrap();
                Ok(())
            }),
            complete: Box::new(|result| result.unwrap()),
        })
        .unwrap();
        arrival.recv().unwrap();
        send.send(Work {
            task: Box::new(move |current| {
                assert!(current.is_some(), "queued model must be reused");
                second.send(()).unwrap();
                Ok(())
            }),
            complete: Box::new(move |result| {
                result.unwrap();
                done.send(()).unwrap();
            }),
        })
        .unwrap();
        assert!(matches!(
            second_started.try_recv(),
            Err(TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        second_started.recv().unwrap();
        completed.recv().unwrap();
        drop_observed
            .try_recv()
            .expect("idle context released before result delivery");
        drop(send);
        worker.join().unwrap();
    }
    #[test]
    fn dq1_panicked_job_and_its_queued_successor_both_get_answers() {
        let (send, receive) = bounded::<Work<Context, &'static str>>(1);
        let worker = std::thread::spawn(move || run(&receive));
        let (arrived, arrival) = bounded(0);
        let (release, released) = bounded(0);
        let (dropped, drop_observed) = bounded(1);
        let (first, first_reply) = bounded(1);
        let (second, second_reply) = bounded(1);
        send.send(Work {
            task: Box::new(move |current| {
                *current = Some(Context { dropped });
                arrived.send(()).unwrap();
                released.recv().unwrap();
                // Resume a genuine Rust unwind without the global panic hook's output
                // interleaving libtest's result line in the isolated CI harness.
                std::panic::resume_unwind(Box::new("injected recognition panic"))
            }),
            complete: Box::new(move |result| {
                let _ = first.send(result);
            }),
        })
        .unwrap();
        arrival.recv().unwrap();
        send.send(Work {
            task: Box::new(move |current| {
                assert!(current.is_none(), "DQ1_PANIC_DISCARDS_MODEL");
                drop_observed
                    .try_recv()
                    .expect("failed model dropped before successor starts");
                Ok("successor ran")
            }),
            complete: Box::new(move |result| {
                let _ = second.send(result);
            }),
        })
        .unwrap();
        assert!(matches!(second_reply.try_recv(), Err(TryRecvError::Empty)));
        release.send(()).unwrap();
        assert!(
            matches!(first_reply.recv().unwrap(), Err(Error::WorkerPanicked)),
            "DQ1_PANIC_TYPED_REPLY"
        );
        assert_eq!(
            second_reply.recv().unwrap().unwrap(),
            "successor ran",
            "DQ1_PANIC_QUEUED_REPLY"
        );
        let (third, third_reply) = bounded(1);
        send.send(Work {
            task: Box::new(|_| Ok("worker remains alive")),
            complete: Box::new(move |result| {
                let _ = third.send(result);
            }),
        })
        .unwrap();
        assert_eq!(third_reply.recv().unwrap().unwrap(), "worker remains alive");
        drop(send);
        worker.join().unwrap();
    }
    struct TrackedModel {
        identity: Identity,
        live: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl TrackedModel {
        fn load(identity: Identity, live: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
            assert_eq!(
                live.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "DQ1_MODEL_DROP_BEFORE_LOAD"
            );
            live.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Self {
                identity,
                live: std::sync::Arc::clone(live),
            }
        }
    }
    impl Drop for TrackedModel {
        fn drop(&mut self) {
            self.live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    impl Recognizer for TrackedModel {
        fn identity(&self) -> &Identity {
            &self.identity
        }
        fn supports_streaming(&self) -> bool {
            false
        }
        fn session(
            &mut self,
            _: u64,
            _: ExportCancellation,
        ) -> Result<Box<dyn Session + '_>, Error> {
            Err(Error::SessionOrder("model residency fixture"))
        }
    }
    #[test]
    fn dq1_queued_model_switch_drops_before_loading_and_before_idle_reply() {
        let live = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (send, receive) = bounded::<Work<Models<TrackedModel>, ()>>(1);
        let worker = std::thread::spawn(move || run(&receive));
        let (arrived, arrival) = bounded(0);
        let (release, released) = bounded(0);
        let (first, first_reply) = bounded(1);
        let (second, second_reply) = bounded(1);
        let first_live = std::sync::Arc::clone(&live);
        send.send(Work {
            task: Box::new(move |current| {
                let identity = Identity::english();
                current.get_or_insert_with(Models::new).resolve(
                    "english",
                    &identity,
                    |_| true,
                    || Ok(TrackedModel::load(identity.clone(), &first_live)),
                )?;
                arrived.send(()).unwrap();
                released.recv().unwrap();
                Ok(())
            }),
            complete: Box::new(move |r| {
                first.send(r).unwrap();
            }),
        })
        .unwrap();
        arrival.recv().unwrap();
        let second_live = std::sync::Arc::clone(&live);
        send.send(Work {
            task: Box::new(move |current| {
                let identity = Identity::for_model(&super::super::PARAKEET_MODEL);
                current.as_mut().unwrap().resolve(
                    "parakeet",
                    &identity,
                    |_| true,
                    || Ok(TrackedModel::load(identity.clone(), &second_live)),
                )?;
                assert_eq!(
                    second_live.load(std::sync::atomic::Ordering::SeqCst),
                    1,
                    "DQ1_ONE_RESIDENT_MODEL"
                );
                Ok(())
            }),
            complete: Box::new(move |r| {
                second.send(r).unwrap();
            }),
        })
        .unwrap();
        release.send(()).unwrap();
        first_reply.recv().unwrap().unwrap();
        second_reply.recv().unwrap().unwrap();
        assert_eq!(
            live.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "DQ1_IDLE_RELEASE_BEFORE_REPLY"
        );
        drop(send);
        worker.join().unwrap();
    }
    #[test]
    fn dq1_loaded_model_identity_change_reloads_the_same_id() {
        let live = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut models = Models::new();
        let first = Identity::english();
        models
            .resolve(
                "english",
                &first,
                |_| true,
                || Ok(TrackedModel::load(first.clone(), &live)),
            )
            .unwrap();
        models
            .resolve(
                "english",
                &first,
                |_| true,
                || panic!("same identity should reuse"),
            )
            .unwrap();
        let mut second = first.clone();
        second.lookahead = Some(7);
        let mut loaded = false;
        let result = models
            .resolve(
                "english",
                &second,
                |_| true,
                || {
                    loaded = true;
                    Ok(TrackedModel::load(second.clone(), &live))
                },
            )
            .unwrap();
        assert_eq!(result.identity(), &second, "DQ1_CHANGED_IDENTITY_RELOADS");
        assert!(loaded, "DQ1_CHANGED_IDENTITY_LOAD_CALLED");
    }
    #[test]
    fn dq1_error_and_cancel_answer_their_requester_and_run_the_successor() {
        for cancelled in [false, true] {
            let (send, receive) = bounded::<Work<(), &'static str>>(1);
            let worker = std::thread::spawn(move || run(&receive));
            let (arrived, arrival) = bounded(0);
            let (release, released) = bounded(0);
            let (first, first_reply) = bounded(1);
            let (second, second_reply) = bounded(1);
            send.send(Work {
                task: Box::new(move |_| {
                    arrived.send(()).unwrap();
                    released.recv().unwrap();
                    Err(if cancelled {
                        Error::Cancelled
                    } else {
                        Error::InvalidAudio
                    })
                }),
                complete: Box::new(move |r| {
                    first.send(r).unwrap();
                }),
            })
            .unwrap();
            arrival.recv().unwrap();
            send.send(Work {
                task: Box::new(|_| Ok("successor")),
                complete: Box::new(move |r| {
                    second.send(r).unwrap();
                }),
            })
            .unwrap();
            assert!(matches!(second_reply.try_recv(), Err(TryRecvError::Empty)));
            release.send(()).unwrap();
            let first = first_reply.recv().expect("DQ1_JOB_OWNS_ERROR_REPLY");
            assert!(
                matches!(
                    (&first, cancelled),
                    (Err(Error::Cancelled), true) | (Err(Error::InvalidAudio), false)
                ),
                "DQ1_JOB_OWNS_ERROR_REPLY"
            );
            assert_eq!(
                second_reply.recv().unwrap().unwrap(),
                "successor",
                "DQ1_ERROR_SUCCESSOR_REPLY"
            );
            drop(send);
            worker.join().unwrap();
        }
    }
}

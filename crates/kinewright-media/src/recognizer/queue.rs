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

type Completion = Box<dyn FnOnce() + Send>;
type Work<C> = Box<dyn FnOnce(&mut Option<C>) -> Completion + Send>;
fn run<C>(receiver: &Receiver<Work<C>>) {
    while let Ok(mut work) = receiver.recv() {
        let mut current = None;
        loop {
            let complete = work(&mut current);
            match receiver.try_recv() {
                Ok(next) => {
                    complete();
                    work = next;
                }
                Err(TryRecvError::Empty) => {
                    drop(current.take());
                    complete();
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    drop(current.take());
                    complete();
                    return;
                }
            }
        }
    }
}
fn sender() -> Result<&'static Sender<Work<BuiltinRecognizer>>, Error> {
    static QUEUE: OnceLock<Result<Sender<Work<BuiltinRecognizer>>, String>> = OnceLock::new();
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
        .send(Box::new(move |current| {
            let operation = || -> Result<Recognized, Error> {
                let _authority = grant.as_ref().map(crate::locator::LocatorAuthority::enter);
                let _access = crate::locator::enter_retained(grant.as_ref())?;
                cancelled(&cancel)?;
                if current.as_ref().is_some_and(|loaded| {
                    loaded.identity() != &identity
                        || loaded.verified.path() != path
                        || loaded.verified.check().is_err()
                }) {
                    *current = None;
                }
                if current.is_none() {
                    *current = Some(BuiltinRecognizer::load_identity(
                        &path,
                        &receipts,
                        &cancel,
                        identity,
                        Some(&data_root),
                    )?);
                }
                let loaded = current.as_mut().ok_or(Error::QueueClosed)?;
                let recognition = work(loaded)?;
                cancelled(&cancel)?;
                // The cache result owns this identity after the queued model is released.
                Ok(Recognized {
                    identity: loaded.identity().clone(),
                    recognition,
                })
            };
            let result = operation();
            Box::new(move || {
                let _ = send.send(result);
            })
        }))
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
        model_root.join(super::ENGLISH_MODEL.files[0].name),
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
        model_root.join(super::ENGLISH_MODEL.files[0].name),
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
            self.dropped.send(()).unwrap();
        }
    }
    #[test]
    fn dq1_queued_job_waits_reuses_model_and_releases_before_idle_reply() {
        let (send, receive) = bounded::<Work<Context>>(1);
        let worker = std::thread::spawn(move || run(&receive));
        let (arrived, arrival) = bounded(0);
        let (release, released) = bounded(0);
        let (second, second_started) = bounded(1);
        let (done, completed) = bounded(0);
        let (dropped, drop_observed) = bounded(1);
        send.send(Box::new(move |current| {
            assert!(current.is_none());
            *current = Some(Context { dropped });
            arrived.send(()).unwrap();
            released.recv().unwrap();
            Box::new(|| {})
        }))
        .unwrap();
        arrival.recv().unwrap();
        send.send(Box::new(move |current| {
            assert!(current.is_some(), "queued model must be reused");
            second.send(()).unwrap();
            Box::new(move || {
                done.send(()).unwrap();
            })
        }))
        .unwrap();
        assert!(
            matches!(second_started.try_recv(), Err(TryRecvError::Empty)),
            "second job cannot start while first is held"
        );
        release.send(()).unwrap();
        second_started.recv().unwrap();
        completed.recv().unwrap();
        drop_observed
            .try_recv()
            .expect("idle model released before result delivery");
        drop(send);
        worker.join().unwrap();
    }
}

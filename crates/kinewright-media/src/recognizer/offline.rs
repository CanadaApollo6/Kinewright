//! An offline model implements the same session: buffer one bounded piece,
//! decode on the step after commit/finish, then return window-relative words.
use super::{Backend, Error, Frame, PiecePolicy};

pub(super) struct Offline<F> {
    recognize: F,
    samples: Vec<f32>,
    limit: usize,
    pending: bool,
    closed: bool,
}
impl<F: FnMut(&[f32]) -> Result<crate::nemo::Frame, Error>> Offline<F> {
    pub(super) fn new(recognize: F, policy: &PiecePolicy) -> Result<Self, Error> {
        Ok(Self {
            recognize,
            samples: Vec::new(),
            limit: policy.buffer_samples()?,
            pending: false,
            closed: false,
        })
    }
}
impl<F: FnMut(&[f32]) -> Result<crate::nemo::Frame, Error>> Backend for Offline<F> {
    fn feed(&mut self, samples: &[f32]) -> Result<(), Error> {
        if self.closed || self.pending {
            return Err(Error::SessionOrder(
                "offline piece must be drained before feeding",
            ));
        }
        if samples.len() > self.limit.saturating_sub(self.samples.len()) {
            return Err(Error::SessionOrder(
                "offline piece exceeds the pause policy; commit first",
            ));
        }
        self.samples.extend_from_slice(samples);
        Ok(())
    }
    fn commit(&mut self, _: u64) -> Result<(), Error> {
        if self.closed || self.pending {
            return Err(Error::SessionOrder(
                "offline piece already pending or closed",
            ));
        }
        self.pending = true;
        Ok(())
    }
    fn finish(&mut self) -> Result<(), Error> {
        self.commit(0)
    }
    fn next(&mut self) -> Result<Option<Frame>, Error> {
        if !self.pending {
            return Ok(None);
        }
        let raw = if self.samples.is_empty() {
            crate::nemo::Frame {
                final_result: true,
                text: String::new(),
                words: Vec::new(),
                audio_processed_us: 0,
            }
        } else {
            (self.recognize)(&self.samples)?
        };
        if !raw.final_result {
            return Err(Error::SessionOrder("offline recognition omitted its final"));
        }
        self.samples.clear();
        self.pending = false;
        Ok(Some(Frame {
            final_result: true,
            text: raw.text,
            words: raw
                .words
                .into_iter()
                .map(|word| super::WindowWord {
                    text: word.text,
                    start_us: word.start_us,
                    end_us: word.end_us,
                })
                .collect(),
            processed_us: raw.audio_processed_us,
        }))
    }
    fn close(&mut self) {
        self.closed = true;
        self.samples = Vec::new();
    }
}

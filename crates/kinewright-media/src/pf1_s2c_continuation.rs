//! PF1 S2c-2: [`ContinuationPath`], the C-4 witnesses' adapter over the
//! production S-2 continuation (`VideoDecoder::decode_paused`, the call a
//! paused reader makes). It injects only what the witnesses inject (the stop
//! signal through `arm`, timestamp and shadow faults) and reports only
//! through `observe`; cancellation between produced frames is the decoder's
//! own check (R39 item 3, R46: a source-lines guard test keeps it so).

use std::rc::Rc;

use kinewright_core::TimeCode;

use super::{Before, CancelAt, Cursor, Event, Observation, TargetPath, arm, observe};
use crate::{
    cache::FrameCache,
    decode::{ShadowFault, Tamper, VideoDecoder},
    pf1_s2c_fixtures::Fixture,
};

/// One reader: a `VideoDecoder` kept across calls. `produce(Some(Cursor(c)),
/// t)` hands `c` to `decode_paused`, which continues only in the S-2 domain
/// and only if the decoder's own cursor is `c`, and otherwise seeks. A
/// replaced decoder (`Relink`, `KeyChange`, `ShrinkReopen`) is dropped and
/// reopened on the next call, as a reader's is; an `Error` resets the
/// decoder as a failed decode does (`reset_continuation`).
pub(in crate::pf1_s2c_witness) struct ContinuationPath {
    fx: Rc<Fixture>,
    threads: usize,
    decoder: Option<VideoDecoder>,
    cancel: Option<usize>,
    /// Timestamp faults, for every decoder from now on.
    tampers: Vec<Tamper>,
    /// A shadow fault, for every decoder from now on.
    fault: Option<ShadowFault>,
    /// A `MismatchOnce` waiting to reach the decoder (its next seek takes it).
    mismatch_once: bool,
}

impl ContinuationPath {
    pub(in crate::pf1_s2c_witness) fn new(fx: &Rc<Fixture>, threads: usize) -> Self {
        Self {
            fx: fx.clone(),
            threads,
            decoder: None,
            cancel: None,
            tampers: Vec::new(),
            fault: None,
            mismatch_once: false,
        }
    }

    /// The reader's decoder, opened as a reader opens it (with the faults
    /// injected so far) if it has none.
    fn decoder(&mut self) -> &mut VideoDecoder {
        let (fx, threads) = (&self.fx, self.threads);
        let (tampers, fault) = (&self.tampers, self.fault);
        self.decoder.get_or_insert_with(|| {
            let mut decoder = fx.open(threads);
            for tamper in tampers {
                decoder.tamper(*tamper);
            }
            if let Some(fault) = fault {
                decoder.fault_shadow(fault);
            }
            decoder
        })
    }
}

impl TargetPath for ContinuationPath {
    fn produce(&mut self, from: Option<Cursor>, t: TimeCode) -> Observation {
        let cancel = self.cancel.take();
        let once = std::mem::take(&mut self.mismatch_once);
        let decoder = self.decoder();
        if once {
            decoder.mismatch_once();
        }
        arm(decoder, cancel, CancelAt::Exact);
        let before = Before::of(decoder);
        let mut cache = FrameCache::new(1);
        let result = decoder.decode_paused(from.map(|c| c.0), t, &mut cache);
        observe(decoder, &mut cache, t, result, before)
    }

    fn event(&mut self, event: Event) {
        let fault = match event {
            Event::Relink(fx) => {
                (self.fx, self.decoder) = (fx, None);
                return;
            }
            Event::KeyChange | Event::ShrinkReopen => {
                self.decoder = None;
                return;
            }
            Event::Error => {
                if let Some(decoder) = &mut self.decoder {
                    decoder.reset_continuation();
                }
                return;
            }
            Event::Cancel(n) => {
                self.cancel = Some(n);
                return;
            }
            Event::MismatchOnce => {
                self.mismatch_once = true;
                return;
            }
            Event::Tamper(t) => {
                self.tampers.push(t);
                if let Some(decoder) = &mut self.decoder {
                    decoder.tamper(t);
                }
                return;
            }
            Event::ShadowFails => ShadowFault::Fails,
            Event::ShadowMismatch => ShadowFault::Mismatch,
            Event::StreamMismatch => ShadowFault::StreamMismatch,
            Event::AnchorDrifts => ShadowFault::Drifts,
        };
        self.fault = Some(fault);
        if let Some(decoder) = &mut self.decoder {
            decoder.fault_shadow(fault);
        }
    }
}

use super::Phase;
use std::{
    fmt,
    time::{Duration, Instant},
};

/// Elapsed monotonic time up to the end of the probe or the detected deadline.
/// Zero indicates a phase that did not run, such as scope setup in degraded mode.
#[derive(Clone, Copy, Debug)]
pub struct ProbeTiming {
    pub phase: Phase,
    pub supervisor_handshake: Duration,
    pub scope_start_and_verify: Duration,
    pub helper_handshake: Duration,
    pub request_to_reply: Duration,
}

impl Default for ProbeTiming {
    fn default() -> Self {
        Self {
            phase: Phase::Launch,
            supervisor_handshake: Duration::ZERO,
            scope_start_and_verify: Duration::ZERO,
            helper_handshake: Duration::ZERO,
            request_to_reply: Duration::ZERO,
        }
    }
}

impl ProbeTiming {
    pub(super) fn add(&mut self, earlier: Self) {
        self.supervisor_handshake += earlier.supervisor_handshake;
        self.scope_start_and_verify += earlier.scope_start_and_verify;
        self.helper_handshake += earlier.helper_handshake;
        self.request_to_reply += earlier.request_to_reply;
    }

    fn spend(&mut self, elapsed: Duration) {
        match self.phase {
            Phase::CreateScope | Phase::VerifyScope => self.scope_start_and_verify += elapsed,
            Phase::Forked | Phase::Boundary | Phase::Ready => self.helper_handshake += elapsed,
            Phase::ProbeResult | Phase::Complete => self.request_to_reply += elapsed,
            _ => {}
        }
    }

    pub(super) fn report(self, success: bool) {
        #[cfg(any(test, feature = "probe-phase-logs", feature = "slow-tests"))]
        eprintln!(
            "MS_T10_PROBE_PHASES outcome={} {self}",
            if success { "ok" } else { "timeout" }
        );
        #[cfg(not(any(test, feature = "probe-phase-logs", feature = "slow-tests")))]
        let _ = (self, success);
    }
}

impl fmt::Display for ProbeTiming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "phase={} supervisor_handshake_us={} scope_start_verify_us={} helper_handshake_us={} request_to_reply_us={}",
            self.phase,
            self.supervisor_handshake.as_micros(),
            self.scope_start_and_verify.as_micros(),
            self.helper_handshake.as_micros(),
            self.request_to_reply.as_micros(),
        )
    }
}

/// The I/O worker marks transitions and the actor can snapshot a live deadline.
/// The existing worker and actor share this state without holding it across I/O.
pub(super) struct Progress {
    timing: ProbeTiming,
    entered: Instant,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            timing: ProbeTiming::default(),
            entered: Instant::now(),
        }
    }
}

impl Progress {
    pub(super) fn enter(&mut self, phase: Phase) {
        if self.timing.phase == phase {
            return;
        }
        let now = Instant::now();
        self.timing.spend(now.duration_since(self.entered));
        self.timing.phase = phase;
        self.entered = now;
    }

    pub(super) fn snapshot(&self) -> ProbeTiming {
        let mut timing = self.timing;
        timing.spend(self.entered.elapsed());
        timing
    }
}

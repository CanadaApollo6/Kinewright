use ffmpeg_next::frame::Video;
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

thread_local! {
    pub(crate) static START: RefCell<Option<Arc<Ledger>>> = const { RefCell::new(None) };
}

// AVBuffer padding beyond the plane spans is not observed;
// FFmpeg-internal buffer-pool lifetimes are not observed;
// identity means plane address plus a lifetime generation, and a generation is recorded at each registration so that a reused pooled address is a new identity.
pub(crate) fn spans(frame: &Video) -> Vec<[usize; 3]> {
    (0..frame.planes())
        .map(|i| {
            let data = frame.data(i);
            let start = data.as_ptr() as usize;
            [
                start,
                start.checked_add(data.len()).expect("plane end"),
                frame.stride(i),
            ]
        })
        .collect()
}

pub(crate) fn union(mut intervals: Vec<[usize; 3]>) -> usize {
    intervals.sort_unstable();
    let (mut end, mut bytes) = (0, 0);
    for [start, next, _] in intervals {
        assert!(next >= start);
        bytes += next.saturating_sub(end.max(start));
        end = end.max(next);
    }
    bytes
}

#[derive(Clone, Serialize)]
pub(crate) struct Record {
    pub(crate) owner: u64,
    pub(crate) window: u64,
    pub(crate) spans: Vec<[usize; 3]>,
    pub(crate) retired: bool,
}

#[derive(Default, Serialize)]
pub(crate) struct State {
    next: u64,
    owners: BTreeMap<u64, bool>,
    pub(crate) records: BTreeMap<u64, Record>,
    invalid: bool,
}

pub(crate) struct Ledger {
    pub(crate) state: Mutex<State>,
    pub(crate) d: usize,
    gate: Mutex<Option<(i64, bool)>>,
    ready: std::sync::Condvar,
}

impl Ledger {
    pub(crate) fn new(d: usize) -> Arc<Self> {
        assert!(d > 0);
        Arc::new(Self {
            state: Mutex::default(),
            d,
            gate: Mutex::default(),
            ready: std::sync::Condvar::new(),
        })
    }

    pub(crate) fn arm(&self, at: i64) {
        *self.gate.lock().expect("D13 gate") = Some((at, false));
    }
    pub(crate) fn release(&self) {
        *self.gate.lock().expect("D13 gate") = None;
        self.ready.notify_all();
    }
    pub(crate) fn held(&self) -> bool {
        self.gate
            .lock()
            .expect("D13 gate")
            .is_some_and(|(_, held)| held)
    }
    pub(crate) fn boundary(&self, at: Option<i64>) {
        let mut gate = self.gate.lock().expect("D13 gate");
        if gate.is_some_and(|(target, held)| held || at == Some(target)) {
            gate.as_mut().expect("armed").1 = true;
            self.ready.notify_all();
            let (guard, timeout) = self
                .ready
                .wait_timeout_while(gate, std::time::Duration::from_secs(10), |g| g.is_some())
                .expect("D13 gate wait");
            drop(guard);
            assert!(!timeout.timed_out(), "D13 boundary exceeded 10 s");
        }
    }

    fn check(&self, state: &State) -> bool {
        let owners: Vec<_> = state.owners.iter().filter(|(_, live)| **live).collect();
        let retained: Vec<_> = state
            .records
            .values()
            .filter(|r| !r.retired && r.window != 0)
            .collect();
        owners.len() <= 2
            && owners.iter().all(|(id, _)| {
                let frames: Vec<_> = retained.iter().filter(|r| r.owner == **id).collect();
                let windows: std::collections::BTreeSet<_> =
                    frames.iter().map(|r| r.window).collect();
                windows.len() <= 1
                    && frames.len() <= 15
                    && union(frames.iter().flat_map(|r| r.spans.clone()).collect()) <= 15 * self.d
            })
            && retained.iter().all(|r| {
                state.owners.get(&r.owner) == Some(&true) && union(r.spans.clone()) <= self.d
            })
            && union(retained.iter().flat_map(|r| r.spans.clone()).collect()) <= 30 * self.d
    }

    pub(crate) fn snapshot(&self) -> serde_json::Value {
        let state = self.state.lock().expect("D13 ledger");
        let valid = !state.invalid && self.check(&state);
        let snapshot = serde_json::to_value(&*state).expect("D13 records");
        drop(state);
        assert!(valid, "D13 retained bound");
        snapshot
    }

    pub(crate) fn retained(&self) -> Vec<u64> {
        self.state
            .lock()
            .expect("D13 ledger")
            .records
            .iter()
            .filter(|(_, r)| !r.retired && r.window != 0)
            .map(|(id, _)| *id)
            .collect()
    }

    pub(crate) fn retired(&self, ids: &[u64]) -> bool {
        let state = self.state.lock().expect("D13 ledger");
        ids.iter()
            .all(|id| state.records.get(id).is_some_and(|r| r.retired))
    }

    pub(crate) fn closed(&self) -> bool {
        let state = self.state.lock().expect("D13 ledger");
        state.owners.values().all(|live| !live) && state.records.values().all(|r| r.retired)
    }
}

pub(crate) struct Owner(Arc<Ledger>, u64);
impl Owner {
    pub(crate) fn new() -> Option<Self> {
        START.with_borrow(|ledger| {
            ledger.as_ref().map(|ledger| {
                let mut state = ledger.state.lock().expect("D13 ledger");
                assert!(state.owners.len() < 256, "D13 owner history overflow");
                state.next += 1;
                let id = state.next;
                state.owners.insert(id, true);
                Self(Arc::clone(ledger), id)
            })
        })
    }

    pub(crate) fn register(&self, frame: &Video, window: u64) -> Registration {
        let mut state = self.0.state.lock().expect("D13 ledger");
        assert!(
            state.records.len() < 100_000,
            "D13 registration history overflow"
        );
        state.next += 1;
        let id = state.next;
        state.records.insert(
            id,
            Record {
                owner: self.1,
                window,
                spans: spans(frame),
                retired: false,
            },
        );
        state.invalid = state.invalid || !self.0.check(&state);
        Registration(Arc::clone(&self.0), id)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .expect("D13 ledger")
            .owners
            .insert(self.1, false);
    }
}
pub(crate) struct Registration(Arc<Ledger>, u64);
impl Drop for Registration {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().expect("D13 ledger");
        let record = state.records.get_mut(&self.1).expect("registered identity");
        assert!(!record.retired, "D13 double retire");
        record.retired = true;
    }
}

pub(crate) fn calibrate(path: &std::path::Path) -> serde_json::Value {
    ffmpeg_next::init().expect("FFmpeg");
    let mut input = ffmpeg_next::format::input(path).expect("calibration input");
    let stream = input
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .expect("video");
    let index = stream.index();
    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())
        .expect("calibration context")
        .decoder()
        .video()
        .expect("calibration decoder");
    let (mut count, mut d, mut shape) = (0, 0, serde_json::Value::Null);
    let mut receive = |decoder: &mut ffmpeg_next::decoder::Video| {
        let mut frame = Video::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            let measured = spans(&frame);
            d = d.max(union(measured.clone()));
            let current = serde_json::json!({"format": format!("{:?}", frame.format()),
                "width": frame.width(), "height": frame.height(),
                "planes": measured.iter().map(|[a,b,s]| [b-a, *s, (b-a)/s]).collect::<Vec<_>>()});
            if !shape.is_null() {
                assert_eq!(shape, current, "unregistered calibration shape");
            }
            shape = current;
            count += 1;
        }
    };
    for (stream, packet) in input.packets() {
        if stream.index() == index {
            decoder.send_packet(&packet).expect("calibration packet");
            receive(&mut decoder);
        }
    }
    decoder.send_eof().expect("calibration eof");
    receive(&mut decoder);
    assert!(count > 0 && d > 0);
    serde_json::json!({"media_sha256": crate::sha256::sha256_file(path).expect("fixture hash"),
        "ffmpeg": ffmpeg_next::util::version(), "frames": count, "shape": shape, "d_bytes": d,
        "B": 16, "reader_cap": 2, "method": "safe measured plane span union"})
}

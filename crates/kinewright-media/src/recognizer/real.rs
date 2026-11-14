//! Explicit real-model witnesses. They never discover a person's application data.
use super::*;
use crate::test_support::TempDirectory;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug)]
struct Stats {
    start: Instant,
    first_commit: Option<Duration>,
    longest_step: Duration,
}
impl Stats {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            first_commit: None,
            longest_step: Duration::ZERO,
        }
    }
}
struct Measured<'a> {
    model: &'a mut BuiltinRecognizer,
    stats: &'a mut Stats,
}
struct MeasuredSession<'a> {
    inner: Box<dyn Session + 'a>,
    stats: &'a mut Stats,
}
impl Recognizer for Measured<'_> {
    fn identity(&self) -> &Identity {
        self.model.identity()
    }
    fn supports_streaming(&self) -> bool {
        self.model.supports_streaming()
    }
    fn session(
        &mut self,
        origin: u64,
        cancel: ExportCancellation,
    ) -> Result<Box<dyn Session + '_>, Error> {
        Ok(Box::new(MeasuredSession {
            inner: self.model.session(origin, cancel)?,
            stats: self.stats,
        }))
    }
}
impl Session for MeasuredSession<'_> {
    fn feed(&mut self, samples: &[f32]) -> Result<(), Error> {
        self.inner.feed(samples)
    }
    fn commit_at(&mut self, position: u64) -> Result<(), Error> {
        self.inner.commit_at(position)
    }
    fn finish(&mut self) -> Result<(), Error> {
        self.inner.finish()
    }
    fn step(&mut self) -> Result<Option<Update>, Error> {
        let start = Instant::now();
        let result = self.inner.step();
        self.stats.longest_step = self.stats.longest_step.max(start.elapsed());
        if matches!(result,Ok(Some(Update::Committed{ref words,..})) if !words.is_empty())
            && self.stats.first_commit.is_none()
        {
            self.stats.first_commit = Some(self.stats.start.elapsed());
        }
        result
    }
}
fn model_path() -> PathBuf {
    PathBuf::from(
        std::env::var_os("KINEWRIGHT_ASR_MODEL_DIR")
            .expect("set KINEWRIGHT_ASR_MODEL_DIR to the directory holding the pinned EN GGUF"),
    )
    .join(ENGLISH_MODEL.files[0].name)
}
fn recognize(
    model: &mut dyn Recognizer,
    path: &Path,
    seconds: u64,
    commits: bool,
) -> Result<Recognition, Error> {
    crate::initialize_ffmpeg()?;
    let mut decoder = crate::audio::AudioDecoder::open(
        path,
        SAMPLE_RATE,
        1,
        0,
        seconds * u64::from(SAMPLE_RATE),
    )?;
    queue::drive(
        model,
        &ExportCancellation::default(),
        |window| {
            if let Some(chunk) = decoder.next_chunk()? {
                window.extend(chunk);
                Ok(true)
            } else {
                Ok(false)
            }
        },
        |_| {},
        commits,
    )
}
#[test]
#[ignore = "on-demand: pinned EN model; explicitly run in the recognition slow job"]
fn dq1_real_ami_short_keeps_um_and_known_words() {
    let receipts = TempDirectory::new("dq1-real-short");
    let mut stats = Stats::new();
    let mut model = BuiltinRecognizer::load(
        &model_path(),
        receipts.root(),
        &ExportCancellation::default(),
    )
    .unwrap();
    let result = recognize(
        &mut Measured {
            model: &mut model,
            stats: &mut stats,
        },
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ami-es2002a-90-102.wav"),
        12,
        true,
    )
    .unwrap();
    let words = result
        .words
        .iter()
        .map(|word| kinewright_core::normalize_transcript_word(&word.text))
        .collect::<Vec<_>>();
    for expected in ["okay", "so", "designing", "new", "remote", "control", "um"] {
        assert!(
            words.iter().any(|word| word == expected),
            "missing {expected}: {words:?}"
        );
    }
    // Build-independent acoustic tolerance: 0.8 s around these probe positions.
    for (text, expected) in [("remote", 4_320_000_u64), ("um", 5_760_000)] {
        assert!(
            result.words.iter().any(
                |word| kinewright_core::normalize_transcript_word(&word.text) == text
                    && word.start_us.abs_diff(expected) <= 800_000
            ),
            "{text}: {:?}",
            result.words
        );
    }
    assert_eq!(result.timing_source, TimingSource::TokenDurationsLagged);
    assert!(
        result
            .words
            .iter()
            .all(|word| word.start_us < word.end_us && word.end_us <= 12_000_000)
    );
    println!(
        "DQ1_SHORT words={} elapsed={:?} first_commit={:?} longest_step={:?}",
        words.len(),
        stats.start.elapsed(),
        stats.first_commit,
        stats.longest_step
    );
}
#[test]
#[ignore = "on-demand: pinned EN model and KINEWRIGHT_ASR_AMI8 fixture"]
fn dq1_real_ami8_whole_equals_pause_commits() {
    let path =
        PathBuf::from(std::env::var_os("KINEWRIGHT_ASR_AMI8").expect("set KINEWRIGHT_ASR_AMI8"));
    let receipts = TempDirectory::new("dq1-real-ami8");
    let mut stats = Stats::new();
    let mut model = BuiltinRecognizer::load(
        &model_path(),
        receipts.root(),
        &ExportCancellation::default(),
    )
    .unwrap();
    let pieces = recognize(
        &mut Measured {
            model: &mut model,
            stats: &mut stats,
        },
        &path,
        480,
        true,
    )
    .unwrap();
    let elapsed = stats.start.elapsed();
    let whole = recognize(&mut model, &path, 480, false).unwrap();
    assert_eq!(
        pieces, whole,
        "same-build words and exact source times across commits"
    );
    println!(
        "DQ1_AMI8 words={} elapsed={:?} first_commit={:?} longest_step={:?}",
        pieces.words.len(),
        elapsed,
        stats.first_commit,
        stats.longest_step
    );
}
#[test]
#[ignore = "on-demand: pinned EN model and KINEWRIGHT_ASR_AMI30; memory and cancellation measurement"]
fn dq1_real_ami30_bounded_memory_and_cancel() {
    let path =
        PathBuf::from(std::env::var_os("KINEWRIGHT_ASR_AMI30").expect("set KINEWRIGHT_ASR_AMI30"));
    let receipts = TempDirectory::new("dq1-real-ami30");
    let mut stats = Stats::new();
    let mut model = BuiltinRecognizer::load(
        &model_path(),
        receipts.root(),
        &ExportCancellation::default(),
    )
    .unwrap();
    let result = recognize(
        &mut Measured {
            model: &mut model,
            stats: &mut stats,
        },
        &path,
        1800,
        true,
    )
    .unwrap();
    println!(
        "DQ1_AMI30 words={} elapsed={:?} first_commit={:?} longest_step={:?}",
        result.words.len(),
        stats.start.elapsed(),
        stats.first_commit,
        stats.longest_step
    );
    let cancel = ExportCancellation::default();
    let mut session = model.session(0, cancel.clone()).unwrap();
    session.feed(&[0.0; FEED_SAMPLES]).unwrap();
    let _ = session.step().unwrap();
    cancel.cancel();
    let start = Instant::now();
    assert!(matches!(session.step(), Err(Error::Cancelled)));
    println!("DQ1_CANCEL between_steps={:?}", start.elapsed());
    drop(session);
    drop(model);
    #[cfg(target_os = "linux")]
    {
        let memory_report = std::fs::read_to_string("/proc/self/status").unwrap();
        println!(
            "DQ1_AFTER_DROP {}",
            memory_report
                .lines()
                .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert!(
            !std::fs::read_to_string("/proc/self/maps")
                .unwrap()
                .contains(ENGLISH_MODEL.files[0].name),
            "no weights mapped after model drop"
        );
    }
}

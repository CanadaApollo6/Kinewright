struct RecheckRefuser {
    value: Arc<BoundDialogue>,
    fail_at: usize,
    reads: std::sync::atomic::AtomicUsize,
    refusal: fn() -> Refusal,
}
impl DialogueMeasurements for RecheckRefuser {
    fn audio_stream(&self, _: &MediaIdentity) -> Result<u32, Refusal> {
        Ok(0)
    }
    fn get(&self, _: &MediaIdentity, _: u32) -> Result<Arc<BoundDialogue>, Refusal> {
        let read = self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if read == self.fail_at {
            Err((self.refusal)())
        } else {
            Ok(Arc::clone(&self.value))
        }
    }
}
fn recheck_acoustic_refusal() -> Refusal {
    Refusal::Acoustic {
        reason: "measurement still running; status=queued".into(),
        at_us: 1_234_567,
        quiet_us: [Some(120_000), None],
    }
}
fn recheck_request_refusal() -> Refusal {
    Refusal::Request {
        why: "measurement requested; status=queued".into(),
    }
}
fn recheck_service(
    fail_at: usize,
    refusal: fn() -> Refusal,
) -> (KinewrightMcp, Arc<BoundDialogue>, Arc<RecheckRefuser>) {
    let (s, v) = fixture();
    let provider = Arc::new(RecheckRefuser {
        value: Arc::clone(&v),
        fail_at,
        reads: std::sync::atomic::AtomicUsize::new(0),
        refusal,
    });
    // The test and dispatcher observe the same provider and its exact read count.
    let installed = Arc::clone(&provider);
    *s.dialogue_measurements.write().unwrap() = Some(installed);
    (s, v, provider)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecheckPhase {
    BeforePrompt,
    AfterConsent,
}
fn typed_recheck_refusal(phase: RecheckPhase) {
    let failures: [fn() -> Refusal; 2] = [recheck_acoustic_refusal, recheck_request_refusal];
    for failure in failures {
        let (first_service, first_value, _) = recheck_service(1, failure);
        let first = cut(&first_service, args(vec![words(&first_value, 1)]), false);
        assert_eq!(first.is_error, Some(true));
        assert_eq!(content(&first)["status"], "refused");
        assert_eq!(
            content(&first)["reason"],
            serde_json::to_value(failure()).unwrap()
        );
        let fail_at = match phase {
            RecheckPhase::BeforePrompt => 2,
            RecheckPhase::AfterConsent => 3,
        };
        let (s, v, provider) = recheck_service(fail_at, failure);
        let before = s.snapshot().unwrap();
        let history = s.core.export_history_identified().unwrap();
        thread::scope(|scope| {
            let worker = scope.spawn(|| invoke(&s, "cut_dialogue", args(vec![words(&v, 1)])));
            let until = Instant::now() + Duration::from_secs(60);
            let mut prompted = false;
            while !worker.is_finished() {
                for request in s.confirmations.pending_requests() {
                    prompted = true;
                    if phase == RecheckPhase::AfterConsent {
                        assert!(s.confirmations.approve(request.id));
                    } else {
                        s.confirmations
                            .reject(request.id, "pre-prompt refusal must not ask");
                    }
                }
                assert!(Instant::now() < until, "DQ_TOOLS_TYPED_RECHECK_BOUNDED");
                thread::yield_now();
            }
            let result = worker.join().unwrap();
            assert_eq!(
                prompted,
                phase == RecheckPhase::AfterConsent,
                "DQ_TOOLS_TYPED_RECHECK_PROMPT_ORDER"
            );
            assert_refused_unchanged(
                &s,
                &before,
                &history,
                &result,
                "DQ_TOOLS_TYPED_RECHECK_ATOMIC",
            );
            println!("DQ_TYPED_RECHECK {phase:?} {}", content(&result));
            assert_eq!(
                content(&result)["reason"],
                content(&first)["reason"],
                "DQ_TOOLS_TYPED_RECHECK_REFUSAL"
            );
            assert_eq!(
                serde_json::to_value(&result).unwrap(),
                serde_json::to_value(&first).unwrap(),
                "DQ_TOOLS_TYPED_RECHECK_REPLY"
            );
            assert_eq!(
                provider.reads.load(std::sync::atomic::Ordering::SeqCst),
                fail_at
            );
        });
    }
}
#[test]
fn dq_tools_item13_typed_refusal_before_prompt() {
    typed_recheck_refusal(RecheckPhase::BeforePrompt);
}
#[test]
fn dq_tools_item13_typed_refusal_after_consent() {
    typed_recheck_refusal(RecheckPhase::AfterConsent);
}

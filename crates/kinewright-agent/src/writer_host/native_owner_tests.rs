struct LiveNative {
    execution: ExecutionIdentity,
    session: Box<dyn kinewright_core::AgentSession>,
    claim: Arc<Mutex<crate::execution_claim::MachineClaim>>,
    scratch: std::path::PathBuf,
    control: crate::guardian::StartupControl,
    _unit: NativeUnitCleanup,
}
impl LiveNative {
    fn start(f: &Fixture) -> Self {
        let (run, execution, _) = reserve(f);
        let unit = NativeUnitCleanup::new(&execution);
        let admitted = native_admitted(f, &execution);
        let (profile, checks, scratch) = native_profile(f, &admitted);
        let invocation = native_charge(f, &admitted, profile);
        let (session, claim) = native_start(f, admitted, invocation, checks, false);
        let session = session.unwrap();
        wait_native(&scratch.join("execution-ready"));
        let control = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .hooks
            .native
            .get(&(run, execution.attempt.clone()))
            .unwrap()
            .control
            .clone();
        Self {
            execution,
            session,
            claim,
            scratch,
            control,
            _unit: unit,
        }
    }
    fn retired(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.claim.lock().unwrap().retained_attempts() != 0 {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }
    fn finish(mut self) {
        // Also cleans a mutated failure before any assertion ends the test.
        self.session.interrupt();
        drop(self.session);
        finish_stopped_native(self.claim);
        std::fs::remove_dir_all(self.scratch).unwrap();
    }
}
fn native_save_as(f: &Fixture, name: &str) -> HostReply {
    let snapshot = {
        let owner = f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        HostSnapshot::capture(host.core()).unwrap()
    };
    f.editor
        .request(HostCommand::SaveAs {
            destination: f.temp.path(name),
            snapshot,
        })
        .unwrap()
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_fix2_save_as_revokes_executing_native() {
    let f = Fixture::new();
    let live = LiveNative::start(&f);
    let before = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .snapshot_read_owner();
    let reply = native_save_as(&f, "native-owner-copy.kinewright");
    let after = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .snapshot_read_owner();
    let closed = live.control.closed();
    let retired = live.retired();
    live.finish();
    assert!(reply.error.is_none(), "S4B_C2_FIX2_SAVE_AS_ACCEPTED");
    assert_ne!(before, after, "S4B_C2_FIX2_SAVE_AS_CHANGED_OWNER");
    assert!(closed, "S4B_C2_FIX2_EXECUTING_OWNER_REVOKED");
    assert!(retired, "S4B_C2_FIX2_EXECUTING_CLEANUP_PROVEN");
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_fix2_save_as_before_commit_revokes_lifetime() {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Stage {
        Waiting,
        Registered,
        Resume,
    }
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let _unit = NativeUnitCleanup::new(&execution);
    let admitted = native_admitted(&f, &execution);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    let invocation = native_charge(&f, &admitted, profile);
    let handoff = Arc::new((Mutex::new(Stage::Waiting), std::sync::Condvar::new()));
    let gate = handoff.clone();
    let barrier = Arc::new(move || {
        let (lock, changed) = &*gate;
        let mut stage = lock.lock().unwrap();
        *stage = Stage::Registered;
        changed.notify_all();
        let (stage, _) = changed
            .wait_timeout_while(stage, Duration::from_secs(10), |stage| {
                *stage != Stage::Resume
            })
            .unwrap();
        assert!(*stage == Stage::Resume, "native startup resume deadline");
    });
    let (result, claim, closed, retained, accepted) = std::thread::scope(|scope| {
        let f = &f;
        let launch = scope.spawn(move || {
            native_start_barrier(f, admitted, invocation, checks, false, Some(barrier))
        });
        let (lock, changed) = &*handoff;
        let (stage, _) = changed
            .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(10), |stage| {
                *stage != Stage::Registered
            })
            .unwrap();
        assert!(*stage == Stage::Registered, "native registration deadline");
        drop(stage);
        let (control, machine) = {
            let host = f.host.owner.upgrade().unwrap();
            let host = host.lock().unwrap();
            let control = host
                .hooks
                .native
                .get(&(run.clone(), execution.attempt.clone()))
                .unwrap()
                .control
                .clone();
            (control, f.temp.path("native-machine"))
        };
        let reply = native_save_as(f, "startup-owner-copy.kinewright");
        let closed = control.closed();
        // Cleanup cannot run while the real launch is paused in authorization.
        let retained = machine
            .join(format!("attempt-{}.json", execution.nonce.as_str()))
            .exists();
        *lock.lock().unwrap() = Stage::Resume;
        changed.notify_all();
        let (result, claim) = launch.join().unwrap();
        (result, claim, closed, retained, reply.error.is_none())
    });
    let refused = result.is_err();
    if let Ok(mut session) = result {
        wait_native(&scratch.join("execution-ready"));
        session.interrupt();
        drop(session);
    }
    finish_stopped_native(claim);
    let executed = scratch.join("executions").exists();
    std::fs::remove_dir_all(scratch).unwrap();
    assert!(accepted, "S4B_C2_FIX2_STARTUP_SAVE_AS_ACCEPTED");
    assert!(closed, "S4B_C2_FIX2_STARTUP_OWNER_REVOKED");
    assert!(retained, "S4B_C2_FIX2_SLOT_RETAINED_UNTIL_PROOF");
    assert!(
        refused && !executed,
        "S4B_C2_FIX2_OWNER_CHANGE_ZERO_EXECUTION"
    );
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_fix2_detach_retains_native_owner() {
    let f = Fixture::new();
    let live = LiveNative::start(&f);
    let before = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .snapshot_read_owner();
    let refused = f.editor.request(HostCommand::Shutdown).unwrap();
    let refused_live = !live.control.closed();
    let reply = f
        .editor
        .request(HostCommand::DetachForContinuation)
        .unwrap();
    let after = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .snapshot_read_owner();
    let closed = live.control.closed();
    let retained = live.claim.lock().unwrap().retained_attempts();
    let recovery: crate::job_guardian::protocol::RecoveryIdentity = serde_json::from_slice(
        &std::fs::read(f.temp.path(&format!(
            "native-machine/native-{}.json",
            live.execution.nonce.as_str()
        )))
        .unwrap(),
    )
    .unwrap();
    let members = kinewright_job_sys::Cgroup::open(std::path::Path::new(&recovery.containment))
        .unwrap()
        .members()
        .unwrap();
    let detached = reply
        .detached
        .as_ref()
        .is_some_and(|detached| detached.retained_runs.contains(&live.execution.run));
    live.finish();
    assert_eq!(refused.error.unwrap().code, "continuation_required");
    assert!(refused_live, "S4B_C2_FIX2_REFUSED_SHUTDOWN_RETAINS_OWNER");
    assert!(reply.error.is_none() && detached);
    assert_eq!(before, after, "S4B_C2_FIX2_DETACH_SAME_OWNER");
    assert!(
        !closed && retained == 1 && !members.is_empty(),
        "S4B_C2_FIX2_DETACH_NATIVE_ALIVE"
    );
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_fix2_host_drop_closes_native() {
    let f = Fixture::new();
    let live = LiveNative::start(&f);
    let weak = f.host.owner.clone();
    drop(f.host);
    let closed = live.control.closed();
    let retired = live.retired();
    let owner_gone = weak.upgrade().is_none();
    live.finish();
    assert!(owner_gone, "S4B_C2_FIX2_HOST_OWNER_DROPPED");
    assert!(closed && retired, "S4B_C2_FIX2_HOST_DROP_CLEANUP");
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_fix2_owner_scope_checks_project_epoch_and_loss() {
    for mode in 0..3 {
        let f = Fixture::new();
        let live = LiveNative::start(&f);
        let reply = {
            let owner = f.host.owner.upgrade().unwrap();
            let mut host = owner.lock().unwrap();
            if mode < 2 {
                let registered = host
                    .hooks
                    .native
                    .get_mut(&(live.execution.run.clone(), live.execution.attempt.clone()))
                    .unwrap();
                if mode == 0 {
                    registered.identity.project = identity();
                } else {
                    registered.owner_epoch = registered.owner_epoch.wrapping_add(1);
                }
                host.execute(HostCommand::Status, f.host.roots.clone())
            } else {
                let snapshot = HostSnapshot::capture(host.core()).unwrap();
                // Model an owner lost by a partially completed transition. The
                // failed command must revoke even with before == after == None.
                host.owner.take();
                host.execute(HostCommand::Save { snapshot }, f.host.roots.clone())
            }
        };
        let closed = live.control.closed();
        let retired = live.retired();
        live.finish();
        assert_eq!(reply.error.is_some(), mode == 2);
        match mode {
            0 => assert!(closed && retired, "S4B_C2_FIX2_PROJECT_SCOPE_REVOKED"),
            1 => assert!(closed && retired, "S4B_C2_FIX2_EPOCH_SCOPE_REVOKED"),
            _ => assert!(closed && retired, "S4B_C2_FIX2_UNAVAILABLE_OWNER_REVOKED"),
        }
    }
}

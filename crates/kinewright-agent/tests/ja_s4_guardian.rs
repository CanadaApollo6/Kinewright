#![cfg(feature = "eval-harness")]
#[cfg(target_os = "linux")]
use kinewright_agent::guardian::RecoveryObserver;
use kinewright_agent::{
    execution_claim::MachineClaim,
    guardian::{ContainedSession, DurableAttemptOwner},
    job_guardian::protocol::{self, AttemptIdentity, Program, RecoveryIdentity},
};
#[cfg(target_os = "linux")]
use kinewright_agent::{
    guardian::spawn_transport,
    job_guardian::protocol::{GuardianMessage, HostMessage},
};
use kinewright_job_sys::ProcessHandle;
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
fn fixture() -> &'static Path {
    FIXTURE.get_or_init(|| {
        if let Some(binary) = std::env::var_os("KINEWRIGHT_S4_PREBUILT_NATIVE") {
            let binary = PathBuf::from(binary);
            assert!(
                binary.is_absolute() && binary.is_file(),
                "prebuilt native fixture prerequisite"
            );
            return binary;
        }
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        fs::create_dir_all(&target).unwrap();
        let binary = target.join(format!(
            "c2-native-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let status = Command::new("rustc")
            .args(["--edition", "2024"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja_s4_native.rs"))
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap();
        assert!(status.success(), "native fixture compiler prerequisite");
        binary
    })
}
fn guardian() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_kinewright-job-guardian"))
}
fn root(label: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "kinewright-c2-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
fn identity() -> AttemptIdentity {
    use std::fmt::Write as _;
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).unwrap();
    let mut rendered = String::with_capacity(32);
    for byte in nonce {
        write!(&mut rendered, "{byte:02x}").unwrap();
    }
    let nonce = rendered;
    AttemptIdentity {
        project: "a1".into(),
        run: "b2".into(),
        task: "c3".into(),
        attempt: nonce.clone(),
        nonce,
        owner_instance: "1234567890abcdef1234567890abcdef".into(),
    }
}
fn program(root: &Path) -> Program {
    Program {
        executable: fixture().into(),
        arguments: vec![root.to_string_lossy().into_owned(), "payload".into()],
    }
}
struct Owner {
    root: PathBuf,
    refuse: bool,
    reservations: usize,
    commits: usize,
}
impl Owner {
    fn new(root: &Path) -> Self {
        Self {
            root: root.into(),
            refuse: false,
            reservations: 0,
            commits: 0,
        }
    }
}
impl DurableAttemptOwner for Owner {
    fn reserve(&mut self, id: &AttemptIdentity) -> io::Result<()> {
        fs::write(
            self.root.join("fixture-reservation.json"),
            serde_json::to_vec(id)?,
        )?;
        #[cfg(windows)]
        File::options()
            .write(true)
            .open(self.root.join("fixture-reservation.json"))?
            .sync_all()?;
        #[cfg(not(windows))]
        File::open(self.root.join("fixture-reservation.json"))?.sync_all()?;
        self.reservations += 1;
        Ok(())
    }
    fn authorize_commit(&mut self, recovery: &RecoveryIdentity) -> io::Result<()> {
        if self.refuse {
            return Err(io::Error::other("durable owner refused COMMIT"));
        }
        fs::write(
            self.root.join("fixture-committed.json"),
            serde_json::to_vec(recovery)?,
        )?;
        #[cfg(windows)]
        File::options()
            .write(true)
            .open(self.root.join("fixture-committed.json"))?
            .sync_all()?;
        #[cfg(not(windows))]
        File::open(self.root.join("fixture-committed.json"))?.sync_all()?;
        self.commits += 1;
        Ok(())
    }
}
fn wait_file(path: &Path) {
    let deadline = Instant::now() + protocol::STARTUP;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "native fixture signal {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(1));
    }
}
fn fixture_pid(root: &Path, role: &str) -> u32 {
    let path = root.join(format!("{role}.json"));
    wait_file(&path);
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    u32::try_from(value["pid"].as_u64().unwrap()).unwrap()
}
fn pin_descendants(session: &mut ContainedSession, root: &Path) -> Vec<ProcessHandle> {
    ["payload", "grandchild"]
        .iter()
        .map(|role| {
            let process = ProcessHandle::open(fixture_pid(root, role)).unwrap();
            assert!(
                !process.reaped().unwrap(),
                "fixture must be independently live before fault"
            );
            session
                .observe_descendant(process.pid(), process.start())
                .unwrap();
            process
        })
        .collect()
}
fn assert_retired(claim: &mut MachineClaim, proof: &kinewright_agent::guardian::CleanupProof) {
    assert_eq!(claim.retained_attempts(), 1);
    claim.retire(proof).unwrap();
    assert_eq!(claim.retained_attempts(), 0);
    assert!(
        claim.retire(proof).is_err(),
        "retirement must happen exactly once"
    );
}

#[test]
fn s4_precommit_cannot_execute() {
    let root = root("precommit");
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session =
        ContainedSession::prepare(&mut claim, &mut owner, id, program(&root), guardian()).unwrap();
    let payload = ProcessHandle::open(session.payload_identity().0).unwrap();
    assert!(
        !payload.reaped().unwrap(),
        "a contained native process must exist before COMMIT"
    );
    thread::sleep(Duration::from_millis(100));
    assert!(
        !root.join("payload.json").exists(),
        "zero provider execution before durable COMMIT"
    );
    owner.refuse = true;
    assert!(session.commit(&mut owner).is_err());
    assert!(!root.join("payload.json").exists());
    assert_eq!(owner.reservations, 1);
    assert_eq!(owner.commits, 0);
    assert!(MachineClaim::acquire_fixture(&root.join("machine"), "d4", "e5").is_err());
    let proof = session.cleanup().unwrap();
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn s4_cancel_tree_and_retire_once() {
    let root = root("cancel");
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session =
        ContainedSession::prepare(&mut claim, &mut owner, id, program(&root), guardian()).unwrap();
    session.commit(&mut owner).unwrap();
    let descendants = pin_descendants(&mut session, &root);
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{}/stat", descendants[1].pid())).unwrap();
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert_eq!(
            fields[3].parse::<u32>().unwrap(),
            descendants[1].pid(),
            "grandchild must call setsid"
        );
    }
    assert!(MachineClaim::acquire_fixture(&root.join("machine"), "d4", "e5").is_err());
    let started = Instant::now();
    session.request_cancel().unwrap();
    let proof = session.cleanup().unwrap();
    assert!(!proof.guardian_lost());
    println!(
        "C2 phase_evidence={}",
        serde_json::json!({"recovery":session.recovery(), "startup_us":session.phase_timestamps_us(), "cleanup_us":session.cleanup_timestamps_us()})
    );
    assert!(
        started.elapsed() >= Duration::from_millis(490),
        "cancel must retain the 500 ms grace"
    );
    assert!(started.elapsed() <= protocol::GRACE + protocol::CLEANUP);
    for process in descendants {
        assert!(
            process.reaped().unwrap(),
            "no live or zombie fixture identity"
        );
    }
    assert_retired(&mut claim, &proof);
    println!(
        "C2 cancel kill-to-empty/reap bounded; elapsed_us={}",
        started.elapsed().as_micros()
    );
    claim.finish().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn s4_guardian_death_and_observation_loss() {
    let root = root("guardian-death");
    let id = identity();
    let machine = root.join("machine");
    let mut claim = MachineClaim::acquire_fixture(&machine, &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session =
        ContainedSession::prepare(&mut claim, &mut owner, id, program(&root), guardian()).unwrap();
    session.commit(&mut owner).unwrap();
    let descendants = pin_descendants(&mut session, &root);
    let mut observer = RecoveryObserver::open(session.recovery()).unwrap();
    let guardian = ProcessHandle::open(session.recovery().guardian_pid).unwrap();
    guardian.kill().unwrap();
    // Deliberately lose the host's ordinary pipe/cleanup observer and OS lock.
    drop(session);
    drop(claim);
    assert!(
        MachineClaim::acquire_fixture(&machine, "d4", "e5").is_err(),
        "free lock/EOF is not emptiness"
    );
    let mut recovered = MachineClaim::recover_fixture(&machine).unwrap();
    assert_eq!(
        recovered.retained_attempts(),
        1,
        "missing observation retains the actual reservation"
    );
    assert!(
        recovered.reserve(&identity()).is_err(),
        "recovery cannot overlap a replacement"
    );
    let outcome = observer.wait_for_cleanup();
    if outcome.is_err() {
        let _ = observer.stop_tree();
    }
    let proof = outcome.unwrap();
    assert!(
        proof.guardian_lost(),
        "guardian loss permanently disqualifies unsaved output"
    );
    for process in descendants {
        assert!(process.reaped().unwrap());
    }
    assert_retired(&mut recovered, &proof);
    recovered.finish().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn s4_real_host_death_skips_grace() {
    let root = root("host-death");
    let mut host = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "s4_native_host_fixture", "--nocapture"])
        .env("KINEWRIGHT_S4_HOST_FIXTURE", &root)
        .env("KINEWRIGHT_S4_NATIVE", fixture())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_file(&root.join("host-ready.json"));
    let recovery: RecoveryIdentity =
        serde_json::from_slice(&fs::read(root.join("host-ready.json")).unwrap()).unwrap();
    let mut observer = RecoveryObserver::open(&recovery).unwrap();
    let child = ProcessHandle::open(fixture_pid(&root, "payload")).unwrap();
    let grandchild = ProcessHandle::open(fixture_pid(&root, "grandchild")).unwrap();
    assert!(!child.reaped().unwrap() && !grandchild.reaped().unwrap());
    let started = Instant::now();
    host.kill().unwrap();
    host.wait().unwrap();
    let outcome = observer.wait_for_cleanup();
    if outcome.is_err() {
        let _ = observer.stop_tree();
    }
    let proof = outcome.unwrap();
    assert!(child.reaped().unwrap() && grandchild.reaped().unwrap());
    assert!(started.elapsed() < protocol::CLEANUP);
    let mut recovered = MachineClaim::recover_fixture(&root.join("machine")).unwrap();
    assert_retired(&mut recovered, &proof);
    recovered.finish().unwrap();
    println!(
        "C2 real host death empty/reaped_us={}",
        started.elapsed().as_micros()
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn s4_native_host_fixture() {
    let Some(root) = std::env::var_os("KINEWRIGHT_S4_HOST_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let program = Program {
        executable: std::env::var_os("KINEWRIGHT_S4_NATIVE").unwrap().into(),
        arguments: vec![root.to_string_lossy().into_owned(), "payload".into()],
    };
    let mut session =
        ContainedSession::prepare(&mut claim, &mut owner, id, program, guardian()).unwrap();
    session.commit(&mut owner).unwrap();
    pin_descendants(&mut session, &root);
    // An unrelated native fixture catches accidental lifetime-writer inheritance.
    let mut writer_holder = Command::new(std::env::var_os("KINEWRIGHT_S4_NATIVE").unwrap())
        .arg(&root)
        .arg("writer-holder")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    fs::write(
        root.join("host-ready.partial"),
        serde_json::to_vec(session.recovery()).unwrap(),
    )
    .unwrap();
    fs::rename(
        root.join("host-ready.partial"),
        root.join("host-ready.json"),
    )
    .unwrap();
    let _ = &mut writer_holder;
    loop {
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn s4_lost_startup_acknowledgments() {
    for stage in ["owned", "contained", "executing"] {
        let root = root(stage);
        let id = identity();
        let mut claim =
            MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
        claim.reserve(&id).unwrap();
        let private = root.join("native");
        fs::create_dir(&private).unwrap();
        let (mut launcher, mut writer, replies) =
            spawn_transport(guardian(), &private, &id).unwrap();
        protocol::write_frame(
            &mut writer,
            &HostMessage::Prepare {
                identity: id.clone(),
                program: program(&root),
            },
        )
        .unwrap();
        let deadline = Instant::now() + protocol::STARTUP;
        let next = |deadline| loop {
            match replies.try_recv() {
                Ok(Ok(message)) => break message,
                Ok(Err(error)) => panic!("native transport failed: {error}"),
                Err(_) => {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(1));
                }
            }
        };
        let GuardianMessage::Owned { recovery, .. } = next(deadline) else {
            panic!("OWNED");
        };
        claim.bind(&recovery).unwrap();
        let mut observer = RecoveryObserver::open(&recovery).unwrap();
        if stage != "owned" {
            protocol::write_frame(
                &mut writer,
                &HostMessage::Contain {
                    nonce: id.nonce.clone(),
                },
            )
            .unwrap();
            let GuardianMessage::Contained { pid, start, .. } =
                next(Instant::now() + protocol::STARTUP)
            else {
                panic!("CONTAINED");
            };
            let process = ProcessHandle::open(pid).unwrap();
            assert_eq!(process.start(), start);
            assert!(!process.reaped().unwrap());
            assert!(!root.join("payload.json").exists(), "pre-COMMIT execution");
        }
        if stage == "executing" {
            protocol::write_frame(
                &mut writer,
                &HostMessage::Commit {
                    nonce: id.nonce.clone(),
                },
            )
            .unwrap();
            wait_file(&root.join("grandchild.json"));
        }
        // The host loses this ACK and abandons startup. No alternate launcher runs.
        drop(writer);
        let outcome = observer.wait_for_cleanup();
        if outcome.is_err() {
            let _ = observer.stop_tree();
        }
        let proof = outcome.unwrap();
        if stage != "executing" {
            assert!(!root.join("payload.json").exists());
        }
        assert_retired(&mut claim, &proof);
        claim.finish().unwrap();
        launcher.wait().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn s4_machine_claim_stays_scoped_and_bounded() {
    let root = root("claim");
    let id = identity();
    let mut claim = MachineClaim::acquire_fixture(&root, &id.project, &id.run).unwrap();
    for _ in 0..3 {
        claim.reserve(&identity()).unwrap();
    }
    assert!(claim.reserve(&identity()).is_err());
    assert!(
        claim
            .reserve(&AttemptIdentity {
                project: "d4".into(),
                ..identity()
            })
            .is_err()
    );
    assert!(MachineClaim::acquire_fixture(&root, "d4", "e5").is_err());
    drop(claim);
    assert!(MachineClaim::acquire_fixture(&root, "d4", "e5").is_err());
    let recovered = MachineClaim::recover_fixture(&root).unwrap();
    assert_eq!(recovered.retained_attempts(), 3);
    assert!(recovered.finish().is_err());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn s4_retained_inode_deletion_signal() {
    let root = root("kernel-signal");
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session =
        ContainedSession::prepare(&mut claim, &mut owner, id, program(&root), guardian()).unwrap();
    let leaf =
        kinewright_job_sys::Cgroup::open(Path::new(&session.recovery().containment)).unwrap();
    assert!(
        leaf.try_remove().is_err(),
        "kernel must refuse rmdir of a real nonempty leaf"
    );
    let empty = leaf.create_child("native-signal").unwrap();
    let live_links = match empty.link_count() {
        Ok(links) if links > 0 => links,
        observed => {
            skip_kernel_signal(&format!("live fstat signal unavailable: {observed:?}"));
            session.close_lifetime();
            return;
        }
    };
    assert!(
        !match empty.kernel_deleted() {
            Ok(deleted) => deleted,
            Err(error) => {
                skip_kernel_signal(&format!("live inode verification unavailable: {error}"));
                session.close_lifetime();
                return;
            }
        },
        "a live retained inode must not prove deletion"
    );
    empty.try_remove().unwrap();
    let deleted_links = empty.link_count();
    if live_links == 0 || !deleted_links.as_ref().is_ok_and(|links| *links == 0) {
        let reason = format!("live_nlink={live_links}, deleted_nlink={deleted_links:?}");
        skip_kernel_signal(&reason);
        session.close_lifetime();
        return;
    }
    assert!(empty.kernel_deleted().unwrap());
    let kernel = Command::new("uname").arg("-r").output().unwrap();
    println!(
        "C2 kernel={} live_nlink={live_links} removed_nlink={} nonempty_rmdir=refused",
        String::from_utf8_lossy(&kernel.stdout).trim(),
        deleted_links.unwrap()
    );
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::explicit_write,
    reason = "bypass libtest capture so unsupported kernel signals are visible in CI"
)]
fn skip_kernel_signal(reason: &str) {
    use std::io::Write as _;
    let kernel = Command::new("uname").arg("-r").output().unwrap();
    let reason = format!(
        "kernel={} retained-inode-signal {reason}",
        String::from_utf8_lossy(&kernel.stdout).trim()
    );
    writeln!(std::io::stderr(), "C2_NATIVE_NOT_RUN {reason}").unwrap();
    fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("c2-kernel-signal-not-run.txt"),
        reason,
    )
    .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn s4_reaped_guardian_and_identity_are_required() {
    let root = root("reap-identity");
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    claim.reserve(&id).unwrap();
    let private = root.join("native");
    fs::create_dir(&private).unwrap();
    let (mut launcher, mut writer, replies) = spawn_transport(guardian(), &private, &id).unwrap();
    protocol::write_frame(
        &mut writer,
        &HostMessage::Prepare {
            identity: id.clone(),
            program: program(&root),
        },
    )
    .unwrap();
    let deadline = Instant::now() + protocol::STARTUP;
    let recovery = loop {
        if let Ok(Ok(GuardianMessage::Owned { recovery, .. })) = replies.try_recv() {
            break recovery;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    };
    claim.bind(&recovery).unwrap();
    let mut wrong = recovery.clone();
    wrong.inode ^= 1;
    assert!(
        RecoveryObserver::open(&wrong).is_err(),
        "foreign retained inode cannot prove this attempt empty"
    );
    let mut observer = RecoveryObserver::open(&recovery).unwrap();
    assert!(
        observer.wait_for_cleanup().is_err(),
        "an empty leaf cannot retire a still-live guardian"
    );
    drop(writer);
    let proof = observer.wait_for_cleanup().unwrap();
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    launcher.wait().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn s4_deleted_cgroup_zombies_still_block_retirement() {
    let root = root("zombie");
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut native = program(&root);
    native.arguments[1] = "zombie-parent".into();
    let mut session =
        ContainedSession::prepare(&mut claim, &mut owner, id, native, guardian()).unwrap();
    let leaf =
        kinewright_job_sys::Cgroup::open(Path::new(&session.recovery().containment)).unwrap();
    let mut zombie_leaf = leaf.create_child("zombie-witness").unwrap();
    fs::write(
        root.join("zombie-target"),
        zombie_leaf.path().as_os_str().as_encoded_bytes(),
    )
    .unwrap();
    session.commit(&mut owner).unwrap();
    let pid = fixture_pid(&root, "zombie-child");
    let process = ProcessHandle::open(pid).unwrap();
    let deadline = Instant::now() + protocol::STARTUP;
    loop {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        if stat.rsplit_once(')').unwrap().1.split_whitespace().next() == Some("Z") {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        zombie_leaf.empty().unwrap(),
        "events deliberately excludes this real zombie"
    );
    zombie_leaf.try_remove().unwrap();
    assert!(zombie_leaf.kernel_deleted().unwrap());
    let scan_deadline = Instant::now() + protocol::CLEANUP;
    loop {
        if let Ok(empty) = zombie_leaf.no_processes() {
            assert!(
                !empty,
                "deleted cgroup's zombie must block independent retirement"
            );
            break;
        }
        assert!(
            Instant::now() < scan_deadline,
            "complete native process scan unavailable"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!process.reaped().unwrap());
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    assert!(process.reaped().unwrap());
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn s4_guardian_subreaps_real_orphans() {
    let root = root("subreaper");
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut native = program(&root);
    native.arguments[1] = "orphan-payload".into();
    let mut session =
        ContainedSession::prepare(&mut claim, &mut owner, id, native, guardian()).unwrap();
    session.commit(&mut owner).unwrap();
    wait_file(&root.join("adopted.json"));
    let adopted: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("adopted.json")).unwrap()).unwrap();
    assert_eq!(
        adopted["parent"].as_u64(),
        Some(u64::from(session.recovery().guardian_pid)),
        "real orphan must be adopted by the native guardian subreaper"
    );
    let orphan =
        ProcessHandle::open(u32::try_from(adopted["pid"].as_u64().unwrap()).unwrap()).unwrap();
    session
        .observe_descendant(orphan.pid(), orphan.start())
        .unwrap();
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    assert!(orphan.reaped().unwrap());
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn s4_startup_deadline_kills_a_contained_shim() {
    let root = root("startup-deadline");
    let id = identity();
    let mut claim =
        MachineClaim::acquire_fixture(&root.join("machine"), &id.project, &id.run).unwrap();
    claim.reserve(&id).unwrap();
    let private = root.join("native");
    fs::create_dir(&private).unwrap();
    let (mut launcher, mut writer, replies) = spawn_transport(guardian(), &private, &id).unwrap();
    protocol::write_frame(
        &mut writer,
        &HostMessage::Prepare {
            identity: id.clone(),
            program: program(&root),
        },
    )
    .unwrap();
    let receive = || {
        let deadline = Instant::now() + protocol::STARTUP;
        loop {
            if let Ok(Ok(message)) = replies.try_recv() {
                break message;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    };
    let GuardianMessage::Owned { recovery, .. } = receive() else {
        panic!("OWNED");
    };
    claim.bind(&recovery).unwrap();
    let mut observer = RecoveryObserver::open(&recovery).unwrap();
    protocol::write_frame(
        &mut writer,
        &HostMessage::Contain {
            nonce: id.nonce.clone(),
        },
    )
    .unwrap();
    let GuardianMessage::Contained { pid, .. } = receive() else {
        panic!("CONTAINED");
    };
    let child = ProcessHandle::open(pid).unwrap();
    assert!(!child.reaped().unwrap());
    let started = Instant::now();
    thread::sleep(Duration::from_millis(4800));
    let proof = observer.wait_for_cleanup().unwrap();
    assert!(started.elapsed() <= protocol::STARTUP + protocol::CLEANUP);
    assert!(
        !root.join("payload.json").exists(),
        "expired startup cannot release provider code"
    );
    assert!(child.reaped().unwrap());
    println!(
        "C2 missing COMMIT startup-to-empty/reaped_us={}",
        started.elapsed().as_micros()
    );
    assert_retired(&mut claim, &proof);
    claim.finish().unwrap();
    drop(writer);
    launcher.wait().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn s4_windows_job_membership_and_last_handle_closure() {
    let root = root("windows-job");
    let mut job = kinewright_job_sys::Job::create().unwrap();
    job.spawn_suspended(fixture(), &program(&root).arguments)
        .unwrap();
    job.verify().unwrap();
    assert_eq!(job.members().unwrap().len(), 1);
    assert!(!root.join("payload.json").exists());
    job.resume().unwrap();
    wait_file(&root.join("grandchild.json"));
    let members = job.members().unwrap();
    assert_eq!(members.len(), 2);
    for role in ["payload", "grandchild"] {
        assert!(members.iter().any(|p| p.pid() == fixture_pid(&root, role)));
    }
    drop(job);
    let deadline = Instant::now() + protocol::CLEANUP;
    while !members.iter().all(|p| p.reaped().unwrap()) {
        assert!(
            Instant::now() < deadline,
            "last Job-handle close must kill every member"
        );
        thread::sleep(Duration::from_millis(1));
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn s4_windows_guardian_death_retains_cleanup_reservation() {
    let root = root("windows-guardian-death");
    let id = identity();
    let machine = root.join("machine");
    let mut claim = MachineClaim::acquire_fixture(&machine, &id.project, &id.run).unwrap();
    let mut owner = Owner::new(&root);
    let mut session =
        ContainedSession::prepare(&mut claim, &mut owner, id, program(&root), guardian()).unwrap();
    session.commit(&mut owner).unwrap();
    let descendants = pin_descendants(&mut session, &root);
    let guardian = ProcessHandle::open(session.recovery().guardian_pid).unwrap();
    guardian.kill().unwrap();
    let deadline = Instant::now() + protocol::CLEANUP;
    while !guardian.reaped().unwrap() || !descendants.iter().all(|p| p.reaped().unwrap()) {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        session.cleanup().is_err(),
        "lost Job observation cannot release a reservation"
    );
    assert_eq!(claim.retained_attempts(), 1);
    assert!(MachineClaim::acquire_fixture(&machine, "d4", "e5").is_err());
    drop(session);
    drop(claim);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn s4_windows_real_host_death_kills_the_job() {
    let root = root("windows-host-death");
    let mut host = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "s4_native_host_fixture", "--nocapture"])
        .env("KINEWRIGHT_S4_HOST_FIXTURE", &root)
        .env("KINEWRIGHT_S4_NATIVE", fixture())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_file(&root.join("host-ready.json"));
    let recovery: RecoveryIdentity =
        serde_json::from_slice(&fs::read(root.join("host-ready.json")).unwrap()).unwrap();
    let guardian = ProcessHandle::open(recovery.guardian_pid).unwrap();
    assert_eq!(guardian.start(), recovery.guardian_start);
    let child = ProcessHandle::open(fixture_pid(&root, "payload")).unwrap();
    let grandchild = ProcessHandle::open(fixture_pid(&root, "grandchild")).unwrap();
    let processes = [guardian, child, grandchild];
    assert!(processes.iter().all(|p| !p.reaped().unwrap()));
    host.kill().unwrap();
    host.wait().unwrap();
    let deadline = Instant::now() + protocol::CLEANUP;
    while !processes.iter().all(|p| p.reaped().unwrap()) {
        assert!(
            Instant::now() < deadline,
            "host death must empty the real Windows fixture Job"
        );
        thread::sleep(Duration::from_millis(1));
    }
    let machine = root.join("machine");
    assert!(MachineClaim::acquire_fixture(&machine, "d4", "e5").is_err());
    let recovered = MachineClaim::recover_fixture(&machine).unwrap();
    assert_eq!(
        recovered.retained_attempts(),
        1,
        "process exit alone cannot retire the claim"
    );
    drop(recovered);
    fs::remove_dir_all(root).unwrap();
}

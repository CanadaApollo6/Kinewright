//! Real byte boundaries and existing physical-ledger admission, not H1 acceptance.
#![allow(dead_code, reason = "the B-owned sources include pending owner ports")]

#[path = "../src/orchestration/limits.rs"]
mod limits;
use kinewright_agent::orchestration;
use kinewright_core::AgentEvent;
use limits::{EVENTS, FRAME_BYTES, Frames, event_channel};
use std::{
    io::{self, Read},
    process::{Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

fn fixture() -> &'static std::path::Path {
    static FIXTURE: OnceLock<std::path::PathBuf> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "kinewright-c5-ingress-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let status = Command::new("rustc")
            .args(["--edition", "2024", "-C", "debuginfo=0"])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/ja_s4_native.rs"),
            )
            .arg("-o")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "native fixture build prerequisite");
        path
    })
}
struct CountRead<R> {
    inner: R,
    bytes: Arc<AtomicUsize>,
}
impl<R: Read> Read for CountRead<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(bytes)?;
        self.bytes.fetch_add(n, Ordering::SeqCst);
        Ok(n)
    }
}
struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn s4_ingress_before_allocation() {
    let child = Command::new(fixture())
        .args(["unused", "ingress"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = ChildGuard(child);
    let stderr = limits::stderr_capture(child.0.stderr.take().unwrap(), "c5-stderr").unwrap();
    let read = Arc::new(AtomicUsize::new(0));
    let stdout = CountRead {
        inner: child.0.stdout.take().unwrap(),
        bytes: Arc::clone(&read),
    };
    let mut frames = Frames::new(stdout);
    let error = frames.next().unwrap().unwrap_err();
    assert!(error.to_string().contains("frame_capacity"));
    assert!(
        read.load(Ordering::SeqCst) <= 2 * FRAME_BYTES,
        "reader never materializes the 192KiB unframed payload"
    );
    assert!(frames.next().is_none());
    drop(frames);
    child.0.wait().unwrap();
    let diagnostic = stderr.join().unwrap();
    assert_eq!(diagnostic.as_bytes(), vec![b'e'; FRAME_BYTES]);
    let mut exact = vec![b'a'; FRAME_BYTES];
    exact.push(b'\n');
    assert_eq!(
        Frames::new(&exact[..]).next().unwrap().unwrap().as_bytes(),
        &exact[..FRAME_BYTES]
    );
    let (sender, receiver) = event_channel();
    let accepted = (0..EVENTS - 2)
        .map(|n| AgentEvent::Text(format!("retained-{n}")))
        .collect::<Vec<_>>();
    for event in &accepted {
        sender.send(event.clone()).unwrap();
    }
    assert!(
        sender
            .send(AgentEvent::Text("unaccepted-overflow".into()))
            .is_err()
    );
    assert!(sender.failed());
    // Stop/cancellation can run with every queue slot occupied.
    assert!(sender.send(AgentEvent::Done).is_err());
    for expected in accepted {
        assert_eq!(receiver.try_recv().unwrap(), expected);
    }
    assert!(
        matches!(receiver.try_recv().unwrap(),AgentEvent::Error(reason) if reason.contains("event_capacity"))
    );
    assert_eq!(receiver.try_recv().unwrap(), AgentEvent::Done);
    assert!(receiver.try_recv().is_err());
    let (errors, receiver) = event_channel();
    for n in 0..EVENTS - 2 {
        errors
            .send(AgentEvent::Error(format!("malformed-{n}")))
            .unwrap();
    }
    assert!(
        errors
            .send(AgentEvent::Error("unaccepted error".into()))
            .is_err()
    );
    for n in 0..EVENTS - 2 {
        assert_eq!(
            receiver.try_recv().unwrap(),
            AgentEvent::Error(format!("malformed-{n}"))
        );
    }
    assert!(
        matches!(receiver.try_recv().unwrap(),AgentEvent::Error(reason) if reason.contains("event_capacity"))
    );
    assert_eq!(receiver.try_recv().unwrap(), AgentEvent::Done);
    let mut output = vec![];
    assert!(limits::write_frame(&mut output, &"\u{0000}".repeat(FRAME_BYTES / 2)).is_err());
    assert!(
        output.is_empty(),
        "encoding is admitted before the first pipe write"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_lines,
    reason = "one witness follows accepted objects through admission and actual retirement"
)]
fn s4_every_cap_and_retirement() {
    use kinewright_project::{
        bundle_records::{ObjectRef, ProjectId, RecordKind},
        namespace::ProjectNamespace,
        orchestration_accounting::*,
        storage_budget::StorageLimits,
        store::ProjectStore,
    };
    use limits::{COMMAND_BYTES, COMMANDS, CommandQueue};
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("kinewright-c5-caps-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let project = root.join("accounting.kinewright");
    let mut store = ProjectStore::new(
        ProjectNamespace::acquire(&project).unwrap(),
        Some(ProjectId::fresh().unwrap()),
        None,
        StorageLimits::default(),
    )
    .unwrap();
    let baseline = store.usage().unwrap();
    let queue = CommandQueue::new();
    for n in 0..COMMANDS {
        queue.admit(&store, &format!("accepted-{n}"), &[]).unwrap();
    }
    assert_eq!(
        queue.admit(&store, &"unaccepted", &[]),
        Err(AccountingRefusal::Capacity)
    );
    assert_eq!(
        store.usage().unwrap().reserved_bytes - baseline.reserved_bytes,
        (COMMANDS * COMMAND_BYTES) as u64
    );
    assert!(
        queue
            .admit(&store, &"x".repeat(COMMAND_BYTES), &[])
            .is_err()
    );
    for n in 0..COMMANDS {
        let admitted = queue.receiver.try_recv().unwrap();
        assert_eq!(
            serde_json::from_slice::<String>(&admitted.bytes).unwrap(),
            format!("accepted-{n}")
        );
        assert_eq!(
            store.usage().unwrap().reserved_bytes - baseline.reserved_bytes,
            ((COMMANDS - n) * COMMAND_BYTES) as u64
        );
        drop(admitted);
    }
    assert_eq!(
        store.usage().unwrap().reserved_bytes,
        baseline.reserved_bytes
    );
    assert!(queue.receiver.try_recv().is_err());
    let bytes = b"accepted pending delivery with a live pin";
    let object = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedData,
    };
    let sealed = store.admit_blob(&mut &bytes[..], &object, &[]).unwrap();
    queue
        .admit(
            &store,
            &"accepted dependency",
            std::slice::from_ref(&object),
        )
        .unwrap();
    drop(sealed);
    store.collect_unpublished().unwrap();
    store.verify(&object).unwrap();
    let charged = store.usage().unwrap().physical_bytes;
    let accepted = queue.receiver.try_recv().unwrap();
    store.collect_unpublished().unwrap();
    store.verify(&object).unwrap();
    drop(accepted);
    store.collect_unpublished().unwrap();
    assert!(store.verify(&object).is_err());
    assert!(store.usage().unwrap().physical_bytes < charged);
    let retired = store.usage().unwrap();
    store.collect_unpublished().unwrap();
    assert_eq!(
        store.usage().unwrap().physical_bytes,
        retired.physical_bytes
    );
    let id = || serde_json::from_value(serde_json::json!("1".repeat(32))).unwrap();
    let mut record = AccountingRecord::new(ProjectId::fresh().unwrap(), id(), 1000).unwrap();
    record.consume_event_bytes(RECORD_BYTES as u64).unwrap();
    assert_eq!(
        record.consume_event_bytes(1),
        Err(AccountingRefusal::Capacity)
    );
    assert_eq!(record.durable_event_bytes, RECORD_BYTES as u64);
    for n in 0..TASKS {
        record
            .add_task(
                serde_json::from_value(serde_json::json!(format!("{n:032x}"))).unwrap(),
                None,
            )
            .unwrap();
    }
    assert_eq!(
        record.add_task(id(), None),
        Err(AccountingRefusal::Capacity)
    );
    let next = record.successor().unwrap();
    next.validate_successor(&record).unwrap();
    let mut lost = next;
    lost.durable_event_bytes = 0;
    assert_eq!(
        lost.validate_successor(&record),
        Err(AccountingRefusal::Scope)
    );
    drop(queue);
    drop(store);
    std::fs::remove_dir_all(&root).unwrap();
}

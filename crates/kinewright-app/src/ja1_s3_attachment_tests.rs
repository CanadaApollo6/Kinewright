#![cfg(target_os = "linux")]
use super::*;

#[test]
fn s3_confirm_host_death_enters_l2_before_replacement() {
    for floor in [false, true] {
        let (_temp, mut app, path) = s2h_fix3_app(false);
        app.send_manual(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
        app.send_manual(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: true,
        }))
        .unwrap();
        let actor = app.owner.core().actor_identity();
        let save_proxy = app.host.as_mut().unwrap().drop_http_reply("save", false);
        assert!(app.apply(Action::Save).is_err());
        let confirm_proxy = app
            .host
            .as_mut()
            .unwrap()
            .drop_http_reply("confirm_barrier", false);
        if floor {
            app.host
                .as_mut()
                .unwrap()
                .begin_manual_recovery_with_floor(&mut app.owner, Some(1))
                .unwrap();
        } else {
            app.apply(Action::Retry).unwrap();
        }
        s2h_fix3_wait(&mut app);
        assert!(
            confirm_proxy
                .dropped
                .load(std::sync::atomic::Ordering::Acquire)
        );
        let installed = app.owner.core().export_history().unwrap();
        let child = app.host.as_ref().unwrap().child_id();
        app.host.as_mut().unwrap().kill_owned_child();
        assert!(app.apply(Action::Retry).is_err());
        assert!(
            app.retention_lost,
            "confirmed death must enter L2 before replacement"
        );
        assert_eq!(app.host.as_ref().unwrap().child_id(), child);
        assert_eq!(app.owner.core().actor_identity(), actor);
        assert_eq!(app.owner.core().export_history().unwrap(), installed);
        assert!(app.read_only());
        assert!(app.apply(Action::Save).is_err());
        assert!(app.close());
        drop(app);
        let restored = Core::spawn(kinewright_core::Document::default()).unwrap();
        LoadedBundle::read(&path)
            .unwrap()
            .restore_manual(&restored)
            .unwrap();
        assert_eq!(restored.export_history().unwrap(), installed);
        drop((save_proxy, confirm_proxy));
    }
}

#[test]
fn s3_known_manual_refusal_stays_known() {
    let (_temp, mut app, _path) = s2h_fix3_app(false);
    let before = app.owner.core().export_history().unwrap();
    let client = app.host.as_ref().unwrap().manual_identity_for_test();
    let reply = app
        .host_client()
        .unwrap()
        .request(kinewright_agent::writer_host::HostCommand::BeginRecovery {
            client,
            frozen: Some(
                kinewright_agent::writer_host::HostSnapshot::from_history(before.clone()).unwrap(),
            ),
            pending: vec![],
            discard_batches: None,
        })
        .unwrap();
    assert!(reply.error.is_none(), "{:?}", reply.error);
    app.host.as_mut().unwrap().manual = None;
    assert!(app.start_host_manual().is_err());
    let host = app.host.as_ref().unwrap();
    assert!(
        !host.unknown_operation,
        "a received refusal must stay known"
    );
    assert!(!host.pending_foreground());
    assert!(!app.message.contains("reply was not confirmed"));
    assert_eq!(app.owner.core().export_history().unwrap(), before);
    assert!(!app.retention_lost);
    assert!(app.close());
}

#[test]
fn s3_chat_status_reply_loss_keeps_pending_manual_work() {
    use kinewright_agent::writer_host::HostCommand;
    let (_temp, mut app, _path) = s2h_fix3_app(false);
    let started = app
        .host_client()
        .unwrap()
        .request(HostCommand::StartScripted {
            turns: vec![],
            prompts: vec![],
        })
        .unwrap();
    assert!(started.error.is_none(), "{:?}", started.error);
    let session = started.session.unwrap().id;
    app.send_manual(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }))
    .unwrap();
    let before = app.owner.core().export_history().unwrap();
    assert!(app.host.as_ref().unwrap().pending_count() > 0);
    let child = app.host.as_ref().unwrap().child_id();
    let proxy = app
        .host
        .as_mut()
        .unwrap()
        .drop_http_reply("session_status", false);
    let mut chat = crate::ja1_chat::HostChat::observe_for_test(app.host_client().unwrap(), session);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let error = loop {
        match chat.poll() {
            Err(error) => break error,
            Ok(_) => assert!(std::time::Instant::now() < deadline, "chat reply timeout"),
        }
        std::thread::park_timeout(std::time::Duration::from_millis(2));
    };
    assert!(proxy.dropped.load(std::sync::atomic::Ordering::Acquire));
    assert!(!error.retention_lost, "chat transport loss is unconfirmed");
    let refreshed = loop {
        if let Some(status) = chat.poll().expect("status retry") {
            break status;
        }
        assert!(std::time::Instant::now() < deadline, "chat refresh timeout");
        std::thread::park_timeout(std::time::Duration::from_millis(2));
    };
    assert_eq!(refreshed.id, session);
    assert_eq!(app.host.as_ref().unwrap().child_id(), child);
    assert_eq!(app.owner.core().export_history().unwrap(), before);
    assert!(!app.retention_lost);
    app.apply(Action::Save).unwrap();
    assert!(!app.read_only());
    assert!(app.close());
    drop(proxy);
}

#[test]
fn s3_chat_start_reply_loss_is_unconfirmed_without_resend() {
    let (_temp, mut app, _path) = s2h_fix3_app(false);
    let before = app.owner.core().export_history().unwrap();
    let proxy = app
        .host
        .as_mut()
        .unwrap()
        .drop_http_reply("start_session", false);
    let mut chat = crate::ja1_chat::HostChat::start(
        app.host_client().unwrap(),
        "s3-unavailable-harness".into(),
        kinewright_agent::writer_host::HostSessionConfig::default(),
        "Keep this draft".into(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let error = loop {
        if let Err(error) = chat.poll() {
            break error;
        }
        assert!(std::time::Instant::now() < deadline, "startup timeout");
        std::thread::park_timeout(std::time::Duration::from_millis(2));
    };
    assert!(proxy.dropped.load(std::sync::atomic::Ordering::Acquire));
    assert!(!error.retention_lost);
    assert!(
        !chat.running,
        "unknown startup identity must never be resent"
    );
    assert!(!app.read_only());
    assert_eq!(app.owner.core().export_history().unwrap(), before);
    assert!(app.close());
    drop(proxy);
}

#[test]
fn c6b_dropping_app_fixture_reaps_its_host() {
    let (_temp, app, _path) = s2h_fix3_app(false);
    let host = app.host.as_ref().unwrap();
    let pid = host.child_id().unwrap();
    let client = host.client.clone();
    let process = std::path::PathBuf::from(format!("/proc/{pid}"));
    assert!(process.exists(), "the fixture launched a real host");
    drop(app);
    let survived = process.exists();
    if survived {
        let _ = client.request(kinewright_agent::writer_host::HostCommand::Shutdown);
    }
    assert!(!survived, "C6B_FIXTURE_HOST_REAPED_ON_DROP");
}

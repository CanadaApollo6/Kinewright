#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::writer_host::ja_s3_host::{accepted, publish_command};
    use kinewright_media::{owned_io::ReadAuthority, test_support::TempDirectory};
    use kinewright_project::{
        store::snapshot_reader::{SnapshotLease, SnapshotReadSession},
        studio_records::*,
    };
    use std::{io::SeekFrom, sync::atomic::AtomicBool};

    fn fixture() -> (TempDirectory, WriterHost, SnapshotKey) {
        let root = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c9-lease",
        );
        let [fixture, _] =
            kinewright_project::c0_fixtures::build_c0_fixture_pair(root.root()).unwrap();
        let host = WriterHost::start(HostConfig {
            project: "c9-lease".into(),
            path: Some(fixture.path),
            roots: SessionRoots {
                read: vec![root.root().into()],
                write: vec![root.root().into()],
            },
            origin: HostOrigin::Headless,
        })
        .unwrap();
        let producer = host
            .attach(
                "snapshot-producer".into(),
                AttachmentRole::Producer,
                host.roots.clone(),
            )
            .unwrap();
        let reply = accepted(producer.request(publish_command(&host, &producer)).unwrap());
        let kinewright_project::receipts::ReceiptResult::Metadata {
            result: MetadataResult::Snapshot(key),
        } = reply.metadata.unwrap().result
        else {
            panic!("snapshot")
        };
        (root, host, key)
    }
    fn lease(host: &WriterHost, key: &SnapshotKey) -> (HostClient, SnapshotLease, ReadAuthority) {
        let client = host.client();
        let lease = accepted(
            client
                .request(HostCommand::LeaseSnapshot { key: key.clone() })
                .unwrap(),
        )
        .snapshot_lease
        .unwrap();
        let remote = client
            .snapshot_lease_remote(&lease.id, &lease.payload_sha256)
            .unwrap();
        let origin = Arc::new(kinewright_core::MediaSourceOrigin::new_project());
        let grant = origin.legacy_media().unwrap();
        let authority = ReadAuthority::new(
            grant,
            lease.payload_sha256.clone(),
            Arc::new(move || {
                let result = remote.check();
                if result.is_err() {
                    origin.revoke_legacy_media();
                }
                result.map_err(|_| kinewright_core::MediaError::MediaAdmissionRequired)
            }),
        );
        (client, lease, authority)
    }
    #[test]
    fn c9_host_lease_pins_collection_and_named_writer_retirement() {
        let (_root, host, key) = fixture();
        let (client, lease, authority) = lease(&host, &key);
        let session = SnapshotReadSession::new(StorageLimits::default()).unwrap();
        let path = host.config_path_probe();
        let reader = session.open(&path, &lease, authority).unwrap();
        let context = reader.frozen_context().unwrap();
        let before = serde_json::to_vec(&context.document).unwrap();
        let input = context.certified_input_leases[0]
            .certified_source()
            .unwrap();
        let mut cursor = input.cursor(Arc::new(AtomicBool::new(false))).unwrap();
        let mut bytes = [0; 32];
        cursor.read(&mut bytes).unwrap();
        let owner = host.owner.upgrade().unwrap();
        {
            let mut owner = owner.lock().unwrap();
            owner
                .owner_mut()
                .unwrap()
                .collect_snapshot_objects_probe()
                .unwrap();
            assert!(
                owner
                    .owner_mut()
                    .unwrap()
                    .retire_snapshot_writer_probe()
                    .is_err(),
                "C9_LEASE_BLOCKS_RETIREMENT"
            );
        }
        assert_eq!(
            before,
            serde_json::to_vec(&reader.frozen_context().unwrap().document).unwrap()
        );
        cursor.seek(SeekFrom::Start(0)).unwrap();
        let mut after = [0; 32];
        cursor.read(&mut after).unwrap();
        assert_eq!(bytes, after);
        accepted(
            client
                .request(HostCommand::ReleaseSnapshot {
                    id: lease.id,
                    payload_sha256: lease.payload_sha256,
                })
                .unwrap(),
        );
        owner
            .lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .retire_snapshot_writer_probe()
            .expect("C9_RELEASE_ALLOWS_RETIREMENT");
    }
    #[test]
    fn c9_revocation_stops_every_existing_reader_and_cache_hit() {
        let (_root, host, key) = fixture();
        let (client, lease, authority) = lease(&host, &key);
        let session = SnapshotReadSession::new(StorageLimits::default()).unwrap();
        let reader = session
            .open(&host.config_path_probe(), &lease, authority)
            .unwrap();
        let context = Arc::new(reader.frozen_context().unwrap());
        let source = context.certified_input_leases[0]
            .certified_source()
            .unwrap();
        let mut cursors = (0..3)
            .map(|_| source.cursor(Arc::new(AtomicBool::new(false))).unwrap())
            .collect::<Vec<_>>();
        for cursor in &mut cursors {
            assert_eq!(cursor.read(&mut [0; 32]).unwrap(), 32);
        }
        assert!(!context.resource_context[0].bytes().unwrap().is_empty());
        let inputs = context.playback_inputs().unwrap();
        assert!(!inputs.test_audio_samples().unwrap().is_empty());
        accepted(
            client
                .request(HostCommand::ReleaseSnapshot {
                    id: lease.id.clone(),
                    payload_sha256: lease.payload_sha256.clone(),
                })
                .unwrap(),
        );
        assert!(
            reader
                .read_record::<Document>(&lease.descriptor.document_ref)
                .is_err(),
            "C9_REVOKED_OBJECT_READER"
        );
        for cursor in &mut cursors {
            assert!(
                cursor.seek(SeekFrom::Start(0)).is_err(),
                "C9_REVOKED_CACHED_SEEK"
            );
            let mut bytes = [0; 32];
            assert!(cursor.read(&mut bytes).is_err(), "C9_REVOKED_CACHED_READ");
            assert_eq!(bytes, [0; 32]);
        }
        assert!(
            source.cursor(Arc::new(AtomicBool::new(false))).is_err(),
            "C9_REVOKED_SOURCE_CLONE"
        );
        assert!(
            context.resource_context[0].bytes().is_err(),
            "C9_REVOKED_RESOURCE"
        );
        assert!(inputs.test_audio_samples().is_err(), "C9_REVOKED_PLAYBACK");
        assert!(context.playback_inputs().is_err(), "C9_REVOKED_CONTEXT");
        assert!(
            session
                .open(
                    &host.config_path_probe(),
                    &lease,
                    ReadAuthority::new(
                        kinewright_core::MediaSourceOrigin::new_project()
                            .legacy_media()
                            .unwrap(),
                        lease.payload_sha256.clone(),
                        Arc::new(|| Err(kinewright_core::MediaError::MediaAdmissionRequired))
                    )
                )
                .is_err()
        );
    }
    #[test]
    fn c9_tampered_object_and_host_death_are_refused() {
        let (_root, host, key) = fixture();
        let (_client, lease, authority) = lease(&host, &key);
        let session = SnapshotReadSession::new(StorageLimits::default()).unwrap();
        let path = host.config_path_probe();
        let reader = session.open(&path, &lease, authority).unwrap();
        let object = path
            .with_extension("kinewright-assets")
            .join("studio/objects")
            .join(&lease.descriptor.document_ref.sha256);
        let bytes = std::fs::read(&object).unwrap();
        let mut altered = bytes.clone();
        altered[0] ^= 1;
        std::fs::write(&object, altered).unwrap();
        assert!(
            reader
                .read_record::<Document>(&lease.descriptor.document_ref)
                .is_err(),
            "C9_TAMPERED_OBJECT_REFUSED"
        );
        std::fs::write(&object, bytes).unwrap();
        reader
            .read_record::<Document>(&lease.descriptor.document_ref)
            .unwrap();
        drop(host);
        assert!(
            reader
                .read_record::<Document>(&lease.descriptor.document_ref)
                .is_err(),
            "C9_HOST_DEATH_REVOKES_READER"
        );
    }

    #[test]
    fn c9_lease_authority_cannot_be_relabelled_or_released_by_another_attachment() {
        let (_root, host, key) = fixture();
        let (client, first, first_authority) = lease(&host, &key);
        let (_, second, second_authority) = lease(&host, &key);
        let session = SnapshotReadSession::new(StorageLimits::default()).unwrap();
        assert!(
            session
                .open(&host.config_path_probe(), &second, first_authority)
                .is_err(),
            "C9_LEASE_AUTHORITY_BINDING"
        );
        assert_eq!(session.usage().unwrap().objects, 0);
        let outsider = host
            .attach(
                "other-reader".into(),
                AttachmentRole::Viewer,
                host.roots.clone(),
            )
            .unwrap();
        for command in [
            HostCommand::SnapshotLeaseStatus {
                id: first.id.clone(),
                payload_sha256: first.payload_sha256.clone(),
            },
            HostCommand::ReleaseSnapshot {
                id: first.id.clone(),
                payload_sha256: first.payload_sha256.clone(),
            },
        ] {
            assert_eq!(
                outsider.request(command).unwrap().error.unwrap().code,
                "lease_revoked",
                "C9_ATTACHMENT_OWNS_LEASE"
            );
        }
        accepted(
            client
                .request(HostCommand::SnapshotLeaseStatus {
                    id: first.id,
                    payload_sha256: first.payload_sha256,
                })
                .unwrap(),
        );
        let reader = session
            .open(&host.config_path_probe(), &second, second_authority)
            .unwrap();
        reader.frozen_context().unwrap();
    }

    #[test]
    fn c9_failed_transition_keeps_lease_and_save_as_revokes_it() {
        let (root, host, key) = fixture();
        let (client, lease, authority) = lease(&host, &key);
        let session = SnapshotReadSession::new(StorageLimits::default()).unwrap();
        let reader = session
            .open(&host.config_path_probe(), &lease, authority)
            .unwrap();
        assert!(
            client
                .request(HostCommand::OpenRecovery)
                .unwrap()
                .error
                .is_some()
        );
        reader
            .frozen_context()
            .expect("C9_FAILED_TRANSITION_KEEPS_LEASE");
        let snapshot = {
            let owner = host.owner.upgrade().unwrap();
            let owner = owner.lock().unwrap();
            HostSnapshot::capture(owner.core()).unwrap()
        };
        accepted(
            client
                .request(HostCommand::SaveAs {
                    destination: root.path("lease-copy.kinewright"),
                    snapshot,
                })
                .unwrap(),
        );
        assert!(
            reader.frozen_context().is_err(),
            "C9_PROJECT_CHANGE_REVOKES_LEASE"
        );
        assert_eq!(
            client
                .request(HostCommand::SnapshotLeaseStatus {
                    id: lease.id,
                    payload_sha256: lease.payload_sha256
                })
                .unwrap()
                .error
                .unwrap()
                .code,
            "lease_revoked"
        );
    }
    #[test]
    fn fix1_lease_honors_forwarded_roots_on_both_routes() {
        let (_root, host, key) = fixture();
        let client = host.client();
        let lease = accepted(
            client
                .request(HostCommand::LeaseSnapshot { key: key.clone() })
                .unwrap(),
        )
        .snapshot_lease
        .unwrap();
        let mut narrowed = client.clone();
        narrowed.bootstrap.roots = SessionRoots::default();
        let refused = narrowed
            .request(HostCommand::LeaseSnapshot { key: key.clone() })
            .unwrap();
        assert_eq!(
            refused.error.as_ref().map(|e| e.code.as_str()),
            Some("root_refused"),
            "FIX1_LEASE_ROOTS_ISSUANCE"
        );
        assert!(refused.snapshot_lease.is_none());
        let status = HostCommand::SnapshotLeaseStatus {
            id: lease.id.clone(),
            payload_sha256: lease.payload_sha256.clone(),
        };
        assert_eq!(
            narrowed
                .request(status.clone())
                .unwrap()
                .error
                .unwrap()
                .code,
            "root_refused",
            "FIX1_LEASE_ROOTS_STATUS"
        );
        assert!(
            narrowed
                .snapshot_lease_remote(&lease.id, &lease.payload_sha256)
                .unwrap()
                .check()
                .is_err(),
            "FIX1_LEASE_ROOTS_DEDICATED"
        );
        accepted(client.request(status).unwrap());
        client
            .snapshot_lease_remote(&lease.id, &lease.payload_sha256)
            .unwrap()
            .check()
            .unwrap();
        accepted(
            narrowed
                .request(HostCommand::ReleaseSnapshot {
                    id: lease.id.clone(),
                    payload_sha256: lease.payload_sha256,
                })
                .unwrap(),
        );
    }
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one retained playback session covers timeout, recovery, paused target and cancellation"
    )]
    fn fix2_lease_status_timeout_suspends_then_resumes_playback() {
        use kinewright_core::{Playback, TimeCode};
        let (_root, host, key) = fixture();
        let client = host.client();
        let lease = accepted(client.request(HostCommand::LeaseSnapshot { key }).unwrap())
            .snapshot_lease
            .unwrap();
        let remote = client
            .snapshot_lease_remote(&lease.id, &lease.payload_sha256)
            .unwrap();
        let origin = Arc::new(kinewright_core::MediaSourceOrigin::new_project());
        let grant = origin.legacy_media().unwrap();
        let checker = remote.clone();
        let revoke = Arc::clone(&origin);
        let authority = ReadAuthority::new(
            grant,
            lease.payload_sha256.clone(),
            Arc::new(move || {
                let result = checker.check_cached();
                match result {
                    Ok(()) => Ok(()),
                    Err(SnapshotLeaseError::TemporarilyUnavailable) => {
                        Err(ReadAuthority::temporarily_unavailable())
                    }
                    Err(SnapshotLeaseError::Ended) => {
                        revoke.revoke_legacy_media();
                        Err(kinewright_core::MediaError::MediaAdmissionRequired)
                    }
                }
            }),
        );
        let session = SnapshotReadSession::new(StorageLimits::default()).unwrap();
        let reader = session
            .open(&host.config_path_probe(), &lease, authority)
            .unwrap();
        let context = Arc::new(reader.frozen_context().unwrap());
        let inputs = context.playback_inputs().unwrap();
        let engine = Arc::new(
            kinewright_media::FfmpegMediaEngine::new_with_paced_test_audio(
                kinewright_media::GpuContext::headless(true).unwrap(),
                kinewright_media::default_data_dir(),
            )
            .unwrap(),
        );
        let mut document = context.document.clone();
        for track in &mut document.tracks {
            let original = track.clips[0].clone();
            track.clips = (0..40_u64)
                .map(|index| {
                    let mut clip = original.clone();
                    clip.id = kinewright_core::ClipId(track.id.0 * 1000 + index);
                    clip.timeline_start = TimeCode(i64::try_from(index * 60).unwrap());
                    clip
                })
                .collect();
        }
        document.duration = TimeCode(39 * 60 + context.document.duration.0);
        let document = Arc::new(document);
        let media = engine.certified_media_for_document(Arc::clone(&inputs), Arc::clone(&document));
        media.set_document(document);
        let frames = media.frames();
        media.play(TimeCode::ZERO);
        media.request_frame(TimeCode::ZERO);
        frames
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|error| {
                panic!(
                    "FIX1_PLAYBACK_POSITIVE {error:?}: {:?}",
                    media.events().try_iter().collect::<Vec<_>>()
                )
            });
        let owner = host.owner.upgrade().unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let load = thread::spawn(move || {
            let _held = owner.lock().unwrap();
            tx.send(()).unwrap();
            thread::sleep(Duration::from_secs(4));
        });
        rx.recv().unwrap();
        thread::sleep(Duration::from_millis(500));
        let probing = std::time::Instant::now();
        assert!(
            remote.check_cached() == Err(SnapshotLeaseError::TemporarilyUnavailable),
            "FIX2_TIMEOUT_SUSPENDS_READS"
        );
        assert!(
            probing.elapsed() >= Duration::from_secs(1),
            "FIX1_REAL_STATUS_TIMEOUT"
        );
        assert!(origin.legacy_media().is_some());
        load.join().unwrap();
        let before = media.position();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while media.position() <= before {
            assert!(
                std::time::Instant::now() < deadline,
                "FIX1_PLAYBACK_AFTER_TIMEOUT"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!inputs.test_audio_samples().unwrap().is_empty());
        let events = media.events();
        media.pause();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !matches!(
            events.recv_timeout(Duration::from_millis(100)),
            Ok(kinewright_core::MediaEvent::PlaybackStateChanged(
                kinewright_core::PlaybackState::Paused
            ))
        ) {
            assert!(std::time::Instant::now() < deadline);
        }
        let load = saturate_status(&client, &lease);
        thread::sleep(Duration::from_millis(260));
        media.play(TimeCode::ZERO);
        thread::sleep(Duration::from_millis(100));
        media.pause();
        let applied = engine.test_applied_scene().unwrap();
        assert_eq!(
            applied["epoch"].as_u64(),
            Some(media.stamp().epoch),
            "FIX2_PAUSE_APPLIED_EPOCH"
        );
        assert_eq!(
            applied["seq"].as_u64(),
            Some(media.stamp().seq),
            "FIX2_PAUSE_APPLIED_SEQUENCE"
        );
        for _ in frames.try_iter() {}
        media.request_frame(TimeCode(6));
        drop(load);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if frames
                .recv_timeout(Duration::from_millis(100))
                .is_ok_and(|frame| frame.at == TimeCode(6))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "FIX2_PAUSED_RETRY_KEEPS_TARGET"
            );
        }
        let paused = media.position();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(media.position(), paused, "FIX2_NO_UNREQUESTED_RESUME");
        remote.release().unwrap();
        media.request_frame(TimeCode(3));
        assert!(
            inputs.test_audio_samples().is_err(),
            "FIX1_EXPLICIT_REVOKE_STOPS_PLAYBACK"
        );
        media.pause();
        drop(media);
        drop(engine);
    }
    fn dedicated_status(client: &HostClient, lease: &SnapshotLease) -> reqwest::StatusCode {
        let request = HostRequest {
            editor: None,
            operation: None,
            protocol: HOST_PROTOCOL,
            project: client.endpoint().project.clone(),
            roots: client.bootstrap.roots.clone(),
            command: HostCommand::SnapshotLeaseStatus {
                id: lease.id.clone(),
                payload_sha256: lease.payload_sha256.clone(),
            },
        };
        reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap()
            .post(format!(
                "http://{}/snapshot-lease",
                client.endpoint().address
            ))
            .header("authorization", client.bootstrap.auth.bearer())
            .header("x-kinewright-project", &client.endpoint().project)
            .header("x-kinewright-protocol", "1")
            .json(&request)
            .send()
            .unwrap()
            .status()
    }
    fn saturate_status(client: &HostClient, lease: &SnapshotLease) -> Vec<std::net::TcpStream> {
        use std::io::Write as _;
        let mut streams = Vec::new();
        for _ in 0..64 {
            let mut socket = std::net::TcpStream::connect(client.endpoint().address).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(20)))
                .unwrap();
            write!(socket,"POST /snapshot-lease HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: {}\r\nx-kinewright-project: {}\r\nx-kinewright-protocol: 1\r\nContent-Length: 2\r\n\r\n{{",client.bootstrap.auth.bearer(),client.endpoint().project).unwrap();
            streams.push(socket);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while dedicated_status(client, lease) != StatusCode::TOO_MANY_REQUESTS {
            assert!(
                std::time::Instant::now() < deadline,
                "FIX2_STATUS_SATURATED"
            );
            thread::sleep(Duration::from_millis(5));
        }
        streams
    }
    fn cached_authority(
        client: &HostClient,
        lease: &SnapshotLease,
    ) -> (
        SnapshotLeaseRemote,
        Arc<kinewright_core::MediaSourceOrigin>,
        ReadAuthority,
    ) {
        let remote = client
            .snapshot_lease_remote(&lease.id, &lease.payload_sha256)
            .unwrap();
        remote.check_cached().unwrap();
        let origin = Arc::new(kinewright_core::MediaSourceOrigin::new_project());
        let checker = remote.clone();
        let revoke = Arc::clone(&origin);
        let authority = ReadAuthority::new(
            origin.legacy_media().unwrap(),
            lease.payload_sha256.clone(),
            Arc::new(move || match checker.check_cached() {
                Ok(()) => Ok(()),
                Err(SnapshotLeaseError::TemporarilyUnavailable) => {
                    Err(ReadAuthority::temporarily_unavailable())
                }
                Err(SnapshotLeaseError::Ended) => {
                    revoke.revoke_legacy_media();
                    Err(kinewright_core::MediaError::MediaAdmissionRequired)
                }
            }),
        );
        (remote, origin, authority)
    }
    #[test]
    fn fix2_host_revocation_during_busy_status_stops_retained_reads() {
        for transition in [false, true] {
            let (root, host, key) = fixture();
            let client = host.client();
            let lease = accepted(client.request(HostCommand::LeaseSnapshot { key }).unwrap())
                .snapshot_lease
                .unwrap();
            let (remote, origin, authority) = cached_authority(&client, &lease);
            let session = SnapshotReadSession::new(StorageLimits::default()).unwrap();
            let reader = session
                .open(&host.config_path_probe(), &lease, authority)
                .unwrap();
            let context = Arc::new(reader.frozen_context().unwrap());
            let source = context.certified_input_leases[0]
                .certified_source()
                .unwrap();
            let mut cursors = (0..3)
                .map(|_| source.cursor(Arc::new(AtomicBool::new(false))).unwrap())
                .collect::<Vec<_>>();
            for cursor in &mut cursors {
                assert_eq!(cursor.read(&mut [0; 32]).unwrap(), 32);
            }
            assert!(!context.resource_context[0].bytes().unwrap().is_empty());
            // Force a successful status after setup, then hold the dedicated lane busy.
            thread::sleep(Duration::from_millis(260));
            remote.check_cached().unwrap();
            let last_positive = std::time::Instant::now();
            let load = saturate_status(&client, &lease);
            if transition {
                let snapshot = {
                    let owner = host.owner.upgrade().unwrap();
                    let owner = owner.lock().unwrap();
                    HostSnapshot::capture(owner.core()).unwrap()
                };
                accepted(
                    client
                        .request(HostCommand::SaveAs {
                            destination: root.path("busy-copy.kinewright"),
                            snapshot,
                        })
                        .unwrap(),
                );
            } else {
                accepted(
                    client
                        .request(HostCommand::ReleaseSnapshot {
                            id: lease.id.clone(),
                            payload_sha256: lease.payload_sha256.clone(),
                        })
                        .unwrap(),
                );
            }
            let remaining = (last_positive + Duration::from_millis(250))
                .saturating_duration_since(std::time::Instant::now());
            thread::sleep(remaining);
            for cursor in &mut cursors {
                let mut bytes = [0; 32];
                assert!(
                    matches!(
                        cursor.read(&mut bytes),
                        Err(kinewright_media::owned_io::OwnedIoError::TemporarilyUnavailable)
                    ),
                    "FIX2_BUSY_REVOKED_READ_STOP transition={transition}"
                );
                assert_eq!(bytes, [0; 32]);
            }
            assert!(
                reader
                    .read_record::<Document>(&lease.descriptor.document_ref)
                    .is_err()
            );
            assert!(context.resource_context[0].bytes().is_err());
            assert!(origin.legacy_media().is_some(), "FIX2_BUSY_IS_NOT_ENDED");
            for _ in 0..3 {
                thread::sleep(Duration::from_millis(70));
                assert_eq!(
                    remote.check_cached(),
                    Err(SnapshotLeaseError::TemporarilyUnavailable),
                    "FIX2_BUSY_NEVER_RENEWS"
                );
            }
            assert_eq!(
                dedicated_status(&client, &lease),
                StatusCode::TOO_MANY_REQUESTS
            );
            drop(load);
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while remote.check_cached() != Err(SnapshotLeaseError::Ended) {
                assert!(std::time::Instant::now() < deadline);
                thread::sleep(Duration::from_millis(20));
            }
            assert!(cursors[0].read(&mut [0; 32]).is_err());
        }
    }
    #[test]
    fn fix2_busy_without_revocation_resumes_same_cached_cursor() {
        let (_root, host, key) = fixture();
        let client = host.client();
        let lease = accepted(client.request(HostCommand::LeaseSnapshot { key }).unwrap())
            .snapshot_lease
            .unwrap();
        let (remote, origin, authority) = cached_authority(&client, &lease);
        let session = SnapshotReadSession::new(StorageLimits::default()).unwrap();
        let reader = session
            .open(&host.config_path_probe(), &lease, authority)
            .unwrap();
        let context = reader.frozen_context().unwrap();
        let source = context.certified_input_leases[0]
            .certified_source()
            .unwrap();
        let mut cursor = source.cursor(Arc::new(AtomicBool::new(false))).unwrap();
        let mut before = [0; 32];
        cursor.read(&mut before).unwrap();
        cursor.seek(SeekFrom::Start(0)).unwrap();
        let load = saturate_status(&client, &lease);
        thread::sleep(Duration::from_millis(260));
        assert!(matches!(
            cursor.read(&mut [0; 32]),
            Err(kinewright_media::owned_io::OwnedIoError::TemporarilyUnavailable)
        ));
        assert!(origin.legacy_media().is_some());
        drop(load);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while remote.check_cached().is_err() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        let mut after = [0; 32];
        assert_eq!(cursor.read(&mut after).unwrap(), 32);
        assert_eq!(before, after);
    }
    #[test]
    fn fix2_lease_body_deadline_frees_all_status_permits() {
        use std::io::Read as _;
        let (_root, host, key) = fixture();
        let client = host.client();
        let lease = accepted(client.request(HostCommand::LeaseSnapshot { key }).unwrap())
            .snapshot_lease
            .unwrap();
        let mut load = saturate_status(&client, &lease);
        for socket in &mut load {
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            assert!(
                response.starts_with("HTTP/1.1 408"),
                "FIX2_LEASE_BODY_TIMEOUT {response}"
            );
        }
        assert_eq!(dedicated_status(&client, &lease), StatusCode::NO_CONTENT);
    }
    impl WriterHost {
        fn config_path_probe(&self) -> std::path::PathBuf {
            self.owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .config
                .path
                .clone()
                .unwrap()
        }
    }
}

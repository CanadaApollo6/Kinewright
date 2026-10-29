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
                result
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
                        Arc::new(|| Err("ended".into()))
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

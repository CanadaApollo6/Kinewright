#[cfg(test)]
mod tests {
    //! C8 witnesses use the same transport and recovery client as Studio.
    use crate::writer_host::ja_s3_host::{accepted, feedback_revision, metadata, publish_command};
    use crate::writer_host::*;
    use kinewright_media::{
        test_support::TempDirectory,
        voice_note::{CaptureStatus, PlayheadLog, VoiceTranscript, VoiceWord},
    };
    use kinewright_project::{
        receipts::ReceiptResult, studio_records::*, voice_ingress::VoiceUpload,
    };
    use std::{io::Cursor, path::Path, sync::mpsc, time::Instant};

    fn fixture() -> (TempDirectory, WriterHost, Vec<ObjectRef>) {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c8-voice",
        );
        let [fixture, _] =
            kinewright_project::c0_fixtures::build_c0_fixture_pair(temp.root()).unwrap();
        let mut bundle = kinewright_project::bundle::LoadedBundle::read(&fixture.path)
            .unwrap()
            .into_writer()
            .unwrap();
        let mut refs = Vec::new();
        for bytes in [b"original feedback".as_slice(), b"[]", b"{}"] {
            let reference = ref_for(bytes);
            let sealed = bundle
                .store_mut()
                .unwrap()
                .admit_blob(&mut &*bytes, &reference, &[])
                .unwrap();
            bundle.retain_evidence(&reference).unwrap();
            refs.push(reference);
            drop(sealed);
        }
        let core = Core::spawn(Document::default()).unwrap();
        // Restore the actual saved history before retaining the fixture evidence.
        let loaded = kinewright_project::bundle::LoadedBundle::read(&fixture.path).unwrap();
        loaded.restore_manual(&core).unwrap();
        let inputs = loaded
            .manual_checkpoint()
            .unwrap()
            .input_manifest_ref
            .clone();
        bundle.save(&core, &inputs).unwrap();
        drop(bundle);
        let host = WriterHost::start(HostConfig {
            scheduler_settings: None,
            project: "s3-capabilities".into(),
            path: Some(fixture.path),
            roots: SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            },
            origin: HostOrigin::Headless,
        })
        .unwrap();
        (temp, host, refs)
    }

    fn wav(silence: bool) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend(32_036_u32.to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16_u32.to_le_bytes());
        out.extend(1_u16.to_le_bytes());
        out.extend(1_u16.to_le_bytes());
        out.extend(16_000_u32.to_le_bytes());
        out.extend(32_000_u32.to_le_bytes());
        out.extend(2_u16.to_le_bytes());
        out.extend(16_u16.to_le_bytes());
        out.extend(b"data");
        out.extend(32_000_u32.to_le_bytes());
        for n in 0..16_000 {
            let sample: i16 = if silence {
                0
            } else if n < 8000 {
                3000
            } else {
                -7000
            };
            out.extend(sample.to_le_bytes());
        }
        out
    }
    fn ref_for(bytes: &[u8]) -> ObjectRef {
        ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: kinewright_project::bundle_records::RecordKind::OwnedEvidence,
        }
    }
    fn snapshot(host: &WriterHost) -> SnapshotKey {
        let producer = host
            .attach(
                "voice-snapshot".into(),
                AttachmentRole::Producer,
                host.roots.clone(),
            )
            .unwrap();
        let reply = accepted(producer.request(publish_command(host, &producer)).unwrap());
        let ReceiptResult::Metadata {
            result: MetadataResult::Snapshot(key),
        } = reply.metadata.unwrap().result
        else {
            panic!("snapshot")
        };
        key
    }
    fn descriptor(
        client: &HostClient,
        key: SnapshotKey,
        refs: &[ObjectRef],
        silence: bool,
    ) -> (VoiceUpload, Vec<u8>, Vec<u8>) {
        let mut anchor = feedback_revision(key, refs).anchor;
        anchor.fps.numerator = 30;
        let mut log = PlayheadLog::new(anchor);
        log.version = 2;
        log.capture_status = Some(CaptureStatus {
            complete: true,
            capture_error: None,
        });
        let log = serde_json::to_vec(&log).unwrap();
        let wav = wav(silence);
        let grant = accepted(
            client
                .request(HostCommand::IssueMetadataIssuer { issuer: None })
                .unwrap(),
        )
        .metadata_issuer
        .unwrap();
        let descriptor = VoiceUpload {
            envelope: grant.envelope,
            root_generation: grant.root_generation,
            note: NoteId::fresh().unwrap(),
            duration_micros: 1_000_000,
            audio: ref_for(&wav),
            log: ref_for(&log),
        };
        (descriptor, wav, log)
    }
    fn send(client: &HostClient, upload: &VoiceUpload, wav: &[u8], log: &[u8]) -> HostReply {
        accepted(
            client
                .acknowledge_voice(upload.clone(), Cursor::new([wav, log].concat()))
                .unwrap(),
        )
    }
    fn usage(host: &WriterHost) -> kinewright_project::storage_budget::StorageUsage {
        host.owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .usage()
            .unwrap()
    }
    fn notes(client: &HostClient) -> FeedbackIndex {
        accepted(client.request(HostCommand::ListFeedback).unwrap())
            .feedback_index
            .unwrap()
    }

    #[test]
    fn fix1_stalled_voice_body_releases_permit_and_reservation() {
        use std::io::{Read as _, Write as _};
        let (_temp, host, refs) = fixture();
        let client = host.client();
        let (upload, wav, log) = descriptor(&client, snapshot(&host), &refs, false);
        let before = usage(&host);
        let request = HostRequest {
            editor: client.editor.clone(),
            operation: None,
            protocol: HOST_PROTOCOL,
            project: client.endpoint().project.clone(),
            roots: client.bootstrap.roots.clone(),
            command: HostCommand::AcknowledgeVoice {
                upload: upload.clone(),
            },
        };
        let mut socket = std::net::TcpStream::connect(client.endpoint().address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(25)))
            .unwrap();
        write!(socket, "POST /voice HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: {}\r\nx-kinewright-project: {}\r\nx-kinewright-protocol: 1\r\nx-kinewright-voice: {}\r\nContent-Length: {}\r\n\r\nx", client.bootstrap.auth.bearer(), client.endpoint().project, serde_json::to_string(&request).unwrap(), wav.len()+log.len()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while usage(&host).reserved_bytes == before.reserved_bytes {
            assert!(Instant::now() < deadline, "FIX1_UPLOAD_ADMITTED");
            thread::sleep(Duration::from_millis(10));
        }
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        assert!(
            response.starts_with("HTTP/1.1 408"),
            "FIX1_BODY_DEADLINE {response}"
        );
        assert_eq!(usage(&host).reserved_bytes, before.reserved_bytes);
        assert_eq!(usage(&host).objects, before.objects);
        send(&client, &upload, &wav, &log);
        assert_eq!(notes(&client).notes.len(), 1);
    }

    #[test]
    fn c8_bounds_refuse_without_objects_or_reservations() {
        let (_temp, host, refs) = fixture();
        let key = snapshot(&host);
        let client = host.client();
        let (mut upload, wav, log) = descriptor(&client, key, &refs, false);
        let before = usage(&host);
        // Valid one-second PCM with an excessive ancillary WAV chunk tests the physical bound.
        let length = 3_905_538_usize;
        let junk = length - wav.len() - 8;
        let mut oversized = wav[..12].to_vec();
        oversized.extend(b"JUNK");
        oversized.extend(u32::try_from(junk).unwrap().to_le_bytes());
        oversized.resize(20 + junk, 0);
        oversized.extend(&wav[12..]);
        oversized[4..8].copy_from_slice(&u32::try_from(length - 8).unwrap().to_le_bytes());
        upload.audio = ref_for(&oversized);
        let result = client.acknowledge_voice(
            upload.clone(),
            Cursor::new([&oversized[..], &log[..]].concat()),
        );
        assert!(
            result.is_err() || result.unwrap().error.is_some(),
            "C8_OVERSIZE_REFUSED"
        );
        let after = usage(&host);
        assert_eq!(after.objects, before.objects, "C8_OVERSIZE_NO_OBJECT");
        assert_eq!(
            after.reserved_bytes, before.reserved_bytes,
            "C8_OVERSIZE_NO_RESERVATION"
        );
        upload.audio = ref_for(&wav);
        assert!(
            client
                .acknowledge_voice(upload.clone(), Cursor::new(wav[..100].to_vec()))
                .is_err(),
            "C8_TRUNCATED_REFUSED"
        );
        let after = usage(&host);
        assert_eq!(after.objects, before.objects);
        assert_eq!(after.reserved_bytes, before.reserved_bytes);
        assert!(notes(&client).notes.is_empty());
        send(&client, &upload, &wav, &log);
        assert_eq!(notes(&client).notes.len(), 1);
    }

    #[test]
    fn c8_private_range_and_transcript_in_batch() {
        let (_temp, host, refs) = fixture();
        let key = snapshot(&host);
        let client = host.client();
        let (upload, wav, log) = descriptor(&client, key.clone(), &refs, false);
        let mut voice = send(&client, &upload, &wav, &log).voice.unwrap().voice;
        let text = b"Make this chart warmer, please.";
        let text_ref = accepted(
            client
                .request(HostCommand::AdmitFeedbackEvidence {
                    kind: FeedbackEvidenceKind::Text,
                    bytes: text.to_vec(),
                })
                .unwrap(),
        )
        .feedback_evidence
        .unwrap();
        let grant = accepted(
            client
                .request(HostCommand::IssueMetadataIssuer {
                    issuer: Some(upload.envelope.id.issuer.clone()),
                })
                .unwrap(),
        )
        .metadata_issuer
        .unwrap();
        let mut stale = grant.envelope.clone();
        stale.owner_epoch = stale.owner_epoch.wrapping_add(1).max(1);
        let refused = client
            .request(HostCommand::ChooseTranscript {
                envelope: stale,
                note: upload.note.clone(),
                text_ref: text_ref.clone(),
            })
            .unwrap();
        assert_eq!(
            refused.error.unwrap().code,
            "history_changed",
            "C9_STALE_CHOICE_REFUSED"
        );
        let choice_generation = grant.root_generation;
        let choice = HostCommand::ChooseTranscript {
            envelope: grant.envelope,
            note: upload.note.clone(),
            text_ref: text_ref.clone(),
        };
        let status = accepted(client.request(choice.clone()).unwrap())
            .voice
            .unwrap();
        assert_eq!(status.text.as_bytes(), text, "C8_EDIT_THEN_CHOOSE");
        {
            let owner = host.owner.upgrade().unwrap();
            let owner = owner.lock().unwrap();
            assert_eq!(
                owner.owner().unwrap().metadata_generation().unwrap(),
                choice_generation + 1,
                "C9_ONE_CHOICE_PUBLICATION"
            );
            let index = owner
                .resolve_attachment(Some(&client.bootstrap.auth.bearer()))
                .unwrap();
            assert!(
                !owner.attachments[index]
                    .read_pins
                    .iter()
                    .any(|pin| pin.roots == vec![text_ref.clone()]),
                "C9_CHOSEN_TEXT_ADMISSION_RELEASED"
            );
        }
        accepted(client.request(choice).unwrap());
        voice.chosen_transcript_revision = Some(text_ref.clone());
        voice.range = Some(AudioRange {
            start_micros: 250_000,
            end_micros: 750_000,
        });
        let mut revision = feedback_revision(key, &refs);
        revision.anchor.fps.numerator = 30;
        revision.voice_ref = Some(voice);
        revision.transcript_revision = Some(text_ref);
        let saved = accepted(
            client
                .request(metadata(
                    &host,
                    &client,
                    MetadataChange::SaveFeedback(revision),
                ))
                .unwrap(),
        );
        let ReceiptResult::Metadata {
            result: MetadataResult::Feedback(reference),
        } = saved.metadata.unwrap().result
        else {
            panic!("feedback")
        };
        let allowed = accepted(
            client
                .request(HostCommand::ReadVoiceRange {
                    revision: reference.clone(),
                    start_micros: 250_000,
                    end_micros: 750_000,
                })
                .unwrap(),
        )
        .voice_audio
        .unwrap();
        assert_eq!(
            kinewright_media::voice_note::voice_audio_duration(&allowed).unwrap(),
            500_000
        );
        assert_eq!(
            allowed,
            kinewright_media::voice_note::voice_range_wav(&wav, 250_000, 750_000).unwrap()
        );
        let refused = client
            .request(HostCommand::ReadVoiceRange {
                revision: reference.clone(),
                start_micros: 0,
                end_micros: 1_000_000,
            })
            .unwrap();
        assert!(refused.error.is_some(), "C8_PRIVATE_RANGE_REFUSED");
        assert!(refused.voice_audio.is_none(), "C8_PRIVATE_NO_BYTES");
        let batch = FeedbackBatch {
            batch_id: FeedbackBatchId::fresh().unwrap(),
            revision_refs: vec![reference],
        };
        let HostCommand::Metadata {
            envelope,
            mut update,
        } = metadata(&host, &client, MetadataChange::SaveVoice(status.voice))
        else {
            unreachable!()
        };
        update.change = MetadataChange::SubmitBatch {
            batch: batch.clone(),
            request: ReviseRequest {
                command_id: envelope.id.clone(),
                batch_id: batch.batch_id.clone(),
                expected_candidate_head: upload.envelope.expected_head.unwrap(),
            },
        };
        accepted(
            client
                .request(HostCommand::Metadata { envelope, update })
                .unwrap(),
        );
        let read = accepted(
            client
                .request(HostCommand::ReadSubmittedBatch {
                    batch: batch.batch_id,
                })
                .unwrap(),
        );
        assert_eq!(
            read.submitted_revisions[0]
                .voice_ref
                .as_ref()
                .unwrap()
                .note_id,
            upload.note,
            "C8_BATCH_VOICE_REVISION"
        );
    }

    #[test]
    fn c8_missing_model_and_silence_are_plain() {
        let (temp, host, refs) = fixture();
        let key = snapshot(&host);
        let client = host.client();
        host.owner.upgrade().unwrap().lock().unwrap().voice_data_dir = temp.path("no-model");
        for silence in [false, true] {
            let (upload, wav, log) = descriptor(&client, key.clone(), &refs, silence);
            send(&client, &upload, &wav, &log);
            accepted(
                client
                    .request(HostCommand::StartTranscription {
                        note: upload.note.clone(),
                    })
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let status = accepted(
                    client
                        .request(HostCommand::VoiceStatus {
                            note: upload.note.clone(),
                        })
                        .unwrap(),
                )
                .voice
                .unwrap();
                if status.progress.contains(if silence {
                    "No speech"
                } else {
                    "not installed"
                }) {
                    assert!(status.text.is_empty());
                    assert_eq!(status.voice.audio_ref, upload.audio);
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "C8_PLAIN_REASON {}",
                    status.progress
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    struct ProcessHost {
        client: HostClient,
        child: Child,
    }
    impl ProcessHost {
        fn start(root: &Path) -> Self {
            let executable = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("kinewright");
            let (client, child) = HostClient::launch(
                &executable,
                HostConfig {
                    scheduler_settings: None,
                    project: "s3-capabilities".into(),
                    path: Some(root.join("c0-red.kinewright")),
                    roots: SessionRoots {
                        read: vec![root.into()],
                        write: vec![root.into()],
                    },
                    origin: HostOrigin::Headless,
                },
            )
            .unwrap();
            Self { client, child }
        }
        fn crash(&mut self) {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
        }
    }
    impl Drop for ProcessHost {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    struct Interrupted {
        bytes: Cursor<Vec<u8>>,
        entered: Option<mpsc::SyncSender<()>>,
        resume: mpsc::Receiver<()>,
    }
    impl Read for Interrupted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.bytes.position() > 0
                && let Some(entered) = self.entered.take()
            {
                entered.send(()).unwrap();
                let _ = self.resume.recv_timeout(Duration::from_secs(15));
            }
            let len = buf.len().min(128);
            self.bytes.read(&mut buf[..len])
        }
    }
    #[test]
    fn c8_restart_before_commit_and_after_lost_reply() {
        for committed in [false, true] {
            let (temp, initial, refs) = fixture();
            let key = snapshot(&initial);
            drop(initial);
            let mut host = ProcessHost::start(temp.root());
            let (mut upload, wav, log) = descriptor(&host.client, key, &refs, false);
            let capture = temp.path("app-capture.wav");
            std::fs::write(&capture, &wav).unwrap();
            let original = upload.clone();
            if committed {
                send(&host.client, &upload, &wav, &log);
                host.crash();
            } else {
                let (entered, arrived) = mpsc::sync_channel(1);
                let (resume, resumed) = mpsc::channel();
                let client = host.client.clone();
                let descriptor = upload.clone();
                let bytes = [&wav[..], &log[..]].concat();
                let worker = thread::spawn(move || {
                    client.acknowledge_voice(
                        descriptor,
                        Interrupted {
                            bytes: Cursor::new(bytes),
                            entered: Some(entered),
                            resume: resumed,
                        },
                    )
                });
                arrived.recv_timeout(Duration::from_secs(15)).unwrap();
                host.crash();
                let _ = resume.send(());
                assert!(worker.join().unwrap().is_err());
            }
            assert_eq!(std::fs::read(&capture).unwrap(), wav, "C8_CAPTURE_RETAINED");
            let restarted = ProcessHost::start(temp.root());
            let receipt = accepted(
                restarted
                    .client
                    .request(HostCommand::VoiceReceipt {
                        upload: original.clone(),
                    })
                    .unwrap(),
            );
            assert_eq!(
                receipt.voice_receipt,
                Some(if committed {
                    VoiceReceipt::Committed
                } else {
                    VoiceReceipt::Absent
                }),
                "C8_ORIGINAL_RECEIPT_FIRST"
            );
            let settled = restarted.client.recover_voice_upload(&mut upload).unwrap();
            if committed {
                assert!(settled.is_some());
                assert_eq!(
                    upload.envelope, original.envelope,
                    "C8_NO_REISSUE_AFTER_COMMIT"
                );
            } else {
                assert!(settled.is_none());
                assert_ne!(upload.envelope.owner_epoch, original.envelope.owner_epoch);
                send(&restarted.client, &upload, &wav, &log);
            }
            let index = notes(&restarted.client);
            assert_eq!(index.notes.len(), 1, "C8_ONE_NOTE_AFTER_RESTART");
            let voice = &index.notes[&upload.note];
            assert_eq!(
                std::fs::read(
                    temp.path("c0-red.kinewright-assets/studio/objects")
                        .join(&voice.audio_ref.sha256)
                )
                .unwrap(),
                wav,
                "C8_RETRY_AUDIO_BYTES"
            );
            std::fs::remove_file(capture).unwrap();
        }
    }

    #[test]
    fn c8_acknowledged_audio_survives_host_crash_before_asr() {
        let (temp, initial, refs) = fixture();
        let key = snapshot(&initial);
        drop(initial);
        let mut host = ProcessHost::start(temp.root());
        let (upload, wav, log) = descriptor(&host.client, key, &refs, false);
        send(&host.client, &upload, &wav, &log);
        eprintln!("C8_ACKNOWLEDGED_BEFORE_HOST_CRASH");
        host.crash();
        let reopened = WriterHost::start(HostConfig {
            scheduler_settings: None,
            project: "s3-capabilities".into(),
            path: Some(temp.path("c0-red.kinewright")),
            roots: SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            },
            origin: HostOrigin::Headless,
        })
        .unwrap();
        let client = reopened.client();
        {
            let owner = reopened.owner.upgrade().unwrap();
            let mut owner = owner.lock().unwrap();
            let voice = owner
                .owner()
                .unwrap()
                .voice_note(&upload.note)
                .unwrap()
                .snapshot()
                .unwrap()
                .voice;
            assert_eq!(
                &*owner
                    .owner()
                    .unwrap()
                    .store()
                    .unwrap()
                    .read_evidence(&voice.audio_ref)
                    .unwrap(),
                &wav,
                "C8_CRASH_AUDIO_BYTES"
            );
            assert_eq!(
                &*owner
                    .owner()
                    .unwrap()
                    .store()
                    .unwrap()
                    .read_evidence(voice.playhead_log_ref.as_ref().unwrap())
                    .unwrap(),
                &log,
                "C8_CRASH_LOG_BYTES"
            );
            let note = owner.owner().unwrap().voice_note(&upload.note).unwrap();
            let key = owner
                .owner()
                .unwrap()
                .start_voice_job(&note, &voice, 0)
                .unwrap();
            let issuer = owner.owner_mut().unwrap().issue_metadata_issuer().unwrap();
            let envelope = owner
                .owner()
                .unwrap()
                .metadata_envelope(kinewright_project::receipts::CommandId {
                    issuer,
                    sequence: 1,
                })
                .unwrap();
            owner.voice_jobs.insert(
                upload.note.clone(),
                crate::writer_host::voice::Job {
                    note,
                    key,
                    task: None,
                    envelope,
                    completion: Some(VoiceTranscript::Ready(vec![VoiceWord {
                        text: "make this warmer".into(),
                        start_micros: 0,
                        end_micros: 900_000,
                    }])),
                    progress: "Saving transcript".into(),
                },
            );
        }
        let saved = accepted(
            client
                .request(HostCommand::VoiceStatus {
                    note: upload.note.clone(),
                })
                .unwrap(),
        )
        .voice
        .unwrap();
        assert!(
            saved.voice.original_asr_ref.is_some(),
            "C8_ASR_AFTER_RESTART"
        );
        let generation = reopened
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .metadata_generation()
            .unwrap();
        accepted(
            client
                .request(HostCommand::VoiceStatus { note: upload.note })
                .unwrap(),
        );
        assert_eq!(
            reopened
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .metadata_generation()
                .unwrap(),
            generation,
            "C8_ASR_PUBLISHES_ONCE"
        );
    }

    #[test]
    fn c8_completed_asr_kept_on_unsettled() {
        let (_temp, host, refs) = fixture();
        let key = snapshot(&host);
        let client = host.client();
        let (upload, wav, log) = descriptor(&client, key, &refs, false);
        send(&client, &upload, &wav, &log);
        let blocker = {
            let owner = host.owner.upgrade().unwrap();
            let mut owner = owner.lock().unwrap();
            let note = owner.owner().unwrap().voice_note(&upload.note).unwrap();
            let voice = note.snapshot().unwrap().voice;
            let key = owner
                .owner()
                .unwrap()
                .start_voice_job(&note, &voice, 0)
                .unwrap();
            let issuer = owner.owner_mut().unwrap().issue_metadata_issuer().unwrap();
            let envelope = owner
                .owner()
                .unwrap()
                .metadata_envelope(kinewright_project::receipts::CommandId {
                    issuer,
                    sequence: 1,
                })
                .unwrap();
            owner.voice_jobs.insert(
                upload.note.clone(),
                crate::writer_host::voice::Job {
                    note,
                    key,
                    task: None,
                    completion: Some(VoiceTranscript::Ready(vec![VoiceWord {
                        text: "retain completed words".into(),
                        start_micros: 0,
                        end_micros: 900_000,
                    }])),
                    envelope,
                    progress: "Saving transcript".into(),
                },
            );
            owner.owner().unwrap().voice_note(&upload.note).unwrap()
        };
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let child_entered = Arc::clone(&entered);
        let child_release = Arc::clone(&release);
        let worker = thread::spawn(move || {
            blocker
                .hold_slot_probe(&child_entered, &child_release)
                .unwrap();
        });
        entered.wait();
        // Read status after the publication attempt, then release the slot on every result.
        let reply = client.request(HostCommand::VoiceStatus {
            note: upload.note.clone(),
        });
        let retained = host.owner.upgrade().unwrap().lock().unwrap().voice_jobs[&upload.note]
            .completion
            .is_some();
        release.wait();
        worker.join().unwrap();
        assert!(retained, "C8_COMPLETION_CARRIER_RETAINED");
        let _ = reply;
        let status = accepted(
            client
                .request(HostCommand::VoiceStatus { note: upload.note })
                .unwrap(),
        )
        .voice
        .unwrap();
        assert_eq!(
            status.text, "retain completed words",
            "C8_COMPLETION_RETRIED"
        );
    }
}

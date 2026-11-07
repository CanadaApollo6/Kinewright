#[test]
fn te13_fix5_accepted_voice_changed_digest_is_read_only() {
    let (_temp, host, refs) = fixture();
    let client = host.client();
    let key = snapshot(&host);
    let (mut upload, wav, log) = descriptor(&client, key, &refs, false);
    send(&client, &upload, &wav, &log);
    let before = host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .root_sha256()
        .unwrap();
    // Root generation participates in the payload digest, even with identical
    // note ID, audio and log. This conflict is the upload's own accepted note.
    upload.root_generation += 1;
    let kept = serde_json::to_value(&upload).unwrap();
    let result = client.recover_voice_upload(&mut upload).unwrap();
    if result.is_none() {
        send(&client, &upload, &wav, &log);
    }
    assert_eq!(
        host.owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .root_sha256()
            .unwrap(),
        before,
        "TE13_FIX5_NO_DUPLICATE_VOICE_PUBLICATION"
    );
    assert!(
        result.is_some(),
        "TE13_FIX5_ALREADY_ACCEPTED_NO_SECOND_SAVE"
    );
    assert_eq!(
        serde_json::to_value(&upload).unwrap(),
        kept,
        "TE13_FIX5_ACCEPTED_DESCRIPTOR_NOT_REBOUND"
    );
    assert_eq!(notes(&client).notes.len(), 1);
}
#[test]
fn te13_fix5_accepted_voice_changed_evidence_refuses() {
    let (_temp, host, refs) = fixture();
    let client = host.client();
    let key = snapshot(&host);
    let (mut upload, wav, log) = descriptor(&client, key, &refs, false);
    send(&client, &upload, &wav, &log);
    let before = notes(&client).notes[&upload.note].clone();
    upload.log.sha256 = kinewright_media::sha256_bytes(b"different retained log");
    let result = client.recover_voice_upload(&mut upload);
    assert!(result.is_err(), "TE13_FIX5_CHANGED_CAPTURE_REFUSED");
    assert_eq!(notes(&client).notes[&upload.note], before);
    assert_eq!(notes(&client).notes.len(), 1);
}
#[test]
fn te13_fix5_voice_conflict_rebinds_even_at_same_clock() {
    let (_temp, host, refs) = fixture();
    let client = host.client();
    let key = snapshot(&host);
    let (mut upload, wav, log) = descriptor(&client, key, &refs, false);
    let mut winner = upload.clone();
    winner.note = NoteId::fresh().unwrap();
    send(&client, &winner, &wav, &log);
    let grant = client
        .request(HostCommand::IssueMetadataIssuer {
            issuer: Some(upload.envelope.id.issuer.clone()),
        })
        .unwrap()
        .metadata_issuer
        .unwrap();
    upload.root_generation = grant.root_generation;
    let taken = upload.envelope.id.clone();
    assert!(client.recover_voice_upload(&mut upload).unwrap().is_none());
    assert_ne!(
        upload.envelope.id, taken,
        "TE13_FIX5_CONFLICT_ALWAYS_FRESH_ENVELOPE"
    );
    send(&client, &upload, &wav, &log);
    assert_eq!(notes(&client).notes.len(), 2, "TE13_FIX5_ONE_PER_VOICE_ID");
    assert!(client.recover_voice_upload(&mut upload).unwrap().is_some());
}
#[test]
fn te13_fix5_unpublished_voice_issuer_stays_unknown() {
    let (_temp, host, refs) = fixture();
    let client = host.client();
    let key = snapshot(&host);
    let (mut upload, _, _) = descriptor(&client, key, &refs, false);
    assert!(client.request(HostCommand::Detach).unwrap().error.is_none());
    let fresh = host
        .attach(
            "fix5-unpublished-editor".into(),
            AttachmentRole::Editor,
            host.roots.clone(),
        )
        .unwrap();
    let owner = host.owner.upgrade().unwrap();
    let before;
    {
        let mut owner = owner.lock().unwrap();
        let index = owner
            .resolve_attachment(Some(&fresh.bootstrap.auth.bearer()))
            .unwrap();
        let subject = owner.metadata_subject(index);
        let grant = serde_json::to_vec(&(
            &owner.instance,
            &owner.attachments[index].subject,
            owner.attachments[index].role,
            &owner.attachments[index].roots,
        ))
        .unwrap();
        let (issuer, _) = owner
            .owner_mut()
            .unwrap()
            .prepare_metadata_authority(&subject, None, &grant)
            .unwrap();
        let sequence = owner
            .owner()
            .unwrap()
            .metadata_issuer_sequence(&issuer)
            .unwrap();
        upload.envelope = owner
            .owner()
            .unwrap()
            .metadata_envelope(kinewright_project::receipts::CommandId { issuer, sequence })
            .unwrap();
        upload.root_generation = owner.owner().unwrap().metadata_generation().unwrap();
        before = owner.owner().unwrap().root_sha256().unwrap();
    }
    let result = fresh.recover_voice_upload(&mut upload);
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.refusal().code == "unsettled"),
        "TE13_FIX5_UNRESOLVED_IS_NOT_ABSENT"
    );
    assert_eq!(
        owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .root_sha256()
            .unwrap(),
        before
    );
}
#[test]
fn te13_fix5_accepted_voice_conflict_after_restart() {
    let (temp, initial, refs) = fixture();
    let key = snapshot(&initial);
    drop(initial);
    let mut host = ProcessHost::start(temp.root());
    let (mut upload, wav, log) = descriptor(&host.client, key, &refs, false);
    send(&host.client, &upload, &wav, &log);
    upload.root_generation += 1;
    let kept = temp.path("kept-conflicting-voice.json");
    std::fs::write(&kept, serde_json::to_vec(&upload).unwrap()).unwrap();
    host.crash();
    let restarted = ProcessHost::start(temp.root());
    let before = std::fs::read(temp.path("c0-red.kinewright")).unwrap();
    let mut restored = serde_json::from_slice(&std::fs::read(kept).unwrap()).unwrap();
    let result = restarted
        .client
        .recover_voice_upload(&mut restored)
        .unwrap();
    if result.is_none() {
        send(&restarted.client, &restored, &wav, &log);
    }
    assert_eq!(
        std::fs::read(temp.path("c0-red.kinewright")).unwrap(),
        before,
        "TE13_FIX5_RESTART_NOTE_ID_PREVENTS_SECOND_SAVE"
    );
    assert_eq!(notes(&restarted.client).notes.len(), 1);
}

#[test]
fn te13_fix5_unsettled_voice_conflict_does_not_rebind() {
    let (_temp, host, refs) = fixture();
    let client = host.client();
    let key = snapshot(&host);
    let (base, wav, log) = descriptor(&client, key.clone(), &refs, false);
    send(&client, &base, &wav, &log);
    let (mut upload, _, _) = descriptor(&client, key, &refs, false);
    let mut winner = upload.clone();
    winner.note = NoteId::fresh().unwrap();
    let host_owner = host.owner.upgrade().unwrap();
    {
        let mut owner = host_owner.lock().unwrap();
        let result = kinewright_project::io_observer::fail_once("root_parent_sync", || {
            owner.owner_mut().unwrap().metadata_command(
                &winner.envelope,
                MetadataScope::Feedback,
                &winner.update(),
            )
        });
        assert!(result.is_err(), "TE13_FIX5_REAL_UNSETTLED_VOICE");
        assert!(owner.owner().unwrap().has_unresolved_publication());
    }
    let original = serde_json::to_value(&upload).unwrap();
    let observation = client
        .request(HostCommand::VoiceReceipt {
            upload: upload.clone(),
        })
        .unwrap();
    assert!(
        observation
            .error
            .is_some_and(|error| error.code == "unsettled"),
        "TE13_FIX5_UNKNOWN_CONFLICT_STAYS_REFUSED"
    );
    let result = client.recover_voice_upload(&mut upload);
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.refusal().code == "unsettled"),
        "TE13_FIX5_UNKNOWN_CONFLICT_STAYS_REFUSED"
    );
    assert_eq!(
        serde_json::to_value(&upload).unwrap(),
        original,
        "TE13_FIX5_UNKNOWN_DESCRIPTOR_KEPT"
    );
    host_owner
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .reconcile_metadata()
        .unwrap();
    assert!(client.recover_voice_upload(&mut upload).unwrap().is_none());
    send(&client, &upload, &wav, &log);
    assert_eq!(notes(&client).notes.len(), 3);
}

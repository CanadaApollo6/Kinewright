//! Native fixture observation of the production copy owner and its actual readers.
use super::{
    protocol::{InputArgs, LoadedFixture},
    revised::Adapter,
};
use kinewright_core::{
    ClipId, Command, Core, Document, Event, HistorySnapshot, LutAsset, LutAssetId, LutAssetKind,
    LutAssetSource, Operation, TimeCode, Title, Track, TrackId, TrackKind, title_font_bytes,
};
use kinewright_project::{
    bundle::LoadedBundle,
    bundle_copy::SessionStore,
    bundle_records::{ObjectRef, RecordKind},
    io_observer,
    owned_inputs::{OwnedInputEntry, OwnedInputManifest, OwnedInputRef},
    storage_budget::StorageLimits,
};
use serde_json::{Value, json};
use std::io::Read;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
impl Adapter {
    #[allow(
        clippy::too_many_lines,
        reason = "one native first-save failure, reader handoff and reopen observation"
    )]
    pub(super) fn copy_probe(
        &self,
        args: &InputArgs,
        fixture: &LoadedFixture,
    ) -> Result<Value, String> {
        let lut = fixture.files.get(&args.input).ok_or("missing LUT bytes")?;
        let declared = fixture
            .fixture
            .inputs
            .get(&args.input)
            .ok_or("missing LUT identity")?;
        let mut document = Document::default();
        document.tracks.push(Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        });
        document.lut_assets.push(LutAsset {
            id: LutAssetId(1),
            sha256: declared.sha256.clone(),
            title: "exact fixture LUT".into(),
            kind: LutAssetKind::Cube3d,
            size: 2,
            byte_len: declared.byte_length,
            domain_min_millionths: [0; 3],
            domain_max_millionths: [1_000_000; 3],
            source: LutAssetSource::Imported {
                source_path: "/unavailable/locator.cube".into(),
            },
        });
        let core = Core::spawn(document).map_err(error)?;
        for command in [
            Command::Do(Operation::AddTitle {
                track: TrackId(1),
                at: TimeCode(0),
                duration: TimeCode(30),
                title: Title::default(),
            }),
            Command::Do(Operation::RemoveLutAsset {
                lut_asset: LutAssetId(1),
            }),
            Command::Do(Operation::DeleteClip { clip: ClipId(1) }),
            Command::Undo,
        ] {
            if !matches!(
                core.request(command).map_err(error)?,
                Event::DocumentChanged { .. }
            ) {
                return Err("fixture command rejected".into());
            }
        }
        let frozen = core.export_history().map_err(error)?;
        let mut session =
            SessionStore::create(&self.baseline.root, StorageLimits::default()).map_err(error)?;
        let session_id = session.session_id().clone();
        let mut entries = vec![];
        for (id, kind, mut bytes) in [
            ("font:Inter", RecordKind::OwnedFont, title_font_bytes()),
            ("lut:1", RecordKind::OwnedLut, lut.as_slice()),
        ] {
            let object = ObjectRef {
                sha256: kinewright_media::sha256_bytes(bytes),
                byte_length: bytes.len() as u64,
                record_kind: kind,
            };
            let sealed = session
                .store_mut()
                .admit_blob(&mut bytes, &object, &[])
                .map_err(error)?;
            entries.push(OwnedInputEntry {
                entry_id: id.into(),
                object,
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            });
            drop(sealed);
        }
        let original = OwnedInputManifest {
            project_id: None,
            session_id: Some(session_id.clone()),
            owner_epoch: session.store().owner_epoch(),
            entries,
        };
        let sealed = session.admit_inputs(&original).map_err(error)?;
        let font = OwnedInputRef {
            manifest_ref: sealed.reference.clone(),
            entry: original.entries[0].clone(),
        };
        let lut_ref = OwnedInputRef {
            manifest_ref: sealed.reference.clone(),
            entry: original.entries[1].clone(),
        };
        let mut reader = session.store().lease_input(&font).map_err(error)?;
        let decoder_unavailable = reader.certify_decoder_open().is_err();
        let monitor = session.store().usage_handle();
        let before = monitor().map_err(error)?;
        let path = self.baseline.root.join("copied.kinewright");
        let failed = io_observer::fail_once("copy_object", || {
            session.first_save(&path, &frozen, &sealed.reference)
        });
        let failure_preserved = failed.is_err()
            && !path.exists()
            && core.export_history().map_err(error)? == frozen
            && monitor().map_err(error)?.reserved_copy_bytes == 0;
        let (copy, trace) =
            io_observer::observe(|| session.first_save(&path, &frozen, &sealed.reference));
        let copy = copy.map_err(error)?;
        let overlap = monitor().map_err(error)?.reserved_copy_bytes;
        let rebound_lut = copy.rebound_input(&lut_ref).map_err(error)?;
        let destination = copy
            .into_destination()
            .map_err(|_| "copy is still pending")?;
        let project = destination.project_id().clone();
        let fresh_identity = destination
            .root()
            .ok_or("copy root")?
            .origin_session_ids
            .contains(&session_id)
            && destination.store().owner_epoch() != reader.owner_epoch();
        let stale_ref_refused = destination.store().lease_input(&font).is_err();
        let mut copied_reader = destination
            .store()
            .lease_input(&rebound_lut)
            .map_err(error)?;
        let mut copied_lut = vec![];
        copied_reader.read_to_end(&mut copied_lut).map_err(error)?;
        drop(copied_reader);
        let root_position = trace
            .events
            .iter()
            .position(|e| e.operation == "root_rename");
        let copied_before_root = !trace.overflowed
            && root_position.is_some_and(|position| {
                original.entries.iter().all(|entry| {
                    trace.events[..position].iter().any(|e| {
                        e.operation == "object_stage"
                            && e.attempted_sha256.as_ref() == Some(&entry.object.sha256)
                    })
                })
            });
        drop(sealed);
        drop(session);
        let retained = monitor().map_err(error)?;
        let mut font_bytes = vec![];
        reader.read_to_end(&mut font_bytes).map_err(error)?;
        let reader_after_handoff =
            font_bytes == title_font_bytes() && retained.physical_bytes == before.physical_bytes;
        drop(reader);
        let retired = monitor().map_err(error)?;
        drop(destination);
        let loaded = LoadedBundle::read(&path).map_err(error)?;
        let restored = Core::spawn(Document::default()).map_err(error)?;
        loaded.restore_manual(&restored).map_err(error)?;
        let actual = restored.export_history().map_err(error)?;
        let digest = |value: &HistorySnapshot| {
            serde_json::to_vec(value)
                .map(|bytes| kinewright_media::sha256_bytes(&bytes))
                .map_err(error)
        };
        Ok(
            json!({"backend":"linux-local-complete-bundle-copy","stored_bytes_equal":copied_lut==*lut && font_bytes==title_font_bytes(),"copy_before_root":copied_before_root,"failure_preserves_undo":failure_preserved,"reader_survives_handoff":reader_after_handoff,"retired_exactly_once":retired.physical_bytes==0 && retired.objects==0 && retired.reserved_copy_bytes==0 && !retired.damaged_accounting && !retired.unsettled,"new_owner_binding":fresh_identity && stale_ref_refused,"history_reopened_exactly":actual==frozen,"history_sha256":digest(&frozen)?,"reopened_history_sha256":digest(&actual)?,"project_id":project,"source_session_id":session_id,"source_physical_bytes":before.physical_bytes,"handoff_overlap_reservation_bytes":overlap,"decoder_certification_unavailable":decoder_unavailable,"io":trace}),
        )
    }
}

//! Durable production-import fixtures shared by the C0 and C2 witnesses.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, BundleWriteAdmission, ProjectBundle},
    bundle_records::{ObjectRef, RecordKind},
    editing_session::Ja1EditingSession,
    owned_inputs::{OwnedInputEntry, OwnedInputManifest, OwnedInputRef},
};
use kinewright_core::{
    Command, Core, Document, Event, HistorySnapshot, LutAsset, LutAssetId, LutAssetKind,
    LutAssetSource, Operation, TimeCode, Track, TrackId, TrackKind,
};
use std::path::{Path, PathBuf};

pub struct C0FixtureSnapshot {
    pub path: PathBuf,
    pub history: HistorySnapshot,
    pub media: OwnedInputRef,
    pub table_ref: ObjectRef,
    pub lut_ref: ObjectRef,
    pub expected_video_plane0: u8,
    pub audio_frequency: u32,
}
pub fn build_c0_fixture_pair(parent: &Path) -> Result<[C0FixtureSnapshot; 2], BundleError> {
    Ok([
        build(parent, "c0-red", "red", 440, 62, false)?,
        build(parent, "c0-blue", "blue", 880, 31, true)?,
    ])
}
fn apply(core: &Core, operation: Operation) -> Result<(), BundleError> {
    if matches!(
        core.request(Command::Do(operation))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?,
        Event::DocumentChanged { .. }
    ) {
        Ok(())
    } else {
        Err(BundleError::Corrupt("fixture Core command refused".into()))
    }
}
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "One fixture transaction preserves its independent media and LUT landmarks"
)]
fn build(
    parent: &Path,
    label: &str,
    color: &str,
    frequency: u32,
    luma: u8,
    swapped: bool,
) -> Result<C0FixtureSnapshot, BundleError> {
    let source = kinewright_media::test_support::GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("color=c={color}:s=32x24:r=24:d=0.5"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency={frequency}:sample_rate=48000:duration=0.5"),
            "-c:v",
            "libx264",
            "-qp",
            "0",
            "-preset",
            "ultrafast",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "-pix_fmt",
            "yuv420p",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ],
        "mkv",
    );
    let path = parent.join(format!("{label}.kinewright"));
    let core = Core::spawn(Document::default()).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document())?;
    let mut lut =
        String::from("TITLE \"C0 landmark\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                use std::fmt::Write as _;
                let (r, b) = if swapped { (b, r) } else { (r, b) };
                writeln!(lut, "{r} {g} {b}").map_err(|e| BundleError::Corrupt(e.to_string()))?;
            }
        }
    }
    let lut_ref = ObjectRef {
        sha256: kinewright_media::sha256_bytes(lut.as_bytes()),
        byte_length: lut.len() as u64,
        record_kind: RecordKind::OwnedLut,
    };
    let lut_pin = bundle
        .store_mut()?
        .admit_blob(&mut lut.as_bytes(), &lut_ref, &[])?;
    let lut_asset = LutAsset {
        id: LutAssetId(1),
        sha256: lut_ref.sha256.clone(),
        title: label.into(),
        kind: LutAssetKind::Cube3d,
        size: 2,
        byte_len: lut_ref.byte_length,
        domain_min_millionths: [0; 3],
        domain_max_millionths: [1_000_000; 3],
        source: LutAssetSource::Builtin { name: label.into() },
    };
    let manifest = OwnedInputManifest {
        project_id: Some(bundle.project_id().clone()),
        session_id: None,
        owner_epoch: bundle.store().owner_epoch(),
        entries: vec![OwnedInputEntry {
            entry_id: format!("lut:1:{}:{}", lut_ref.sha256, lut_ref.byte_length),
            object: lut_ref.clone(),
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        }],
    };
    let sealed = bundle.admit_inputs(&manifest)?;
    apply(&core, Operation::AddLutAsset { asset: lut_asset })?;
    bundle.save(&core, &sealed.reference)?;
    let mut session = Ja1EditingSession::from_bundle(bundle, &core, sealed.reference.clone())?;
    drop(sealed);
    drop(lut_pin);
    let (asset, _report) = session.import_owned_media(source.path())?;
    for (id, kind) in [(1, TrackKind::Video), (2, TrackKind::Audio)] {
        apply(
            &core,
            Operation::AddTrack {
                track: Track {
                    id: TrackId(id),
                    kind,
                    sync_lock: true,
                    clips: vec![],
                },
            },
        )?;
        apply(
            &core,
            Operation::AddClip {
                track: TrackId(id),
                asset: asset.id,
                at: TimeCode::ZERO,
                source: TimeCode::ZERO..asset.duration,
            },
        )?;
    }
    session.save_active()?;
    let inputs_ref = session.current_inputs_ref().clone();
    let manifest = session.store()?.read_input_manifest(&inputs_ref)?;
    let media_entry = manifest
        .entries
        .iter()
        .find(|entry| entry.matches_media_asset(&asset))
        .ok_or(BundleError::HistoryBarrierInvariant)?
        .clone();
    let history = session
        .core()
        .export_history()
        .map_err(crate::bundle::history_error)?;
    let table_ref = media_entry.chunk_table_ref()?.clone();
    let media = OwnedInputRef {
        manifest_ref: inputs_ref,
        entry: media_entry,
    };
    drop(manifest);
    drop(session);
    Ok(C0FixtureSnapshot {
        path,
        history,
        media,
        table_ref,
        lut_ref,
        expected_video_plane0: luma,
        audio_frequency: frequency,
    })
}

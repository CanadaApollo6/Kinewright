from pathlib import Path
import subprocess
path=Path('crates/kinewright-app/src/recording.rs')
original=path.read_bytes()
old=subprocess.check_output(['git','show','c7224d8b:crates/kinewright-app/src/recording.rs']).decode()
repro='''        #[test]
        fn s3_voice_fix2_competing_handle_repro() {
            let root = TempDirectory::new_in(&workspace_target_dir(), "s3-old-handle-repro");
            let (mut owner, anchor) = setup(root.root());
            let mut limits = owner.store().limits().clone();
            limits.owned_bytes = 16 << 20;
            owner.restrict_limits_probe(limits).unwrap();
            let mut a = acknowledge(capture(root.root(), &owner, anchor, 1000), &mut owner);
            let mut b = SavedAudio::reopen(&owner, &a.voice.note_id).unwrap();
            let key = a.start_job(&owner, 0).unwrap();
            let e = envelope(&mut owner);
            let usage = owner.store().usage().unwrap();
            let pressure = owner.store().reserve_copy(owner.store().limits().owned_bytes
                + owner.store().limits().metadata_bytes - (usage.physical_bytes - usage.copy_bytes)
                - usage.reserved_bytes - 512).unwrap();
            assert!(matches!(a.accept_asr(&mut owner, &e, &key, &key, asr()), Err(BundleError::Capacity(_))));
            drop(pressure);
            assert!(matches!(b.start_job(&owner, 1), Err(BundleError::Unsettled)), "S3_VOICE_OWNER_SLOT_EXCLUDES_COMPETING_JOB");
        }
'''
marker='        #[test]\n        fn s3_voice_job_identity'
assert old.count(marker)==1
try:
    path.write_text(old.replace(marker,repro+marker,1))
    result=subprocess.run(['bash','docs/reports/ja1-s3/b-evidence/c5-fix2/gate.sh','c5-fix2','repro-c722-handle','test','-p','kinewright-app','--bin','kinewright-app','recording::feedback_capture::tests::s3_voice_fix2_competing_handle_repro','--','--exact','--nocapture'])
    print('c722 App handle-path repro exit',result.returncode,flush=True)
finally:
    path.write_bytes(original)
    assert path.read_bytes()==original

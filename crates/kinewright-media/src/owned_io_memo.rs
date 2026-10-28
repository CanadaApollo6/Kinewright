//! Reissue an already verified binding without allocating a decoder cursor.
use super::{
    Arc, CertifiedIdentity, CertifiedSource, CursorReservation, File, LifetimeGuard, OwnedIoError,
    SelectedStreams, Sha256, unavailable,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn of_file(file: &File) -> Result<Self, OwnedIoError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let metadata = file.metadata().map_err(unavailable)?;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            let information = winapi_util::file::information(file).map_err(unavailable)?;
            Ok(Self {
                device: information.volume_serial_number(),
                inode: information.file_index(),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            Err(OwnedIoError::Unavailable(
                "native file identity unavailable".into(),
            ))
        }
    }
}

/// Opaque evidence of an earlier complete verification, with no descriptor or pin.
/// Only a successfully verified source can issue this memo.
#[derive(Clone)]
pub struct SourceAdmissionMemo {
    native: FileIdentity,
    identity: CertifiedIdentity,
    chunks_digest: [u8; 32],
    selected: Option<SelectedStreams>,
}

fn chunks_digest(chunks: &[[u8; 32]]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for chunk in chunks {
        digest.update(chunk);
    }
    digest.finalize()
}

pub(super) fn mark_verified(source: &CertifiedSource) -> Result<(), OwnedIoError> {
    let native = FileIdentity::of_file(&source.0.file)?;
    let _ = source.0.verified_identity.set(native);
    Ok(())
}

impl CertifiedSource {
    /// Retain complete verification without retaining this source's leases or caches.
    pub fn admission_memo(&self) -> Result<SourceAdmissionMemo, OwnedIoError> {
        let native = self.0.verified_identity.get().copied().ok_or_else(|| {
            OwnedIoError::InvalidCertificate("source has no complete verification".into())
        })?;
        Ok(SourceAdmissionMemo {
            native,
            identity: self.identity(),
            chunks_digest: chunks_digest(&self.0.chunks),
            selected: self.1,
        })
    }
}

impl SourceAdmissionMemo {
    /// Reissue this exact native source and chunk binding under the receiving leases.
    /// Every subsequent cursor still reserves capacity and verifies every used chunk.
    pub fn reissue(
        &self,
        file: File,
        chunks: Arc<[[u8; 32]]>,
        lifetime: LifetimeGuard,
        reserve_cursor: Arc<CursorReservation>,
    ) -> Result<CertifiedSource, OwnedIoError> {
        if FileIdentity::of_file(&file)? != self.native
            || chunks_digest(&chunks) != self.chunks_digest
        {
            return Err(OwnedIoError::InvalidCertificate(
                "admission memo native/chunk binding".into(),
            ));
        }
        let mut source = CertifiedSource::new(
            file,
            self.identity.source_sha256,
            self.identity.source_length,
            self.identity.table_sha256,
            chunks,
            lifetime,
            reserve_cursor,
        )?;
        let _ = source.0.verified_identity.set(self.native);
        source.1 = self.selected;
        Ok(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDirectory;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn digest(bytes: &[u8]) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(bytes);
        hash.finalize()
    }

    fn fixture() -> (TempDirectory, CertifiedSource, Arc<[[u8; 32]]>) {
        let root = TempDirectory::new("source-memo");
        let bytes = vec![42; 1234];
        std::fs::write(root.path("source"), &bytes).unwrap();
        let chunks: Arc<[[u8; 32]]> = vec![digest(&bytes)].into();
        let source = CertifiedSource::verify(
            File::open(root.path("source")).unwrap(),
            digest(&bytes),
            bytes.len() as u64,
            [7; 32],
            Arc::clone(&chunks),
            Arc::new(()),
            Arc::new(|_| Ok(Arc::new(()))),
        )
        .unwrap();
        (root, source, chunks)
    }

    #[test]
    fn s3_memo_reissue_defers_cursor_capacity_and_keeps_actual_use_verification() {
        let (root, source, chunks) = fixture();
        let source = source.with_selected_streams(Some(1), None).unwrap();
        let memo = source.admission_memo().unwrap();
        let exhausted = Arc::new(AtomicBool::new(true));
        let quota = Arc::clone(&exhausted);
        let reissued = memo
            .reissue(
                File::open(root.path("source")).unwrap(),
                chunks,
                Arc::new(()),
                Arc::new(move |_| {
                    if quota.load(Ordering::Acquire) {
                        Err("all decoder bytes reserved".into())
                    } else {
                        Ok(Arc::new(()))
                    }
                }),
            )
            .expect("C2_MEMO_REISSUE_WITHOUT_CURSOR_CAPACITY");
        assert!(reissued.cursor(Arc::new(AtomicBool::new(false))).is_err());
        assert!(
            reissued
                .clone()
                .with_selected_streams(Some(2), None)
                .is_err(),
            "memo cannot widen selected streams"
        );
        exhausted.store(false, Ordering::Release);
        File::options()
            .write(true)
            .open(root.path("source"))
            .unwrap()
            .write_all(&[0xff])
            .unwrap();
        let mut cursor = reissued.cursor(Arc::new(AtomicBool::new(false))).unwrap();
        assert!(
            cursor.read(&mut [0; 1]).is_err(),
            "C2_MEMO_ACTUAL_USE_RECHECKS_CHUNKS"
        );
    }

    #[test]
    fn s3_memo_cannot_rebind_an_identical_replacement_descriptor_or_chunk_table() {
        let (root, source, chunks) = fixture();
        let memo = source.admission_memo().unwrap();
        std::fs::copy(root.path("source"), root.path("replacement")).unwrap();
        assert!(
            memo.reissue(
                File::open(root.path("replacement")).unwrap(),
                Arc::clone(&chunks),
                Arc::new(()),
                Arc::new(|_| Ok(Arc::new(())))
            )
            .is_err(),
            "C2_MEMO_NATIVE_IDENTITY_BINDING"
        );
        let replacement = vec![54; 1234];
        std::fs::write(root.path("source"), &replacement).unwrap();
        let forged = memo.reissue(
            File::open(root.path("source")).unwrap(),
            vec![digest(&replacement)].into(),
            Arc::new(()),
            Arc::new(|_| Ok(Arc::new(()))),
        );
        if let Ok(source) = &forged {
            let mut cursor = source.cursor(Arc::new(AtomicBool::new(false))).unwrap();
            let mut byte = [0];
            cursor.read(&mut byte).unwrap();
            eprintln!("C2_MEMO_FORGED_BYTES: {byte:?}");
        }
        assert!(forged.is_err(), "C2_MEMO_CHUNK_TABLE_BINDING");
        let unchecked = CertifiedSource::new(
            File::open(root.path("source")).unwrap(),
            source.identity().source_sha256,
            source.identity().source_length,
            [7; 32],
            chunks,
            Arc::new(()),
            Arc::new(|_| Ok(Arc::new(()))),
        )
        .unwrap();
        assert!(
            unchecked.admission_memo().is_err(),
            "only complete verification issues a memo"
        );
    }
}

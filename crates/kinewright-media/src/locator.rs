//! Thread-affine locator execution: queued work retains its source authority,
//! and every native locator open rechecks it inside the operation's lease.
use kinewright_core::{LegacyMediaGrant, LegacyMediaWork, MediaError};
use std::{cell::RefCell, marker::PhantomData, rc::Rc};

/// The receiver cannot extract the payload without entering its source scope.
/// Only running jobs acquire leases: held queued jobs never stall activation.
pub(crate) struct LocatorJob<T> {
    grant: LegacyMediaGrant,
    payload: T,
}
impl<T> LocatorJob<T> {
    pub(crate) const fn new(grant: LegacyMediaGrant, payload: T) -> Self {
        Self { grant, payload }
    }
    pub(crate) fn run<C, R>(
        self,
        context: &mut C,
        process: impl FnOnce(&mut C, T) -> R,
        refused: impl FnOnce(&mut C, T, MediaError) -> R,
    ) -> R {
        match LocatorAccess::enter(&self.grant) {
            Ok(_access) => process(context, self.payload),
            Err(error) => refused(context, self.payload, error),
        }
    }
}

thread_local! {
    static AUTHORITY: RefCell<Vec<LegacyMediaGrant>> = const { RefCell::new(Vec::new()) };
}

/// The guard cannot move to another thread. Native decoder calls and hashing
/// stay on this thread; queued jobs must enter again on their worker thread.
pub(crate) struct LocatorAccess {
    _work: LegacyMediaWork,
    _thread: PhantomData<Rc<()>>,
}

impl LocatorAccess {
    pub(crate) fn enter(grant: &LegacyMediaGrant) -> Result<Self, MediaError> {
        let work = grant.begin()?;
        AUTHORITY.with(|authority| authority.borrow_mut().push(grant.clone()));
        Ok(Self {
            _work: work,
            _thread: PhantomData,
        })
    }
}
impl Drop for LocatorAccess {
    fn drop(&mut self) {
        AUTHORITY.with(|authority| {
            authority.borrow_mut().pop();
        });
    }
}

/// Standalone media utilities have no project scope. Within a session scope,
/// even a later open in a mixer, probe or hash must retain the original grant.
pub(crate) fn check() -> Result<(), MediaError> {
    AUTHORITY.with(|authority| {
        for grant in authority.borrow().iter() {
            grant.check()?;
        }
        Ok(())
    })
}

/// Retain the authority when a lazy decoder outlives its dispatch scope.
pub(crate) fn capture() -> Option<LegacyMediaGrant> {
    AUTHORITY.with(|authority| authority.borrow().last().cloned())
}
pub(crate) fn enter_retained(
    grant: Option<&LegacyMediaGrant>,
) -> Result<Option<LocatorAccess>, MediaError> {
    grant.map(LocatorAccess::enter).transpose()
}

pub(crate) fn before_decoder_open() -> Result<(), MediaError> {
    check()?;
    #[cfg(feature = "test-util")]
    if let Some(grant) = capture() {
        grant.record_locator_open();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::GeneratedMedia;
    use kinewright_core::{Rational, TimeCode};

    #[test]
    fn c2_r3_in_flight_scope_refuses_each_later_locator_open() {
        crate::initialize_ffmpeg().unwrap();
        let source = GeneratedMedia::ffmpeg(
            "c2-r3-receiving",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=blue:s=64x64:r=30",
                "-t",
                "0.2",
                "-c:v",
                "mpeg4",
            ],
            "avi",
        );
        let origin = kinewright_core::MediaSourceOrigin::new_project();
        let grant = origin.legacy_media().unwrap();
        let scope = LocatorAccess::enter(&grant).unwrap();
        assert!(
            crate::decode::thumbnail(
                source.path(),
                Rational::new(30, 1).unwrap(),
                TimeCode::ZERO,
                32
            )
            .is_ok()
        );
        let before = grant.locator_opens();
        assert!(before > 0, "legacy control must open a real decoder");
        origin.revoke_legacy_media();
        assert!(
            !grant.is_quiescent(),
            "the active scope must hold publication"
        );
        assert_eq!(
            crate::decode::thumbnail(
                source.path(),
                Rational::new(30, 1).unwrap(),
                TimeCode::ZERO,
                32
            )
            .unwrap_err(),
            MediaError::MediaAdmissionRequired
        );
        assert_eq!(
            crate::sha256::sha256_file(source.path()).unwrap_err(),
            MediaError::MediaAdmissionRequired
        );
        assert_eq!(
            grant.locator_opens(),
            before,
            "C2_R3_RECEIVING_BOUNDARY_NO_LATER_OPEN"
        );
        drop(scope);
        assert!(grant.is_quiescent());
        assert!(LocatorAccess::enter(&grant).is_err());
    }
    #[test]
    fn c2_r3_retained_audio_decoder_refuses_after_scope_and_revocation() {
        crate::initialize_ffmpeg().unwrap();
        let source = GeneratedMedia::ffmpeg(
            "c2-r3-audio",
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=16000",
                "-t",
                "0.2",
                "-c:a",
                "pcm_s16le",
            ],
            "wav",
        );
        let origin = kinewright_core::MediaSourceOrigin::new_project();
        let grant = origin.legacy_media().unwrap();
        let scope = LocatorAccess::enter(&grant).unwrap();
        let mut decoder =
            crate::audio::AudioDecoder::open(source.path(), 16000, 1, 0, 3200).unwrap();
        drop(scope);
        assert!(grant.is_quiescent());
        origin.revoke_legacy_media();
        assert_eq!(
            decoder.next_chunk().unwrap_err(),
            MediaError::MediaAdmissionRequired,
            "C2_R3_RETAINED_AUDIO_NO_SAMPLES"
        );
    }
}

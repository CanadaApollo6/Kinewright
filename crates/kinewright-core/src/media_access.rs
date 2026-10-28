//! Authority to reopen locators belongs to a legacy project source, not a Document.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A revocable grant issued by a source that has no owned Studio envelope.
#[derive(Clone, Debug)]
pub struct LegacyMediaGrant(Arc<AtomicBool>);

impl LegacyMediaGrant {
    /// Refuse a request whose source has since become owned.
    /// # Errors
    /// Returns the named media-admission refusal after revocation.
    pub fn check(&self) -> Result<(), crate::MediaError> {
        if self.0.load(Ordering::Acquire) {
            Err(crate::MediaError::MediaAdmissionRequired)
        } else {
            Ok(())
        }
    }
}

/// Retained source provenance. It cannot be reconstructed from a Document.
#[derive(Clone, Debug)]
pub struct MediaSourceOrigin {
    owned: bool,
    legacy: Option<LegacyMediaGrant>,
}

impl MediaSourceOrigin {
    /// Start an independent, unsaved legacy project.
    #[must_use]
    pub fn new_project() -> Self {
        Self {
            owned: false,
            legacy: Some(LegacyMediaGrant(Arc::new(AtomicBool::new(false)))),
        }
    }

    /// Classify the retained envelope before its Studio field is removed.
    #[must_use]
    pub fn inspect(source: &serde_json::Value) -> Self {
        if source
            .as_object()
            .is_some_and(|object| object.contains_key("studio"))
        {
            Self {
                owned: true,
                legacy: None,
            }
        } else {
            Self::new_project()
        }
    }

    #[must_use]
    pub const fn is_owned(&self) -> bool {
        self.owned
    }

    #[must_use]
    pub fn legacy_media(&self) -> Option<LegacyMediaGrant> {
        self.legacy
            .as_ref()
            .filter(|grant| grant.check().is_ok())
            .cloned()
    }

    /// Revoke copies already handed to background work before activation.
    pub fn revoke_legacy_media(&self) {
        if let Some(grant) = &self.legacy {
            grant.0.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_envelopes_never_issue_a_legacy_grant_even_without_activation() {
        for studio in [
            serde_json::Value::Null,
            serde_json::json!({"owned_media": {}}),
        ] {
            let origin = MediaSourceOrigin::inspect(&serde_json::json!({"studio": studio}));
            assert!(origin.is_owned());
            assert!(origin.legacy_media().is_none());
        }
    }

    #[test]
    fn activation_revokes_grants_already_captured_by_background_work() {
        let origin = MediaSourceOrigin::new_project();
        let captured = origin.legacy_media().unwrap();
        assert!(captured.check().is_ok());
        origin.revoke_legacy_media();
        assert_eq!(
            captured.check(),
            Err(crate::MediaError::MediaAdmissionRequired)
        );
        assert!(origin.legacy_media().is_none());
    }
}

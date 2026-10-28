//! One immutable PF1 scene admission. A missing certified asset is an error,
//! never permission to open the document's locator.
use super::{CertifiedSource, LifetimeGuard};
use crate::LutLibrary;
use kinewright_core::{AssetId, Document, MediaError};
use std::{collections::BTreeMap, sync::Arc};

pub struct PlaybackInputs {
    pub(crate) document: Arc<Document>,
    pub(crate) sources: BTreeMap<AssetId, CertifiedSource>,
    pub(crate) lut: Arc<LutLibrary>,
    pub(crate) lifetime: LifetimeGuard,
}
impl PlaybackInputs {
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }
    /// Observe one bounded chunk from the production playback mixer.
    /// # Errors
    /// Returns the mixer's source, verification, or decoding refusal.
    #[cfg(any(test, feature = "test-util"))]
    pub fn test_audio_samples(&self) -> Result<Vec<f32>, MediaError> {
        crate::audio::test_certified_mix_samples(self)
    }
    /// Retarget editorial changes without reopening an unchanged namespace.
    /// New media, interpretation, or LUT assets require fresh owner admission.
    /// # Errors
    /// Refuses changes that require a new certified input namespace.
    pub fn with_document(&self, document: Arc<Document>) -> Result<Self, MediaError> {
        if document.media_pool != self.document.media_pool
            || document.lut_assets != self.document.lut_assets
        {
            return Err(MediaError::Backend(
                "playback namespace changed; owner admission required".into(),
            ));
        }
        Self::admit(
            document,
            self.sources.clone(),
            Arc::clone(&self.lut),
            Arc::clone(&self.lifetime),
        )
    }
    /// The project owner supplies manifest-bound sources and resource leases.
    /// The final media boundary checks that every document asset has its exact
    /// full fingerprint before it can be installed as a scene.
    /// # Errors
    /// Refuses missing inputs or a fingerprint or LUT-library conflict.
    pub fn admit(
        document: Arc<Document>,
        sources: BTreeMap<AssetId, CertifiedSource>,
        lut: Arc<LutLibrary>,
        lifetime: LifetimeGuard,
    ) -> Result<Self, MediaError> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for asset in &document.media_pool {
            let source = sources
                .get(&asset.id)
                .ok_or_else(|| MediaError::Backend("missing certified playback input".into()))?;
            let identity = source.identity();
            let digest = identity
                .source_sha256
                .into_iter()
                .flat_map(|byte| {
                    [
                        char::from(HEX[usize::from(byte >> 4)]),
                        char::from(HEX[usize::from(byte & 15)]),
                    ]
                })
                .collect::<String>();
            if asset.source_fingerprint.content_sha256.as_deref() != Some(digest.as_str())
                || asset.source_fingerprint.byte_len != Some(identity.source_length)
            {
                return Err(MediaError::Backend("playback fingerprint conflict".into()));
            }
        }
        for asset in &document.lut_assets {
            if !lut
                .entries()
                .any(|(id, hash, _)| id == asset.id && hash == asset.sha256)
            {
                return Err(MediaError::Backend("playback LUT context conflict".into()));
            }
        }
        Ok(Self {
            document,
            sources,
            lut,
            lifetime,
        })
    }
    pub(crate) fn source(&self, id: AssetId) -> Result<CertifiedSource, MediaError> {
        self.sources
            .get(&id)
            .cloned()
            .ok_or_else(|| MediaError::Backend("missing certified playback input".into()))
    }
}

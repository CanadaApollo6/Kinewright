//! One immutable PF1 scene admission. A missing certified asset is an error,
//! never permission to open the document's locator.
use super::{CertifiedSource, LifetimeGuard, ReadAuthority};
use crate::LutLibrary;
use kinewright_core::{AssetId, Document, MediaError};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub(crate) struct PlaybackAuthority(Arc<()>);
impl PartialEq for PlaybackAuthority {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for PlaybackAuthority {}
impl std::hash::Hash for PlaybackAuthority {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&Arc::as_ptr(&self.0), state);
    }
}

pub struct PlaybackInputs {
    pub(crate) read_authority: Option<ReadAuthority>,
    pub(crate) authority: PlaybackAuthority,
    pub(crate) document: Arc<Document>,
    pub(crate) sources: BTreeMap<AssetId, CertifiedSource>,
    pub(crate) lut: Arc<LutLibrary>,
    pub(crate) lifetime: LifetimeGuard,
}
impl PlaybackInputs {
    pub fn with_read_authority(mut self, authority: ReadAuthority) -> Result<Self, MediaError> {
        authority.check()?;
        self.read_authority = Some(authority);
        Ok(self)
    }
    pub fn check_read_authority(&self) -> Result<(), MediaError> {
        self.read_authority
            .as_ref()
            .map_or(Ok(()), ReadAuthority::check)
    }

    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }
    /// Observe one bounded chunk from the production playback mixer.
    /// # Errors
    /// Returns the mixer's source, verification, or decoding refusal.
    #[cfg(any(test, feature = "test-util"))]
    pub fn test_audio_samples(&self) -> Result<Vec<f32>, MediaError> {
        self.check_read_authority()?;
        crate::audio::test_certified_mix_samples(self)
    }
    /// Retarget editorial changes without reopening an unchanged namespace.
    /// New media, interpretation, or LUT assets require fresh owner admission.
    /// # Errors
    /// Refuses changes that require a new certified input namespace.
    pub fn with_document(&self, document: Arc<Document>) -> Result<Self, MediaError> {
        self.check_read_authority()?;
        if document.media_pool != self.document.media_pool
            || document.lut_assets != self.document.lut_assets
        {
            return Err(MediaError::Backend(
                "playback namespace changed; owner admission required".into(),
            ));
        }
        let mut inputs = Self::admit(
            document,
            self.sources.clone(),
            Arc::clone(&self.lut),
            Arc::clone(&self.lifetime),
        )?;
        inputs.authority = self.authority.clone();
        inputs.read_authority.clone_from(&self.read_authority);
        Ok(inputs)
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
            source.check_read_authority()?;
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
            read_authority: None,
            authority: PlaybackAuthority(Arc::new(())),
            document,
            sources,
            lut,
            lifetime,
        })
    }
    pub(crate) fn source(&self, id: AssetId) -> Result<CertifiedSource, MediaError> {
        self.check_read_authority()?;
        self.sources
            .get(&id)
            .cloned()
            .ok_or_else(|| MediaError::Backend("missing certified playback input".into()))
    }
}

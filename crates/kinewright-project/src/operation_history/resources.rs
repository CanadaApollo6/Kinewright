use kinewright_core::{ClipContent, Document, MediaSourceFingerprint, Operation};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Resource {
    Media {
        asset: u64,
        sha256: Option<String>,
        byte_len: Option<u64>,
    },
    Lut {
        sha256: String,
        byte_len: u64,
    },
    TitleFont,
}
fn media(asset: u64, fingerprint: &MediaSourceFingerprint) -> Resource {
    Resource::Media {
        asset,
        sha256: fingerprint.content_sha256.clone(),
        byte_len: fingerprint.byte_len,
    }
}
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Resources {
    pub(super) baseline: BTreeSet<Resource>,
    pub(super) counts: BTreeMap<Resource, u64>,
}
impl Resources {
    // Media and LUT bytes belong to their document asset tables. The embedded
    // title font is the clip-local resource, including implicitly created clips.
    pub(crate) fn clip_content(content: &ClipContent) -> Option<Resource> {
        match content {
            ClipContent::Title(_) => Some(Resource::TitleFont),
            ClipContent::Media
            | ClipContent::Freeze(_)
            | ClipContent::Adjustment
            | ClipContent::Solid(_) => None,
        }
    }
    #[must_use]
    pub fn document(document: &Document) -> BTreeSet<Resource> {
        super::owner::resource_document_scans(1);
        let mut result: BTreeSet<_> = document
            .media_pool
            .iter()
            .map(|a| media(a.id.0, &a.source_fingerprint))
            .collect();
        result.extend(document.lut_assets.iter().map(|a| Resource::Lut {
            sha256: a.sha256.clone(),
            byte_len: a.byte_len,
        }));
        result.extend(
            document
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .filter_map(|clip| Self::clip_content(&clip.content)),
        );
        result
    }
    #[must_use]
    pub fn payload(
        operations: &[Operation],
        identities: &[kinewright_core::OperationIdentityEffect],
    ) -> BTreeSet<Resource> {
        let mut result = BTreeSet::new();
        for operation in operations {
            match operation {
                Operation::AddAsset { asset } => {
                    result.insert(media(asset.id.0, &asset.source_fingerprint));
                }
                Operation::RelinkAsset {
                    asset, candidate, ..
                } => {
                    result.insert(media(asset.0, &candidate.fingerprint));
                }
                Operation::AddLutAsset { asset } => {
                    result.insert(Resource::Lut {
                        sha256: asset.sha256.clone(),
                        byte_len: asset.byte_len,
                    });
                }
                _ => {}
            }
        }
        result.extend(
            identities
                .iter()
                .flat_map(|identity| &identity.clip_contents)
                .filter_map(|(_, content)| Self::clip_content(content)),
        );
        result
    }
    #[must_use]
    pub fn new(document: &Document) -> Self {
        Self {
            baseline: Self::document(document),
            counts: BTreeMap::new(),
        }
    }
    pub fn values(&self) -> impl Iterator<Item = &Resource> {
        self.baseline
            .iter()
            .chain(self.counts.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
    }
    #[must_use]
    pub fn contains(&self, resource: &Resource) -> bool {
        self.baseline.contains(resource) || self.counts.contains_key(resource)
    }
    pub(super) fn add(&mut self, resources: &BTreeSet<Resource>) {
        for resource in resources {
            *self.counts.entry(resource.clone()).or_default() += 1;
        }
    }
    pub(super) fn remove(&mut self, resources: &BTreeSet<Resource>) {
        for resource in resources {
            if let Some(count) = self.counts.get_mut(resource) {
                *count -= 1;
                if *count == 0 {
                    self.counts.remove(resource);
                }
            }
        }
    }
}

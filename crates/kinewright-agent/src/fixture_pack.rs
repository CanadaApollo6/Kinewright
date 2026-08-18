//! Licensed, immutable media packs for reproducible editing evaluations.

use std::{
    collections::BTreeSet,
    env,
    fs::{self, File},
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const SCHEMA_VERSION: u32 = 1;
const DEFAULT_CACHE_ROOT: &str = "target/eval-fixtures";
const USER_AGENT: &str =
    "Kinewright-Eval-Fixture/0.1 (+https://github.com/CanadaApollo6/Kinewright)";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixturePackManifest {
    pub schema_version: u32,
    pub pack_id: String,
    pub title: String,
    pub description: String,
    pub assets: Vec<FixtureAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureAsset {
    pub id: String,
    pub file_name: String,
    pub download_url: String,
    pub source_page_url: String,
    pub bytes: u64,
    pub sha256: String,
    pub license: FixtureLicense,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureLicense {
    pub spdx_id: String,
    pub name: String,
    pub url: String,
    pub attribution: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FixturePackReport {
    pub pack_id: String,
    pub cache_root: PathBuf,
    pub downloaded: Vec<String>,
    pub already_present: Vec<String>,
}

#[derive(Debug, Error)]
pub enum FixturePackError {
    #[error("could not read fixture manifest {path}: {source}")]
    ReadManifest { path: PathBuf, source: io::Error },
    #[error("could not parse fixture manifest {path}: {source}")]
    ParseManifest {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("invalid fixture manifest: {0}")]
    InvalidManifest(String),
    #[error("fixture asset {0:?} does not exist in the manifest")]
    UnknownAsset(String),
    #[error(
        "fixture asset {asset:?} is missing at {path}; run kinewright-eval --prepare-fixtures with the pack manifest first"
    )]
    MissingAsset { asset: String, path: PathBuf },
    #[error("fixture asset {asset:?} failed integrity verification at {path}: {detail}")]
    Integrity {
        asset: String,
        path: PathBuf,
        detail: String,
    },
    #[error("could not create fixture cache {path}: {source}")]
    CreateCache { path: PathBuf, source: io::Error },
    #[error("could not download fixture asset {asset:?} from {url}: {source}")]
    Download {
        asset: String,
        url: String,
        source: reqwest::Error,
    },
    #[error("could not write fixture asset {asset:?} at {path}: {source}")]
    WriteAsset {
        asset: String,
        path: PathBuf,
        source: io::Error,
    },
    #[error("could not hash fixture asset {asset:?} at {path}: {source}")]
    HashAsset {
        asset: String,
        path: PathBuf,
        source: kinewright_core::MediaError,
    },
}

impl FixturePackManifest {
    /// Load and validate one checked-in fixture manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, parsed, or violates the
    /// immutable fixture-pack contract.
    pub fn load(path: &Path) -> Result<Self, FixturePackError> {
        let bytes = fs::read(path).map_err(|source| FixturePackError::ReadManifest {
            path: path.to_path_buf(),
            source,
        })?;
        let manifest = serde_json::from_slice::<Self>(&bytes).map_err(|source| {
            FixturePackError::ParseManifest {
                path: path.to_path_buf(),
                source,
            }
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Parse and validate an embedded fixture manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when the JSON or manifest contract is invalid.
    pub fn from_json(json: &str) -> Result<Self, FixturePackError> {
        let manifest = serde_json::from_str::<Self>(json).map_err(|source| {
            FixturePackError::ParseManifest {
                path: PathBuf::from("<embedded>"),
                source,
            }
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Verify every asset and return the cache path for a named asset.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown, missing, truncated, or changed asset.
    pub fn verified_asset(
        &self,
        cache_root: &Path,
        asset_id: &str,
    ) -> Result<PathBuf, FixturePackError> {
        let asset = self
            .assets
            .iter()
            .find(|asset| asset.id == asset_id)
            .ok_or_else(|| FixturePackError::UnknownAsset(asset_id.to_owned()))?;
        let path = self.asset_path(cache_root, asset);
        verify_asset(asset, &path)?;
        Ok(path)
    }

    /// Verify every cached asset without network access.
    ///
    /// # Errors
    ///
    /// Returns the first missing or changed asset.
    pub fn verify(&self, cache_root: &Path) -> Result<FixturePackReport, FixturePackError> {
        for asset in &self.assets {
            verify_asset(asset, &self.asset_path(cache_root, asset))?;
        }
        Ok(FixturePackReport {
            pack_id: self.pack_id.clone(),
            cache_root: cache_root.to_path_buf(),
            downloaded: Vec::new(),
            already_present: self.assets.iter().map(|asset| asset.id.clone()).collect(),
        })
    }

    /// Explicitly download missing assets, then verify exact length and SHA-256.
    /// Existing invalid files are never overwritten.
    ///
    /// # Errors
    ///
    /// Returns an error when cache creation, download, writing, or integrity
    /// verification fails.
    pub fn prepare(&self, cache_root: &Path) -> Result<FixturePackReport, FixturePackError> {
        let pack_root = cache_root.join(&self.pack_id);
        fs::create_dir_all(&pack_root).map_err(|source| FixturePackError::CreateCache {
            path: pack_root,
            source,
        })?;
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_mins(20))
            .build()
            .map_err(|source| FixturePackError::Download {
                asset: self.pack_id.clone(),
                url: "<client>".to_owned(),
                source,
            })?;
        let mut downloaded = Vec::new();
        let mut already_present = Vec::new();
        for asset in &self.assets {
            let destination = self.asset_path(cache_root, asset);
            if destination.exists() {
                verify_asset(asset, &destination)?;
                already_present.push(asset.id.clone());
                continue;
            }
            let temporary = temporary_path(&destination);
            let result = download_asset(&client, asset, &temporary)
                .and_then(|()| verify_asset(asset, &temporary))
                .and_then(|()| {
                    fs::rename(&temporary, &destination).map_err(|source| {
                        FixturePackError::WriteAsset {
                            asset: asset.id.clone(),
                            path: destination.clone(),
                            source,
                        }
                    })
                });
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            result?;
            downloaded.push(asset.id.clone());
        }
        Ok(FixturePackReport {
            pack_id: self.pack_id.clone(),
            cache_root: cache_root.to_path_buf(),
            downloaded,
            already_present,
        })
    }

    fn validate(&self) -> Result<(), FixturePackError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(FixturePackError::InvalidManifest(format!(
                "schema_version must be {SCHEMA_VERSION}, observed {}",
                self.schema_version
            )));
        }
        validate_identifier("pack_id", &self.pack_id)?;
        if self.title.trim().is_empty() || self.description.trim().is_empty() {
            return Err(FixturePackError::InvalidManifest(
                "title and description must be non-empty".to_owned(),
            ));
        }
        if self.assets.is_empty() {
            return Err(FixturePackError::InvalidManifest(
                "assets must be non-empty".to_owned(),
            ));
        }
        let mut ids = BTreeSet::new();
        let mut file_names = BTreeSet::new();
        for asset in &self.assets {
            validate_identifier("asset id", &asset.id)?;
            if !ids.insert(asset.id.as_str()) {
                return Err(FixturePackError::InvalidManifest(format!(
                    "duplicate asset id {:?}",
                    asset.id
                )));
            }
            validate_relative_file_name(&asset.file_name)?;
            if !file_names.insert(asset.file_name.as_str()) {
                return Err(FixturePackError::InvalidManifest(format!(
                    "duplicate file_name {:?}",
                    asset.file_name
                )));
            }
            if !is_https(&asset.download_url)
                || !is_https(&asset.source_page_url)
                || !is_https(&asset.license.url)
            {
                return Err(FixturePackError::InvalidManifest(format!(
                    "asset {:?} download, source, and license URLs must use https",
                    asset.id
                )));
            }
            if asset.bytes == 0 {
                return Err(FixturePackError::InvalidManifest(format!(
                    "asset {:?} bytes must be positive",
                    asset.id
                )));
            }
            if asset.sha256.len() != 64
                || !asset
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(FixturePackError::InvalidManifest(format!(
                    "asset {:?} sha256 must be 64 lowercase hexadecimal characters",
                    asset.id
                )));
            }
            if asset.license.spdx_id.trim().is_empty()
                || asset.license.name.trim().is_empty()
                || asset.license.attribution.trim().is_empty()
            {
                return Err(FixturePackError::InvalidManifest(format!(
                    "asset {:?} license fields must be non-empty",
                    asset.id
                )));
            }
        }
        Ok(())
    }

    fn asset_path(&self, cache_root: &Path, asset: &FixtureAsset) -> PathBuf {
        cache_root.join(&self.pack_id).join(&asset.file_name)
    }
}

#[must_use]
pub fn fixture_cache_root() -> PathBuf {
    env::var_os("KINEWRIGHT_EVAL_FIXTURE_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(DEFAULT_CACHE_ROOT)
        },
        PathBuf::from,
    )
}

fn validate_identifier(field: &str, value: &str) -> Result<(), FixturePackError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(FixturePackError::InvalidManifest(format!(
            "{field} {value:?} must contain only ASCII letters, digits, hyphens, or underscores"
        )));
    }
    Ok(())
}

fn validate_relative_file_name(value: &str) -> Result<(), FixturePackError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.components().count() != 1
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(FixturePackError::InvalidManifest(format!(
            "file_name {value:?} must be one safe relative file name"
        )));
    }
    Ok(())
}

fn is_https(value: &str) -> bool {
    value.starts_with("https://") && value.len() > "https://".len()
}

fn verify_asset(asset: &FixtureAsset, path: &Path) -> Result<(), FixturePackError> {
    let metadata = path
        .metadata()
        .map_err(|_source| FixturePackError::MissingAsset {
            asset: asset.id.clone(),
            path: path.to_path_buf(),
        })?;
    if !metadata.is_file() || metadata.len() != asset.bytes {
        return Err(FixturePackError::Integrity {
            asset: asset.id.clone(),
            path: path.to_path_buf(),
            detail: format!(
                "expected a {}-byte regular file, observed {} bytes",
                asset.bytes,
                metadata.len()
            ),
        });
    }
    let hash =
        kinewright_media::sha256_file(path).map_err(|source| FixturePackError::HashAsset {
            asset: asset.id.clone(),
            path: path.to_path_buf(),
            source,
        })?;
    if hash != asset.sha256 {
        return Err(FixturePackError::Integrity {
            asset: asset.id.clone(),
            path: path.to_path_buf(),
            detail: format!("expected sha256 {}, observed {hash}", asset.sha256),
        });
    }
    Ok(())
}

fn download_asset(
    client: &reqwest::blocking::Client,
    asset: &FixtureAsset,
    destination: &Path,
) -> Result<(), FixturePackError> {
    let mut response = client
        .get(&asset.download_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|source| FixturePackError::Download {
            asset: asset.id.clone(),
            url: asset.download_url.clone(),
            source,
        })?;
    let mut file = File::create(destination).map_err(|source| FixturePackError::WriteAsset {
        asset: asset.id.clone(),
        path: destination.to_path_buf(),
        source,
    })?;
    io::copy(&mut response, &mut file).map_err(|source| FixturePackError::WriteAsset {
        asset: asset.id.clone(),
        path: destination.to_path_buf(),
        source,
    })?;
    file.flush()
        .map_err(|source| FixturePackError::WriteAsset {
            asset: asset.id.clone(),
            path: destination.to_path_buf(),
            source,
        })?;
    Ok(())
}

fn temporary_path(destination: &Path) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    destination.with_extension(format!("part-{}-{nonce}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(bytes: u64, sha256: &str) -> FixturePackManifest {
        FixturePackManifest {
            schema_version: 1,
            pack_id: "test-pack".to_owned(),
            title: "Test pack".to_owned(),
            description: "Fixture manifest test".to_owned(),
            assets: vec![FixtureAsset {
                id: "clip".to_owned(),
                file_name: "clip.bin".to_owned(),
                download_url: "https://example.com/clip.bin".to_owned(),
                source_page_url: "https://example.com/source".to_owned(),
                bytes,
                sha256: sha256.to_owned(),
                license: FixtureLicense {
                    spdx_id: "CC0-1.0".to_owned(),
                    name: "CC0 1.0".to_owned(),
                    url: "https://creativecommons.org/publicdomain/zero/1.0/".to_owned(),
                    attribution: "Test Author".to_owned(),
                },
            }],
        }
    }

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("kinewright-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn verifies_pinned_fixture_bytes() {
        let root = temporary_root("fixture-pack-verify");
        let pack_root = root.join("test-pack");
        fs::create_dir_all(&pack_root).unwrap();
        let path = pack_root.join("clip.bin");
        fs::write(&path, b"abc").unwrap();
        let manifest = manifest(
            3,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );

        assert_eq!(manifest.verified_asset(&root, "clip").unwrap(), path);
        let report = manifest.verify(&root).unwrap();
        assert_eq!(report.already_present, ["clip"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_changed_fixture_bytes() {
        let root = temporary_root("fixture-pack-integrity");
        let pack_root = root.join("test-pack");
        fs::create_dir_all(&pack_root).unwrap();
        fs::write(pack_root.join("clip.bin"), b"abd").unwrap();
        let manifest = manifest(
            3,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );

        assert!(matches!(
            manifest.verify(&root),
            Err(FixturePackError::Integrity { .. })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_path_traversal_duplicate_ids_and_insecure_urls() {
        for mutate in 0..3 {
            let mut manifest = manifest(
                3,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            );
            match mutate {
                0 => manifest.assets[0].file_name = "../clip.bin".to_owned(),
                1 => manifest.assets.push(manifest.assets[0].clone()),
                2 => manifest.assets[0].download_url = "http://example.com/clip.bin".to_owned(),
                _ => unreachable!(),
            }
            assert!(matches!(
                manifest.validate(),
                Err(FixturePackError::InvalidManifest(_))
            ));
        }
    }
}

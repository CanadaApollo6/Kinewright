//! Private attachment credentials and read-only path admission.
#![allow(clippy::missing_errors_doc)]
use super::{HostDestination, HostError, HostRefusal, SessionRoots};
use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Write as _;
use std::{
    fmt,
    fs::File,
    path::{Path, PathBuf},
};

/// A runtime secret. Debug never renders its bytes; serialization is only for private bootstrap.
#[derive(Clone)]
pub struct HostAuth([u8; 32]);
impl HostAuth {
    pub fn generate() -> Result<Self, HostError> {
        let mut token = [0; 32];
        getrandom::fill(&mut token)
            .map_err(|_| HostError::Protocol("OS randomness unavailable".into()))?;
        Ok(Self(token))
    }
    pub(crate) fn bearer(&self) -> String {
        format!(
            "Bearer {}",
            base64::engine::general_purpose::STANDARD.encode(self.0)
        )
    }
    pub(crate) fn accepts(&self, value: Option<&str>) -> bool {
        let expected = self.bearer();
        let supplied = value.unwrap_or_default().as_bytes();
        if supplied.len() != expected.len() {
            return false;
        }
        supplied
            .iter()
            .zip(expected.bytes())
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
    }
}
impl fmt::Debug for HostAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HostAuth([REDACTED])")
    }
}
impl Serialize for HostAuth {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(self.0))
    }
}
impl<'de> Deserialize<'de> for HostAuth {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| serde::de::Error::custom("invalid private attachment credential"))?;
        Ok(Self(bytes.try_into().map_err(|_| {
            serde::de::Error::custom("attachment credential must have 256 bits")
        })?))
    }
}

impl SessionRoots {
    /// JSON stays ASCII in HTTP headers; decoding restores each exact Unicode path.
    pub(crate) fn wire_header(&self) -> Result<String, HostError> {
        let json = serde_json::to_string(self)?;
        let mut encoded = String::with_capacity(json.len());
        for unit in json.encode_utf16() {
            match u8::try_from(unit) {
                // Keep connector environment expansion confined to its explicit auth template.
                Ok(byte) if byte.is_ascii() && byte != b'$' => encoded.push(char::from(byte)),
                _ => write!(&mut encoded, "\\u{unit:04x}")
                    .map_err(|_| HostError::Protocol("root header encoding failed".into()))?,
            }
        }
        Ok(encoded)
    }

    pub fn canonicalize(&self) -> Result<Self, HostError> {
        if self.read.len() + self.write.len() > 64 {
            return Err(HostRefusal::roots("too many roots").into());
        }
        let canonical = |roots: &[PathBuf]| {
            roots
                .iter()
                .map(|path| {
                    let path = path
                        .canonicalize()
                        .map_err(|_| HostRefusal::roots("root is unavailable"))?;
                    if !path.is_dir() {
                        return Err(HostRefusal::roots("root is not a directory").into());
                    }
                    Ok(path)
                })
                .collect::<Result<Vec<_>, HostError>>()
        };
        Ok(Self {
            read: canonical(&self.read)?,
            write: canonical(&self.write)?,
        })
    }
    pub(crate) fn permits(&self, path: &Path, write: bool) -> bool {
        self.write
            .iter()
            .chain(if write { [].iter() } else { self.read.iter() })
            .any(|root| path.starts_with(root))
    }
}

/// Keeps the admitted parent alive through namespace acquisition. Filesystem operations use
/// this fd-relative spelling, never the caller's mutable parent spelling.
pub(crate) struct PinnedPath {
    pub display: PathBuf,
    pub io: PathBuf,
    pub inspected: HostDestination,
    pub anchor: PathBuf,
    #[cfg_attr(
        all(not(target_os = "linux"), not(test)),
        expect(
            dead_code,
            reason = "Only the Linux namespace reads the held directory; other production profiles refuse Studio IO"
        )
    )]
    #[cfg_attr(
        all(not(target_os = "linux"), test),
        allow(
            dead_code,
            reason = "Unsupported-profile tests exercise refusal without reading native directory handles"
        )
    )]
    parent: File,
}
impl PinnedPath {
    #[cfg(target_os = "linux")]
    pub fn admit(
        path: &Path,
        configured: &SessionRoots,
        forwarded: &SessionRoots,
        write: bool,
    ) -> Result<Self, HostError> {
        use std::os::{fd::AsRawFd as _, unix::fs::MetadataExt as _};
        let name = path
            .file_name()
            .ok_or_else(|| HostRefusal::roots("project filename required"))?;
        if name == "." || name == ".." {
            return Err(HostRefusal::roots("invalid filename").into());
        }
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut ancestor = absolute
            .parent()
            .ok_or_else(|| HostRefusal::roots("missing destination parent"))?
            .to_path_buf();
        let mut missing = Vec::new();
        loop {
            match std::fs::symlink_metadata(&ancestor) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if missing.len() >= 32 {
                        return Err(HostRefusal::roots("destination ancestry capacity").into());
                    }
                    missing.push(
                        ancestor
                            .file_name()
                            .ok_or_else(|| HostRefusal::roots("invalid destination ancestor"))?
                            .to_os_string(),
                    );
                    ancestor = ancestor
                        .parent()
                        .ok_or_else(|| HostRefusal::roots("missing existing ancestor"))?
                        .into();
                }
                Err(error) => return Err(error.into()),
            }
        }
        let canonical = ancestor.canonicalize()?;
        let handle = File::open(&canonical)?;
        let pinned = PathBuf::from(format!(
            "/proc/{}/fd/{}",
            std::process::id(),
            handle.as_raw_fd()
        ));
        let observed = pinned.canonicalize()?;
        if observed != canonical
            || !configured.permits(&observed, write)
            || !forwarded.permits(&observed, write)
        {
            return Err(
                HostRefusal::roots("path is outside the configured and forwarded roots").into(),
            );
        }
        let mut relative = PathBuf::new();
        for component in missing.into_iter().rev() {
            relative.push(component);
        }
        relative.push(name);
        kinewright_project::namespace::PortablePath::new(&relative)?;
        let target = pinned.join(&relative);
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(HostRefusal::roots(
                "project symlinks require explicit admission of their resolved path",
            )
            .into());
        }
        let identity = handle.metadata()?;
        let inspected = HostDestination {
            // Resolve from the actual held parent fd once, not from the caller alias.
            // Both destination inspectors must prove this same native parent before IO.
            path: observed.join(&relative),
            parent_device: identity.dev(),
            parent_inode: identity.ino(),
        };
        Ok(Self {
            display: path.to_path_buf(),
            io: target,
            inspected,
            anchor: observed,
            parent: handle,
        })
    }
    #[cfg(target_os = "linux")]
    pub fn duplicate(&self) -> Result<Self, HostError> {
        use std::os::fd::AsRawFd as _;
        let parent = self.parent.try_clone()?;
        let relative = self
            .inspected
            .path
            .strip_prefix(&self.anchor)
            .map_err(|_| HostRefusal::roots("destination outside held ancestor"))?;
        let io = PathBuf::from(format!(
            "/proc/{}/fd/{}",
            std::process::id(),
            parent.as_raw_fd()
        ))
        .join(relative);
        Ok(Self {
            display: self.display.clone(),
            io,
            inspected: self.inspected.clone(),
            anchor: self.anchor.clone(),
            parent,
        })
    }
    /// Walk only below the held ancestor fd. Missing descendant directories are legitimate
    /// before `prepare_destination` creates them; symlinks and substitutions are refused.
    #[cfg(target_os = "linux")]
    pub fn matches(&self, target: &HostDestination) -> Result<bool, HostError> {
        use rustix::fs::{Mode, OFlags, openat};
        use std::os::unix::fs::MetadataExt as _;
        if target.path != self.inspected.path {
            return Ok(false);
        }
        let relative = self
            .inspected
            .path
            .strip_prefix(&self.anchor)
            .map_err(|_| HostRefusal::roots("destination outside held ancestor"))?;
        let mut parent = self.parent.try_clone()?;
        for component in relative
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .components()
        {
            let std::path::Component::Normal(component) = component else {
                return Err(HostRefusal::roots("destination traversal").into());
            };
            match openat(
                &parent,
                component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => parent = File::from(fd),
                Err(rustix::io::Errno::NOENT) => break,
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        }
        let actual = parent.metadata()?;
        Ok(actual.dev() == target.parent_device && actual.ino() == target.parent_inode)
    }
    #[cfg(not(target_os = "linux"))]
    #[allow(
        clippy::unused_self,
        reason = "Unsupported profiles preserve the instance capability API while refusing IO"
    )]
    pub fn matches(&self, _target: &HostDestination) -> Result<bool, HostError> {
        Err(HostRefusal::new("durability_unsupported", "pinned Studio IO is unsupported").into())
    }
    #[cfg(not(target_os = "linux"))]
    #[allow(
        clippy::unused_self,
        reason = "Unsupported profiles preserve the instance capability API while refusing IO"
    )]
    pub fn duplicate(&self) -> Result<Self, HostError> {
        Err(HostRefusal::new("durability_unsupported", "pinned Studio IO is unsupported").into())
    }
    #[cfg(not(target_os = "linux"))]
    pub fn admit(
        path: &Path,
        configured: &SessionRoots,
        forwarded: &SessionRoots,
        write: bool,
    ) -> Result<Self, HostError> {
        let _ = (path, configured, forwarded, write);
        // Stable std cannot certify a portable directory-relative Studio writer. This is the
        // project's existing platform refusal, before any namespace or source mutation.
        Err(HostRefusal::new(
            "durability_unsupported",
            "identity-pinned Studio IO is unsupported on this platform",
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_full_width_redacted_and_exact() {
        let auth = HostAuth::generate().unwrap();
        let other = HostAuth::generate().unwrap();
        assert!(auth.accepts(Some(&auth.bearer())));
        assert!(!auth.accepts(None));
        assert!(!auth.accepts(Some(&other.bearer())));
        assert!(!auth.accepts(Some("Bearer short")));
        assert_eq!(format!("{auth:?}"), "HostAuth([REDACTED])");
        let value = serde_json::to_string(&auth).unwrap();
        let roundtrip: HostAuth = serde_json::from_str(&value).unwrap();
        assert!(auth.accepts(Some(&roundtrip.bearer())));
        assert!(serde_json::from_str::<HostAuth>("\"YWJj\"").is_err());
    }
    #[test]
    fn forwarded_roots_can_only_narrow_the_admission() {
        let roots = SessionRoots {
            read: vec![PathBuf::from("/input")],
            write: vec![PathBuf::from("/output")],
        };
        assert!(roots.permits(Path::new("/input/source.mov"), false));
        assert!(!roots.permits(Path::new("/input/source.mov"), true));
        assert!(roots.permits(Path::new("/output/edit.kinewright"), true));
        assert!(!roots.permits(Path::new("/output-other/edit.kinewright"), true));
    }
}

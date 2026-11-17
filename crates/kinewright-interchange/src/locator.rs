use std::{io, path::Path};

use crate::{Refusal, RefusalCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Unix,
    Windows,
}

impl Platform {
    pub const NATIVE: Self = if cfg!(windows) {
        Self::Windows
    } else {
        Self::Unix
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Locator {
    Local(LocalPath),
    Remote,
    Invalid,
}

/// A path checked for remote prefixes and parent traversal, before I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPath {
    path: String,
    absolute: bool,
}

impl LocalPath {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub const fn is_absolute(&self) -> bool {
        self.absolute
    }
}

fn separator(byte: u8) -> bool {
    matches!(byte, b'/' | b'\\')
}

fn drive_path(path: &str) -> bool {
    matches!(path.as_bytes(), [letter, b':', sep, ..] if letter.is_ascii_alphabetic() && separator(*sep))
}

fn native_path(mut path: String, platform: Platform, decoded_file_url: bool) -> Locator {
    let bytes = path.as_bytes();
    let verbatim_drive = !decoded_file_url
        && platform == Platform::Windows
        && matches!(bytes, [b'\\', b'\\', b'?', b'\\', drive, b':', b'\\', ..] if drive.is_ascii_alphabetic());
    if matches!(bytes, [a, b, ..] if separator(*a) && separator(*b)) && !verbatim_drive {
        return Locator::Remote;
    }
    if path.is_empty() || path.contains('\0') || path.split(['/', '\\']).any(|part| part == "..") {
        return Locator::Invalid;
    }
    if platform == Platform::Windows
        && decoded_file_url
        && path.strip_prefix('/').is_some_and(drive_path)
    {
        path.remove(0);
    }
    let absolute = if platform == Platform::Windows {
        if !verbatim_drive
            && !drive_path(&path)
            && (path.starts_with(['/', '\\']) || path.as_bytes().get(1) == Some(&b':'))
        {
            return Locator::Invalid;
        }
        verbatim_drive || drive_path(&path)
    } else {
        path.starts_with('/')
    };
    Locator::Local(LocalPath { path, absolute })
}

fn decode_path(path: &str) -> LocatorDecode {
    let mut output = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            output.push(byte);
            continue;
        }
        let hex = |b: u8| {
            char::from(b)
                .to_digit(16)
                .and_then(|n| u8::try_from(n).ok())
        };
        let Some(high) = bytes.next().and_then(hex) else {
            return LocatorDecode::Invalid;
        };
        let Some(low) = bytes.next().and_then(hex) else {
            return LocatorDecode::Invalid;
        };
        let decoded = high * 16 + low;
        if separator(decoded) {
            return LocatorDecode::Remote;
        }
        output.push(decoded);
    }
    String::from_utf8(output).map_or(LocatorDecode::Invalid, LocatorDecode::Path)
}

enum LocatorDecode {
    Path(String),
    Remote,
    Invalid,
}

/// Classify locators before constructing a native filesystem argument.
#[must_use]
pub fn classify(locator: &str, platform: Platform) -> Locator {
    if platform == Platform::Windows && drive_path(locator) {
        return native_path(locator.into(), platform, false);
    }
    let Some((scheme, rest)) = locator.split_once(':').filter(|(scheme, _)| {
        scheme
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"+.-".contains(&byte))
    }) else {
        return native_path(locator.into(), platform, false);
    };
    if !scheme.eq_ignore_ascii_case("file") {
        return Locator::Remote;
    }
    let path = if let Some(authority) = rest.strip_prefix("//") {
        let index = authority.find(['/', '\\']).unwrap_or(authority.len());
        let (host, path) = authority.split_at(index);
        if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
            return Locator::Remote;
        }
        path
    } else {
        rest
    };
    if path.contains(['?', '#']) {
        return Locator::Invalid;
    }
    match decode_path(path) {
        LocatorDecode::Path(path) => native_path(path, platform, true),
        LocatorDecode::Remote => Locator::Remote,
        LocatorDecode::Invalid => Locator::Invalid,
    }
}

/// Filesystem seam for the project layer. All native I/O belongs to implementations.
pub trait LocatorFs {
    type Metadata;
    type Probe;
    type Reader: io::Read;

    /// # Errors
    /// Returns an I/O error when metadata is unavailable.
    fn stat(&self, path: &Path) -> io::Result<Self::Metadata>;
    /// # Errors
    /// Returns an I/O error when the document cannot be opened.
    fn open(&self, path: &Path) -> io::Result<Self::Reader>;
    /// # Errors
    /// Returns an I/O error when the media cannot be probed.
    fn probe(&self, path: &Path) -> io::Result<Self::Probe>;
}

/// # Errors
/// Refuses remote or malformed input before invoking the local-path callback.
pub fn with_local<T>(
    locator: &str,
    platform: Platform,
    local: impl FnOnce(LocalPath) -> T,
) -> Result<T, Refusal> {
    match classify(locator, platform) {
        Locator::Local(path) => Ok(local(path)),
        Locator::Remote => Err(Refusal::new(
            RefusalCode::RemotePath,
            "The locator names a remote path.",
        )),
        Locator::Invalid => Err(Refusal::new(
            RefusalCode::Malformed,
            "The locator is not a valid local path.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct RecordingFs(RefCell<Vec<&'static str>>);

    impl LocatorFs for RecordingFs {
        type Metadata = ();
        type Probe = ();
        type Reader = io::Empty;

        fn stat(&self, _: &Path) -> io::Result<()> {
            self.0.borrow_mut().push("stat");
            Ok(())
        }

        fn open(&self, _: &Path) -> io::Result<io::Empty> {
            self.0.borrow_mut().push("open");
            Ok(io::empty())
        }

        fn probe(&self, _: &Path) -> io::Result<()> {
            self.0.borrow_mut().push("probe");
            Ok(())
        }
    }

    #[test]
    fn remote_media_document_and_roots_cannot_reach_io() {
        let fs = RecordingFs::default();
        let paths = [
            "file://evil.example/s/a.mov",
            r"\\host\share\a.mov",
            r"\\?\UNC\host\share\a.mov",
            r"\\.\pipe\device",
            "//host/a.mov",
            r"/\host\a.mov",
            "file://localhost//host/share/a.mov",
            "file:///%2Fhost/share/a.mov",
            "file://localhost/%5c%5Chost/share/a.mov",
            "file:///local%2Feditor.otio",
            "https://example.org/a.otio",
        ];
        for platform in [Platform::Unix, Platform::Windows] {
            for path in paths {
                assert_eq!(classify(path, platform), Locator::Remote, "{path}");
                let access = with_local(path, platform, |path| {
                    let path = Path::new(path.as_str());
                    fs.stat(path).unwrap();
                    fs.open(path).unwrap();
                    fs.probe(path).unwrap();
                });
                assert_eq!(access.unwrap_err().code, RefusalCode::RemotePath, "{path}");
            }
        }
        assert!(fs.0.borrow().is_empty());

        let path = "./Media/local.mov";
        with_local(path, Platform::Unix, |local| {
            assert_eq!(local.as_str(), path);
            fs.stat(Path::new(local.as_str())).unwrap();
        })
        .unwrap();
        assert_eq!(&*fs.0.borrow(), &["stat"]);
    }

    #[test]
    fn local_url_and_drive_paths_keep_their_intended_shape() {
        let local = |path: &str, platform| with_local(path, platform, |local| local).unwrap();
        assert_eq!(
            local("file://LOCALHOST/home/My%20clip.mov", Platform::Unix).as_str(),
            "/home/My clip.mov"
        );
        assert_eq!(
            local("file:///C:/Media/clip.mov", Platform::Windows).as_str(),
            "C:/Media/clip.mov"
        );
        assert!(local(r"\\?\C:\Media\clip.mov", Platform::Windows).is_absolute());
        assert_eq!(
            classify("file:./Media/%2e%2e/file.mov", Platform::Unix),
            Locator::Invalid
        );
        assert_eq!(
            classify(r"Media\..\file.mov", Platform::Windows),
            Locator::Invalid
        );
    }
}

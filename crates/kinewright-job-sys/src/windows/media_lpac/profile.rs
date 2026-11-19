use super::{Code, Error, Sid, Step, token::sid_text, wide};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    ptr,
    sync::Mutex,
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_NAME},
    Security::{
        EqualSid,
        Isolation::{
            CreateAppContainerProfile, DeleteAppContainerProfile,
            DeriveAppContainerSidFromAppContainerName, GetAppContainerFolderPath,
        },
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, READ_CONTROL,
        RemoveDirectoryW, WRITE_DAC,
    },
    System::{
        Com::CoTaskMemFree,
        Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, REG_LINK, REG_OPTION_OPEN_LINK, RegCloseKey,
            RegOpenKeyExW, RegQueryValueExW,
        },
    },
};

mod acl;
use acl::PackageSids;

const AC: &str = "AC";
const EMPTY_AC_DIRECTORIES: &[&str] = &["INetCache", "INetCookies", "INetHistory", "Temp"];
const NODE_LIMIT: usize = 256;
const STORAGE_KEY: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppContainer\Storage";

/// A name-bound `AppContainer` identity for the current user.
#[derive(Debug)]
pub struct Profile {
    name: String,
    sid: String,
}

/// A profile that passed a read-only check. Launch checks it again.
#[derive(Debug)]
pub struct VerifiedProfile<'a>(pub(super) &'a Profile);

/// Whether preparation had to change anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Preparation {
    Prepared,
    Unchanged,
}

pub(super) static PROFILE_CHANGE: Mutex<()> = Mutex::new(());

impl Profile {
    /// Derive and remember the expected SID for a stable profile name.
    ///
    /// # Errors
    /// Rejects invalid names and failures from SID derivation.
    pub fn new(name: &str) -> Result<Self, Error> {
        if name.is_empty()
            || name.len() > 64
            || name.ends_with(['.', ' '])
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_. ".contains(&byte))
        {
            return Err(Error::new(
                Step::ProfileIdentity,
                "DeriveAppContainerSidFromAppContainerName",
                Code::Win32(ERROR_INVALID_NAME),
                "invalid profile name",
            ));
        }
        let name = name.to_ascii_lowercase();
        let sid = derive_sid(&name)?;
        let sid = sid_text(sid.0).map_err(|error| {
            Error::launch(error, Step::ProfileIdentity, "ConvertSidToStringSidW")
        })?;
        Ok(Self { name, sid })
    }

    /// The expected package SID in Windows string form.
    #[must_use]
    pub fn sid(&self) -> &str {
        &self.sid
    }

    /// The key path relative to the current user's registry hive.
    #[must_use]
    pub fn registry_path(&self) -> String {
        format!(r"{STORAGE_KEY}\{}", self.name)
    }

    /// Register this profile if needed and make its storage inaccessible to package SIDs.
    ///
    /// # Errors
    /// Returns native errors, identity or storage mismatches, and traversal-limit failures.
    pub fn prepare(&self) -> Result<Preparation, Error> {
        let _lease = PROFILE_CHANGE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (sid, mut path) = self.locate()?;
        let created = if path.is_none() {
            let created = self.register(&sid)?;
            path = Some(self.registered()?);
            created
        } else {
            false
        };
        let ac = path.ok_or_else(|| self.missing())?;
        let root = self.root(&ac)?;
        let storage = Storage::open(self, root, &ac)?;
        let packages = PackageSids::new(&sid)?;
        if storage.unprotected(&packages)?.is_none() {
            return Ok(if created {
                Preparation::Prepared
            } else {
                Preparation::Unchanged
            });
        }

        // Only the known, empty profile directories are candidates for removal.
        // Keep the root and its parent open so the verified parent path stays in place.
        let Storage {
            _packages: packages_dir,
            root,
            registry,
            ac: nodes,
        } = storage;
        if !nodes.is_empty() {
            let prune: Vec<_> = nodes
                .into_iter()
                .filter(|node| {
                    node.directory
                        && node.path.parent() == Some(ac.as_path())
                        && node.path.file_name().is_some_and(|name| {
                            EMPTY_AC_DIRECTORIES
                                .iter()
                                .any(|expected| name.eq_ignore_ascii_case(expected))
                        })
                })
                .map(|node| node.path)
                .collect();
            for child in prune {
                remove_empty(&child)?;
            }
            remove_empty(&ac)?;
        }
        let storage = Storage {
            _packages: packages_dir,
            root,
            registry,
            ac: walk(&ac)?,
        };
        storage.protect(&packages)?;
        // Read through fresh handles so installation cannot report a cached ACL.
        self.check()?;
        Ok(Preparation::Prepared)
    }

    /// Verify registration, identity and storage in the current user's profile.
    ///
    /// # Errors
    /// Returns the first unsafe or unreadable state. This check is read-only.
    pub fn verify(&self) -> Result<VerifiedProfile<'_>, Error> {
        self.check()?;
        Ok(VerifiedProfile(self))
    }

    /// Delete this named current-user profile for uninstall. Absence is success.
    ///
    /// # Errors
    /// Returns failures from identity validation or `DeleteAppContainerProfile`.
    pub fn remove(&self) -> Result<(), Error> {
        let _lease = PROFILE_CHANGE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.expected()?;
        let name = wide(OsStr::new(&self.name)).map_err(input_error)?;
        // SAFETY: name is the validated, terminated name for this user's profile.
        let status = unsafe { DeleteAppContainerProfile(name.as_ptr()) };
        let error = Error::new(
            Step::RemoveProfile,
            "DeleteAppContainerProfile",
            Code::HResult(status),
            &self.name,
        );
        if status >= 0 || error.missing() {
            Ok(())
        } else {
            Err(error)
        }
    }

    pub(super) fn check(&self) -> Result<(Sid, Storage), Error> {
        let sid = self.expected()?;
        let ac = self.registered()?;
        let storage = Storage::open(self, self.root(&ac)?, &ac)?;
        let packages = PackageSids::new(&sid)?;
        if let Some(target) = storage.unprotected(&packages)? {
            return Err(Error::unexpected(
                Step::StorageAcl,
                "GetSecurityInfo",
                format!("{target} has an unprotected DACL or a package ACE"),
            ));
        }
        Ok((sid, storage))
    }

    fn locate(&self) -> Result<(Sid, Option<PathBuf>), Error> {
        let sid = self.expected()?;
        let path = match self.registered() {
            Ok(path) => Some(path),
            Err(error) if error.missing() => None,
            Err(error) => return Err(error),
        };
        Ok((sid, path))
    }

    fn missing(&self) -> Error {
        Error::new(
            Step::ReadStorage,
            "GetAppContainerFolderPath",
            Code::Win32(2),
            &self.name,
        )
    }

    fn expected(&self) -> Result<Sid, Error> {
        let sid = derive_sid(&self.name)?;
        let actual = sid_text(sid.0).map_err(|error| {
            Error::launch(error, Step::ProfileIdentity, "ConvertSidToStringSidW")
        })?;
        if actual != self.sid {
            return Err(Error::unexpected(
                Step::ProfileIdentity,
                "DeriveAppContainerSidFromAppContainerName",
                format!("expected {}, observed {actual}", self.sid),
            ));
        }
        Ok(sid)
    }

    fn register(&self, expected: &Sid) -> Result<bool, Error> {
        let name = wide(OsStr::new(&self.name)).map_err(input_error)?;
        let mut raw = ptr::null_mut();
        // SAFETY: all strings are terminated, and raw receives an owned SID on success.
        let status = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                name.as_ptr(),
                name.as_ptr(),
                ptr::null(),
                0,
                &raw mut raw,
            )
        };
        if status.cast_unsigned() == 0x8007_0000 | ERROR_ALREADY_EXISTS {
            self.expected()?;
            return Ok(false);
        }
        if status < 0 || raw.is_null() {
            return Err(Error::new(
                Step::RegisterProfile,
                "CreateAppContainerProfile",
                Code::HResult(status),
                &self.name,
            ));
        }
        let returned = Sid(raw);
        // SAFETY: both SIDs belong to live allocations from the profile APIs.
        if unsafe { EqualSid(returned.0, expected.0) } == 0 {
            return Err(Error::unexpected(
                Step::RegisterProfile,
                "CreateAppContainerProfile",
                "returned SID differs from the expected SID",
            ));
        }
        Ok(true)
    }

    fn registered(&self) -> Result<PathBuf, Error> {
        let sid = wide(OsStr::new(self.sid())).map_err(input_error)?;
        let mut output = ptr::null_mut();
        // SAFETY: SID text is terminated and output is writable.
        let status = unsafe { GetAppContainerFolderPath(sid.as_ptr(), &raw mut output) };
        let path = if status >= 0 && !output.is_null() {
            // SAFETY: success supplied an allocated, terminated path.
            Some(PathBuf::from(unsafe {
                super::super::copy_os_string(output)
            }))
        } else {
            None
        };
        // SAFETY: the SDK releases this output through CoTaskMemFree; null is accepted.
        unsafe { CoTaskMemFree(output.cast()) };
        path.ok_or_else(|| {
            Error::new(
                Step::ReadStorage,
                "GetAppContainerFolderPath",
                Code::HResult(status),
                self.sid(),
            )
        })
    }

    fn root<'a>(&self, ac: &'a Path) -> Result<&'a Path, Error> {
        ac.parent()
            .filter(|root| {
                ac.is_absolute()
                    && ac
                        .file_name()
                        .is_some_and(|name| name.eq_ignore_ascii_case(AC))
                    && root
                        .file_name()
                        .is_some_and(|name| name.eq_ignore_ascii_case(&self.name))
                    && root
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|name| name.eq_ignore_ascii_case("Packages"))
            })
            .ok_or_else(|| {
                Error::unexpected(
                    Step::ReadStorage,
                    "GetAppContainerFolderPath",
                    format!("unexpected profile folder {}", ac.display()),
                )
            })
    }
}

fn input_error(error: io::Error) -> Error {
    Error::launch(error, Step::ProfileIdentity, "profile name")
}

fn derive_sid(name: &str) -> Result<Sid, Error> {
    let name = wide(OsStr::new(name)).map_err(input_error)?;
    let mut sid = ptr::null_mut();
    // SAFETY: name is terminated and sid receives an owned allocation on success.
    let status = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &raw mut sid) };
    if status < 0 || sid.is_null() {
        return Err(Error::new(
            Step::ProfileIdentity,
            "DeriveAppContainerSidFromAppContainerName",
            Code::HResult(status),
            "SID unavailable",
        ));
    }
    Ok(Sid(sid))
}

pub(super) struct Storage {
    _packages: Node,
    root: Node,
    registry: Key,
    ac: Vec<Node>,
}

impl Storage {
    fn open(profile: &Profile, root: &Path, ac: &Path) -> Result<Self, Error> {
        let packages = root.parent().ok_or_else(|| {
            Error::unexpected(
                Step::ReadStorage,
                "GetAppContainerFolderPath",
                "missing Packages directory",
            )
        })?;
        Ok(Self {
            _packages: Node::open(packages)?.require_directory()?,
            root: Node::open(root)?.require_directory()?,
            registry: Key::open(profile, READ_CONTROL)?,
            ac: walk(ac)?,
        })
    }

    pub(super) fn root(&self) -> &Path {
        &self.root.path
    }

    fn unprotected(&self, packages: &PackageSids<'_>) -> Result<Option<String>, Error> {
        let mut bad = None;
        for node in std::iter::once(&self.root).chain(&self.ac) {
            if !acl::secure(&node.object(), packages)? && bad.is_none() {
                bad = Some(node.path.display().to_string());
            }
        }
        if !acl::secure(&self.registry.object(), packages)? && bad.is_none() {
            bad = Some(self.registry.path.clone());
        }
        Ok(bad)
    }

    fn protect(&self, packages: &PackageSids<'_>) -> Result<(), Error> {
        for node in self.ac.iter().rev().chain([&self.root]) {
            if !acl::secure(&node.object(), packages)? {
                let writable = open_file(&node.path, READ_CONTROL | WRITE_DAC)?;
                acl::protect(&acl::Object::file(&writable, &node.path), packages)?;
            }
        }
        if !acl::secure(&self.registry.object(), packages)? {
            let key = Key::open_path(&self.registry.path, READ_CONTROL | WRITE_DAC)?;
            acl::protect(&key.object(), packages)?;
        }
        Ok(())
    }
}

struct Key {
    handle: HKEY,
    path: String,
}

impl Key {
    fn open(profile: &Profile, rights: u32) -> Result<Self, Error> {
        Self::open_path(&profile.registry_path(), rights)
    }

    fn open_path(path: &str, rights: u32) -> Result<Self, Error> {
        let name = wide(OsStr::new(path)).map_err(input_error)?;
        let mut handle = ptr::null_mut();
        // SAFETY: a terminated subkey names storage under the current user's classes hive.
        let code = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                name.as_ptr(),
                REG_OPTION_OPEN_LINK,
                rights | KEY_QUERY_VALUE,
                &raw mut handle,
            )
        };
        if code != 0 {
            return Err(Error::new(
                Step::ReadStorage,
                "RegOpenKeyExW",
                Code::Win32(code),
                path,
            ));
        }
        let key = Self {
            handle,
            path: path.into(),
        };
        let link_value = wide(OsStr::new("SymbolicLinkValue")).map_err(input_error)?;
        let (mut kind, mut size) = (0, 0);
        // SAFETY: key is open and the terminated value name and outputs are live.
        let code = unsafe {
            RegQueryValueExW(
                key.handle,
                link_value.as_ptr(),
                ptr::null_mut(),
                &raw mut kind,
                ptr::null_mut(),
                &raw mut size,
            )
        };
        if code != 0 && code != ERROR_FILE_NOT_FOUND {
            return Err(Error::new(
                Step::ReadStorage,
                "RegQueryValueExW",
                Code::Win32(code),
                path,
            ));
        }
        if code == 0 && kind == REG_LINK {
            return Err(Error::unexpected(
                Step::ReadStorage,
                "RegQueryValueExW",
                format!("registry link {path}"),
            ));
        }
        Ok(key)
    }

    fn object(&self) -> acl::Object<'_> {
        acl::Object::key(self.handle, &self.path)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this guard owns the key returned by RegOpenKeyExW.
        unsafe { RegCloseKey(self.handle) };
    }
}

struct Node {
    path: PathBuf,
    file: File,
    directory: bool,
}

impl Node {
    fn open(path: &Path) -> Result<Self, Error> {
        let file = open_file(path, READ_CONTROL | FILE_READ_ATTRIBUTES)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the file is live and info is writable.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &raw mut info) } == 0 {
            return Err(Error::last(
                Step::ReadStorage,
                "GetFileInformationByHandle",
                path.display().to_string(),
            ));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(Error::unexpected(
                Step::ReadStorage,
                "GetFileInformationByHandle",
                format!("reparse point {}", path.display()),
            ));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 && info.nNumberOfLinks != 1 {
            return Err(Error::unexpected(
                Step::ReadStorage,
                "GetFileInformationByHandle",
                format!("multiply linked file {}", path.display()),
            ));
        }
        Ok(Self {
            path: path.into(),
            file,
            directory: info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        })
    }

    fn require_directory(self) -> Result<Self, Error> {
        if !self.directory {
            return Err(Error::unexpected(
                Step::ReadStorage,
                "GetFileInformationByHandle",
                format!("expected directory {}", self.path.display()),
            ));
        }
        Ok(self)
    }

    fn object(&self) -> acl::Object<'_> {
        acl::Object::file(&self.file, &self.path)
    }
}

fn walk(ac: &Path) -> Result<Vec<Node>, Error> {
    let ac = match Node::open(ac) {
        Ok(ac) => ac.require_directory()?,
        Err(error) if error.missing() => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut nodes = vec![ac];
    let mut index = 0;
    while index < nodes.len() {
        let parent = &nodes[index];
        if parent.directory {
            let entries = fs::read_dir(&parent.path).map_err(|error| {
                Error::io(
                    Step::ReadStorage,
                    "FindFirstFileW",
                    &error,
                    parent.path.display().to_string(),
                )
            })?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    Error::io(
                        Step::ReadStorage,
                        "FindNextFileW",
                        &error,
                        ac_label(&nodes[0]),
                    )
                })?;
                if nodes.len() == NODE_LIMIT {
                    return Err(Error::unexpected(
                        Step::StorageLimit,
                        "FindNextFileW",
                        format!("AC tree exceeds {NODE_LIMIT} nodes"),
                    ));
                }
                nodes.push(Node::open(&entry.path())?);
            }
        }
        index += 1;
    }
    Ok(nodes)
}

fn ac_label(ac: &Node) -> String {
    ac.path.display().to_string()
}

fn open_file(path: &Path, access: u32) -> Result<File, Error> {
    OpenOptions::new()
        .access_mode(access)
        // Keep the object at this path while the caller inspects it.
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| {
            Error::io(
                Step::ReadStorage,
                "CreateFileW",
                &error,
                path.display().to_string(),
            )
        })
}

fn remove_empty(path: &Path) -> Result<(), Error> {
    let name = wide(path.as_os_str()).map_err(input_error)?;
    // SAFETY: this targets a known profile directory. RemoveDirectoryW is nonrecursive.
    unsafe { RemoveDirectoryW(name.as_ptr()) };
    // A nonempty or inaccessible directory is preserved and hardened by the next walk.
    Ok(())
}

pub(super) fn check_launch_directory(path: &Path, sid: &Sid) -> Result<(), Error> {
    let node = Node::open(path)?.require_directory()?;
    let mut contents = fs::read_dir(path).map_err(|error| {
        Error::io(
            Step::Environment,
            "FindFirstFileW",
            &error,
            path.display().to_string(),
        )
    })?;
    if let Some(entry) = contents.next() {
        entry.map_err(|error| {
            Error::io(
                Step::Environment,
                "FindNextFileW",
                &error,
                path.display().to_string(),
            )
        })?;
        return Err(Error::unexpected(
            Step::Environment,
            "FindNextFileW",
            format!("launch directory is nonempty: {}", path.display()),
        ));
    }
    let packages = PackageSids::new(sid)?;
    if acl::contains_package(&node.object(), &packages)? {
        return Err(Error::unexpected(
            Step::Environment,
            "GetSecurityInfo",
            format!("package ACE on launch directory {}", path.display()),
        ));
    }
    Ok(())
}

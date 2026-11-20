use super::{Code, Error, Sid, Step, StorageKind, token::sid_text, wide};
use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
    ptr,
    sync::Mutex,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_NAME, ERROR_NO_MORE_ITEMS,
    },
    Security::{
        EqualSid,
        Isolation::{
            CreateAppContainerProfile, DeleteAppContainerProfile,
            DeriveAppContainerSidFromAppContainerName, GetAppContainerFolderPath,
        },
    },
    Storage::FileSystem::{READ_CONTROL, WRITE_DAC},
    System::{
        Com::CoTaskMemFree,
        Registry::{
            HKEY, HKEY_USERS, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE, REG_LINK,
            REG_OPTION_OPEN_LINK, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
        },
    },
};

mod acl;
mod files;
use acl::PackageSids;
pub(super) use files::{Node, check_launch_directory};

const AC: &str = "AC";
const NODE_LIMIT: usize = 256;
const CLASSES_KEY: &str = r"Software\Classes";
const STORAGE_KEY: &str =
    r"Local Settings\Software\Microsoft\Windows\CurrentVersion\AppContainer\Storage";

#[derive(Clone, Copy)]
enum Access {
    Verify,
    Prepare,
}

impl Access {
    fn write_dac(self) -> u32 {
        match self {
            Self::Verify => 0,
            Self::Prepare => WRITE_DAC,
        }
    }
}

struct Budget(usize);
impl Budget {
    fn take(&mut self, api: &str) -> Result<(), Error> {
        if self.0 == NODE_LIMIT {
            return Err(Error::unexpected(
                Step::StorageLimit,
                api,
                format!("profile files and registry exceed {NODE_LIMIT} nodes"),
            ));
        }
        self.0 += 1;
        Ok(())
    }
}

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
        format!(r"{CLASSES_KEY}\{STORAGE_KEY}\{}", self.name)
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
        let storage = Storage::open(self, root, Access::Prepare)?;
        let packages = PackageSids::new(&sid)?;
        let changed = storage.protect(&packages)?;
        // Read through fresh handles so installation cannot report a cached ACL.
        drop(storage);
        self.check()?;
        Ok(if created || changed {
            Preparation::Prepared
        } else {
            Preparation::Unchanged
        })
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
        let storage = Storage::open(self, self.root(&ac)?, Access::Verify)?;
        let packages = PackageSids::new(&sid)?;
        if let Some(violation) = storage.acl_violation(&packages)? {
            return Err(violation);
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
    files: Vec<Node>,
    registry: Vec<Key>,
}

impl Storage {
    fn open(profile: &Profile, root: &Path, access: Access) -> Result<Self, Error> {
        let packages = root.parent().ok_or_else(|| {
            Error::unexpected(
                Step::ReadStorage,
                "GetAppContainerFolderPath",
                "missing Packages directory",
            )
        })?;
        let mut budget = Budget(0);
        let packages = Node::open_directory(packages)?;
        let name = root
            .file_name()
            .expect("profile root validated by Profile::root");
        let root = packages.named_directory(name, access)?;
        Ok(Self {
            _packages: packages,
            files: files::walk(root, access, &mut budget)?,
            registry: Key::walk(profile, access, &mut budget)?,
        })
    }

    fn acl_violation(&self, packages: &PackageSids<'_>) -> Result<Option<Error>, Error> {
        let mut bad = None;
        for node in &self.files {
            let violation = node.acl_violation(packages)?;
            bad = bad.or(violation);
        }
        for key in &self.registry {
            let violation = acl::violation(&key.object(), packages)?;
            bad = bad.or(violation);
        }
        Ok(bad)
    }

    fn protect(&self, packages: &PackageSids<'_>) -> Result<bool, Error> {
        let mut changed = false;
        for node in self.files.iter().rev() {
            changed |= node.protect(packages)?;
        }
        for key in self.registry.iter().rev() {
            if acl::violation(&key.object(), packages)?.is_some() {
                acl::protect(&key.object(), packages)?;
                changed = true;
            }
        }
        Ok(changed)
    }
}

struct Key {
    handle: HKEY,
    path: PathBuf,
    access: u32,
}

impl Key {
    fn walk(profile: &Profile, access: Access, budget: &mut Budget) -> Result<Vec<Self>, Error> {
        budget.take("RegOpenKeyExW")?;
        // HKCU\Software\Classes is a system alias for the current user's classes hive.
        // Start at the hive itself so every component we open can reject links.
        let sid = super::super::account_sid().map_err(|error| {
            Error::launch(error, Step::ReadStorage, "resolve classes hive account SID")
        })?;
        let hive = format!("{sid}_Classes");
        let classes = Self::open_at(
            HKEY_USERS,
            &wide(OsStr::new(&hive)).map_err(input_error)?,
            PathBuf::from(format!(r"HKEY_USERS\{hive}")),
            0,
        )?;
        let parent = classes.open_path(STORAGE_KEY.split('\\'))?;
        let name = OsStr::new(&profile.name);
        let rights = READ_CONTROL | access.write_dac();
        let mut keys = vec![Self::open_at(
            parent.handle,
            &wide(name).map_err(input_error)?,
            parent.child_path(name),
            rights,
        )?];
        let mut parent_index = 0;
        while parent_index < keys.len() {
            let parent = &keys[parent_index];
            let mut children = Vec::new();
            let mut index = 0;
            loop {
                let mut name = [0u16; 256];
                let mut len = 256;
                // SAFETY: parent is open for enumeration; name and len are writable.
                let code = unsafe {
                    RegEnumKeyExW(
                        parent.handle,
                        index,
                        name.as_mut_ptr(),
                        &raw mut len,
                        ptr::null(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                    )
                };
                if code == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if code != 0 {
                    return Err(parent.read_error("RegEnumKeyExW", Code::Win32(code), ""));
                }
                budget.take("RegEnumKeyExW")?;
                let len = len as usize;
                if len == 0
                    || len >= name.len()
                    || name[len] != 0
                    || name[..len].iter().any(|unit| matches!(*unit, 0 | 92))
                {
                    return Err(parent.read_error(
                        "RegEnumKeyExW",
                        Code::UnexpectedResult,
                        "invalid subkey name",
                    ));
                }
                let path = parent.child_path(&OsString::from_wide(&name[..len]));
                children.push(Self::open_at(parent.handle, &name[..=len], path, rights)?);
                index += 1;
            }
            keys.extend(children);
            parent_index += 1;
        }
        Ok(keys)
    }

    fn child_path(&self, name: &OsStr) -> PathBuf {
        let mut path = self.path.as_os_str().to_os_string();
        path.push(r"\");
        path.push(name);
        path.into()
    }

    fn open_path<'a>(
        mut self,
        components: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, Error> {
        for component in components {
            let component = OsStr::new(component);
            let name = wide(component).map_err(input_error)?;
            let path = self.child_path(component);
            self = Self::open_at(self.handle, &name, path, READ_CONTROL)?;
        }
        Ok(self)
    }

    fn open_at(parent: HKEY, name: &[u16], path: PathBuf, rights: u32) -> Result<Self, Error> {
        let access = rights | KEY_QUERY_VALUE | KEY_ENUMERATE_SUB_KEYS;
        let mut handle = ptr::null_mut();
        // SAFETY: parent is live and name is a terminated subkey name.
        let code = unsafe {
            RegOpenKeyExW(
                parent,
                name.as_ptr(),
                REG_OPTION_OPEN_LINK,
                access,
                &raw mut handle,
            )
        };
        if code != 0 {
            let step = if rights & WRITE_DAC != 0 {
                Step::StorageAcl
            } else {
                Step::ReadStorage
            };
            return Err(
                Error::new(step, "RegOpenKeyExW", Code::Win32(code), "").at_storage(
                    StorageKind::RegistryKey,
                    &path,
                    access,
                ),
            );
        }
        let key = Self {
            handle,
            path,
            access,
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
            return Err(key.read_error("RegQueryValueExW", Code::Win32(code), "SymbolicLinkValue"));
        }
        if code == 0 && kind == REG_LINK {
            return Err(key.read_error(
                "RegQueryValueExW",
                Code::UnexpectedResult,
                "registry link",
            ));
        }
        Ok(key)
    }

    fn object(&self) -> acl::Object<'_> {
        acl::Object::key(self.handle, &self.path, self.access)
    }

    fn read_error(&self, api: &str, code: Code, detail: &str) -> Error {
        Error::new(Step::ReadStorage, api, code, detail).at_storage(
            StorageKind::RegistryKey,
            &self.path,
            self.access,
        )
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this guard owns the key returned by RegOpenKeyExW.
        unsafe { RegCloseKey(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_CREATE_LINK, RegCreateKeyExW, RegDeleteTreeW,
        RegSetValueExW,
    };

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtDeleteKey(key: HKEY) -> i32;
    }

    struct Scratch(Vec<u16>);
    impl Drop for Scratch {
        fn drop(&mut self) {
            // SAFETY: this is the terminated name of the dedicated test subtree.
            let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, self.0.as_ptr()) };
            assert_eq!(code, 0, "RegDeleteTreeW (test subtree) Win32 {code}");
        }
    }

    struct Link(Key);
    impl Drop for Link {
        fn drop(&mut self) {
            // SAFETY: this deletes the source key by its owned handle, without resolving the link.
            let status = unsafe { NtDeleteKey(self.0.handle) };
            assert_eq!(status, 0, "NtDeleteKey (test link) NTSTATUS {status:#x}");
        }
    }

    fn create_key(parent: HKEY, name: &[u16], path: String, options: u32) -> Key {
        let mut handle = ptr::null_mut();
        // SAFETY: parent is live, name is terminated and handle is writable.
        let code = unsafe {
            RegCreateKeyExW(
                parent,
                name.as_ptr(),
                0,
                ptr::null(),
                options,
                KEY_ALL_ACCESS,
                ptr::null(),
                &raw mut handle,
                ptr::null_mut(),
            )
        };
        assert_eq!(code, 0, "RegCreateKeyExW [{path}] Win32 {code}");
        Key {
            handle,
            path: path.into(),
            access: KEY_ALL_ACCESS,
        }
    }

    #[test]
    fn registry_ancestry_rejects_an_intermediate_link() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = format!(
            r"Software\Kinewright-LpacRegistryLink-{}-{now}",
            std::process::id()
        );
        let name = wide(OsStr::new(&path)).unwrap();
        let root = create_key(HKEY_CURRENT_USER, &name, path.clone(), 0);
        let _scratch = Scratch(name);
        let target = create_key(
            root.handle,
            &wide(OsStr::new("target")).unwrap(),
            format!(r"{path}\target"),
            0,
        );
        create_key(
            target.handle,
            &wide(OsStr::new("leaf")).unwrap(),
            format!(r"{path}\target\leaf"),
            0,
        );
        let link = Link(create_key(
            root.handle,
            &wide(OsStr::new("link")).unwrap(),
            format!(r"{path}\link"),
            REG_OPTION_CREATE_LINK,
        ));
        let sid = super::super::super::account_sid().unwrap();
        let target: Vec<u16> = format!(r"\Registry\User\{sid}\{path}\target")
            .encode_utf16()
            .collect();
        let value = wide(OsStr::new("SymbolicLinkValue")).unwrap();
        // SAFETY: the linked destination and the source are both under the scratch key.
        let code = unsafe {
            RegSetValueExW(
                link.0.handle,
                value.as_ptr(),
                0,
                REG_LINK,
                target.as_ptr().cast(),
                u32::try_from(size_of_val(target.as_slice())).unwrap(),
            )
        };
        assert_eq!(code, 0, "RegSetValueExW (test link) Win32 {code}");

        let mut child = ptr::null_mut();
        let through_link = wide(OsStr::new(r"link\leaf")).unwrap();
        // SAFETY: this deliberately follows the scratch link to its known test target.
        let code = unsafe {
            RegOpenKeyExW(
                root.handle,
                through_link.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &raw mut child,
            )
        };
        assert_eq!(code, 0, "RegOpenKeyExW (link control) Win32 {code}");
        // SAFETY: the successful open transferred the control handle.
        unsafe { RegCloseKey(child) };

        let root_link = root.child_path(OsStr::new("link"));
        let Err(error) = root.open_path(["link", "leaf"]) else {
            panic!("verified registry walk followed a link");
        };
        assert_eq!(error.step, Step::ReadStorage);
        assert_eq!(
            error.storage_object(),
            Some((StorageKind::RegistryKey, root_link.as_path()))
        );
        assert!(error.to_string().contains("registry link"), "{error}");
    }
}

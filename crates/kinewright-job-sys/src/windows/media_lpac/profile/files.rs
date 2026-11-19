use super::{Budget, Error, PackageSids, Sid, Step, acl};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io,
    mem::offset_of,
    os::windows::{
        ffi::OsStringExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx, READ_CONTROL, ReOpenFile,
        WRITE_DAC,
    },
};

const FLAGS: u32 = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;
const SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    volume: u32,
    index: u64,
}

pub(in super::super) struct Node {
    path: PathBuf,
    file: File,
    identity: Identity,
    directory: bool,
}

impl Node {
    pub(super) fn open_directory(path: &Path) -> Result<Self, Error> {
        Self::open(path, true)
    }

    fn open(path: &Path, directory: bool) -> Result<Self, Error> {
        // Attribute-only opens do not enforce the sharing mode.
        let access = READ_CONTROL
            | FILE_READ_ATTRIBUTES
            | if directory {
                FILE_LIST_DIRECTORY
            } else {
                FILE_READ_DATA
            };
        let file = OpenOptions::new()
            .access_mode(access)
            .share_mode(SHARE)
            .custom_flags(FLAGS)
            .open(path)
            .map_err(|error| {
                Error::io(
                    Step::ReadStorage,
                    "CreateFileW",
                    &error,
                    path.display().to_string(),
                )
            })?;
        let (identity, actual_directory) = checked_info(&file, path)?;
        if directory != actual_directory {
            return Err(identity_error(path));
        }
        Ok(Self {
            path: path.into(),
            file,
            identity,
            directory,
        })
    }

    fn open_child(&self, name: &OsStr, file_index: u64, directory: bool) -> Result<Self, Error> {
        let path = self.path.join(name);
        let child = Self::open(&path, directory)?;
        if child.identity
            != (Identity {
                volume: self.identity.volume,
                index: file_index,
            })
        {
            return Err(identity_error(&path));
        }
        Ok(child)
    }

    pub(super) fn named_directory(&self, name: &OsStr) -> Result<Self, Error> {
        let mut found = None;
        self.children(|entry, file_index, directory| {
            let entry = OsString::from_wide(entry);
            if entry.eq_ignore_ascii_case(name) {
                if !directory {
                    return Err(identity_error(&self.path.join(name)));
                }
                found = Some(self.open_child(name, file_index, true)?);
                return Ok(false);
            }
            Ok(true)
        })?;
        found.ok_or_else(|| identity_error(&self.path.join(name)))
    }

    fn confirm(&self, file: &File) -> Result<(), Error> {
        if checked_info(file, &self.path)? != (self.identity, self.directory) {
            return Err(identity_error(&self.path));
        }
        Ok(())
    }

    pub(super) fn acl_violation(
        &self,
        packages: &PackageSids<'_>,
    ) -> Result<Option<String>, Error> {
        self.confirm(&self.file)?;
        acl::violation(&acl::Object::file(&self.file, &self.path), packages)
    }

    pub(super) fn protect(&self, packages: &PackageSids<'_>) -> Result<bool, Error> {
        if self.acl_violation(packages)?.is_none() {
            return Ok(false);
        }
        // SAFETY: ReOpenFile reopens the checked object, not its path; the original stays open.
        let raw = unsafe {
            ReOpenFile(
                self.file.as_raw_handle(),
                READ_CONTROL | FILE_READ_ATTRIBUTES | WRITE_DAC,
                SHARE,
                FLAGS,
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(Error::last(
                Step::StorageAcl,
                "ReOpenFile",
                self.path.display().to_string(),
            ));
        }
        // SAFETY: the successful call transferred ownership of a distinct handle.
        let writable = unsafe { File::from_raw_handle(raw) };
        self.confirm(&writable)?;
        acl::protect(&acl::Object::file(&writable, &self.path), packages)?;
        Ok(true)
    }

    fn children(
        &self,
        mut visit: impl FnMut(&[u16], u64, bool) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        self.confirm(&self.file)?;
        let mut storage = [0u64; 1024];
        let mut class = FileIdBothDirectoryRestartInfo;
        loop {
            // SAFETY: the directory handle is live and the buffer is aligned and writable.
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    self.file.as_raw_handle(),
                    class,
                    storage.as_mut_ptr().cast(),
                    u32::try_from(size_of_val(&storage)).expect("directory buffer size"),
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NO_MORE_FILES.cast_signed()) {
                    return Ok(());
                }
                return Err(Error::io(
                    Step::ReadStorage,
                    "GetFileInformationByHandleEx (directory)",
                    &error,
                    self.path.display().to_string(),
                ));
            }
            class = FileIdBothDirectoryInfo;
            let mut offset = 0;
            loop {
                const NAME_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                let remaining = size_of_val(&storage) - offset;
                if remaining < size_of::<FILE_ID_BOTH_DIR_INFO>()
                    || !offset.is_multiple_of(size_of::<u64>())
                {
                    return Err(identity_error(&self.path));
                }
                // SAFETY: this record lies in the aligned buffer just populated by Windows.
                let raw = unsafe { storage.as_ptr().add(offset / size_of::<u64>()) };
                // SAFETY: full header size and alignment were checked above.
                let entry = unsafe { &*raw.cast::<FILE_ID_BOTH_DIR_INFO>() };
                let next = entry.NextEntryOffset as usize;
                let length = entry.FileNameLength as usize;
                let size = if next == 0 { remaining } else { next };
                if size > remaining
                    || size < NAME_OFFSET
                    || length > size - NAME_OFFSET
                    || !length.is_multiple_of(2)
                {
                    return Err(identity_error(&self.path));
                }
                // SAFETY: the variable UTF-16 name fits in the checked record and starts aligned.
                let name = unsafe {
                    std::slice::from_raw_parts(
                        storage
                            .as_ptr()
                            .cast::<u16>()
                            .add(offset / size_of::<u16>() + NAME_OFFSET / size_of::<u16>()),
                        length / 2,
                    )
                };
                if !matches!(name, [46] | [46, 46]) {
                    if name.is_empty() || name.iter().any(|unit| matches!(*unit, 0 | 47 | 58 | 92))
                    {
                        return Err(identity_error(&self.path));
                    }
                    if !visit(
                        name,
                        entry.FileId.cast_unsigned(),
                        entry.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                    )? {
                        return Ok(());
                    }
                }
                if next == 0 {
                    break;
                }
                offset += next;
            }
        }
    }
}

fn checked_info(file: &File, path: &Path) -> Result<(Identity, bool), Error> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: file is live and info is writable.
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
    let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if !directory && info.nNumberOfLinks != 1 {
        return Err(Error::unexpected(
            Step::ReadStorage,
            "GetFileInformationByHandle",
            format!("multiply linked file {}", path.display()),
        ));
    }
    Ok((
        Identity {
            volume: info.dwVolumeSerialNumber,
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        },
        directory,
    ))
}

fn identity_error(path: &Path) -> Error {
    Error::unexpected(
        Step::ReadStorage,
        "GetFileInformationByHandle(Ex)",
        format!(
            "storage identity changed or directory record is invalid: {}",
            path.display()
        ),
    )
}

pub(super) fn walk(root: Node, budget: &mut Budget) -> Result<Vec<Node>, Error> {
    budget.take("GetFileInformationByHandleEx")?;
    let mut nodes = vec![root];
    let mut index = 0;
    while index < nodes.len() {
        let parent = &nodes[index];
        let mut children = Vec::new();
        if parent.directory {
            parent.children(|name, file_index, directory| {
                budget.take("GetFileInformationByHandleEx")?;
                children.push(parent.open_child(
                    &OsString::from_wide(name),
                    file_index,
                    directory,
                )?);
                Ok(true)
            })?;
        }
        nodes.extend(children);
        index += 1;
    }
    Ok(nodes)
}

pub(in super::super) fn check_launch_directory(path: &Path, sid: &Sid) -> Result<Node, Error> {
    let node = Node::open_directory(path)?;
    node.children(|_, _, _| {
        Err(Error::unexpected(
            Step::Environment,
            "GetFileInformationByHandleEx",
            format!("launch directory is nonempty: {}", path.display()),
        ))
    })?;
    let packages = PackageSids::new(sid)?;
    if acl::contains_package(&acl::Object::file(&node.file, path), &packages)? {
        return Err(Error::unexpected(
            Step::Environment,
            "GetSecurityInfo",
            format!("package ACE on launch directory {}", path.display()),
        ));
    }
    Ok(node)
}

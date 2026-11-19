use super::{AC, Budget, Error, PackageSids, Sid, Step, acl};
use std::{
    ffi::OsString,
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
        BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo,
        FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx, READ_CONTROL, ReOpenFile, SetFileInformationByHandle,
        WRITE_DAC,
    },
};

const FLAGS: u32 = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;
const SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
const EMPTY_AC_DIRECTORIES: &[&str] = &["INetCache", "INetCookies", "INetHistory", "Temp"];

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    volume: u32,
    index: u64,
}

pub(super) struct Node {
    path: PathBuf,
    file: File,
    identity: Identity,
    directory: bool,
    can_prune: bool,
}

impl Node {
    pub(super) fn open_directory(path: &Path) -> Result<Self, Error> {
        Self::open(path, true, false)
    }

    fn open(path: &Path, directory: bool, prune: bool) -> Result<Self, Error> {
        // Attribute-only opens do not enforce the sharing mode.
        let access = READ_CONTROL
            | FILE_READ_ATTRIBUTES
            | if directory {
                FILE_LIST_DIRECTORY
            } else {
                FILE_READ_DATA
            };
        let open = |extra| {
            OpenOptions::new()
                .access_mode(access | extra)
                .share_mode(SHARE)
                .custom_flags(FLAGS)
                .open(path)
        };
        let (file, can_prune) = if prune {
            match open(DELETE) {
                Ok(file) => (Ok(file), true),
                Err(_) => (open(0), false),
            }
        } else {
            (open(0), false)
        };
        let file = file.map_err(|error| {
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
            can_prune,
        })
    }

    fn confirm(&self, file: &File) -> Result<(), Error> {
        if checked_info(file, &self.path)? != (self.identity, self.directory) {
            return Err(identity_error(&self.path));
        }
        Ok(())
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn secure(&self, packages: &PackageSids<'_>) -> Result<bool, Error> {
        self.confirm(&self.file)?;
        acl::secure(&acl::Object::file(&self.file, &self.path), packages)
    }

    pub(super) fn protect(&self, packages: &PackageSids<'_>) -> Result<(), Error> {
        if self.secure(packages)? {
            return Ok(());
        }
        let share = SHARE | if self.can_prune { FILE_SHARE_DELETE } else { 0 };
        // SAFETY: ReOpenFile reopens the checked object, not its path; the original stays open.
        let raw = unsafe {
            ReOpenFile(
                self.file.as_raw_handle(),
                READ_CONTROL | FILE_READ_ATTRIBUTES | WRITE_DAC,
                share,
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
        acl::protect(&acl::Object::file(&writable, &self.path), packages)
    }

    pub(super) fn prune(&self) -> Result<bool, Error> {
        if !self.can_prune {
            return Ok(false);
        }
        self.confirm(&self.file)?;
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: this is the checked plain directory, opened for DELETE without reparse traversal.
        // FileDispositionInfo can remove an empty directory only. Closing the handle completes it.
        let ok = unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle(),
                FileDispositionInfo,
                (&raw const disposition).cast(),
                u32::try_from(size_of_val(&disposition)).expect("file disposition size"),
            )
        };
        // A failed prune leaves this same object open for hardening.
        Ok(ok != 0)
    }

    fn children(
        &self,
        mut visit: impl FnMut(&[u16], u64, bool) -> Result<(), Error>,
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
                    visit(
                        name,
                        entry.FileId.cast_unsigned(),
                        entry.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                    )?;
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

pub(super) fn walk(root: &Path, prepare: bool, budget: &mut Budget) -> Result<Vec<Node>, Error> {
    budget.take("GetFileInformationByHandleEx")?;
    let mut nodes = vec![Node::open_directory(root)?];
    let mut index = 0;
    while index < nodes.len() {
        let parent = &nodes[index];
        let mut children = Vec::new();
        if parent.directory {
            parent.children(|name, file_index, directory| {
                budget.take("GetFileInformationByHandleEx")?;
                let name = OsString::from_wide(name);
                let path = parent.path.join(&name);
                let is_ac = parent.path == root && name.eq_ignore_ascii_case(AC);
                let is_ac_child = parent.path.parent() == Some(root)
                    && parent
                        .path
                        .file_name()
                        .is_some_and(|name| name.eq_ignore_ascii_case(AC));
                let prunable = prepare
                    && directory
                    && (is_ac
                        || (is_ac_child
                            && EMPTY_AC_DIRECTORIES
                                .iter()
                                .any(|expected| name.eq_ignore_ascii_case(expected))));
                let child = Node::open(&path, directory, prunable)?;
                if child.identity
                    != (Identity {
                        volume: parent.identity.volume,
                        index: file_index,
                    })
                {
                    return Err(identity_error(&path));
                }
                children.push(child);
                Ok(())
            })?;
        }
        nodes.extend(children);
        index += 1;
    }
    Ok(nodes)
}

pub(in super::super) fn check_launch_directory(path: &Path, sid: &Sid) -> Result<(), Error> {
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
    Ok(())
}

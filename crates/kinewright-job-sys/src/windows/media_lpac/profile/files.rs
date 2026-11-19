use super::{Access, Budget, Error, PackageSids, Sid, Step, StorageKind, acl};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io,
    mem::offset_of,
    os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::ERROR_NO_MORE_FILES,
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx, READ_CONTROL,
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
    access: u32,
}

impl Node {
    pub(super) fn open_directory(path: &Path) -> Result<Self, Error> {
        Self::open(path, true, Access::Verify)
    }

    fn access_mask(directory: bool, mode: Access) -> u32 {
        let data = if directory {
            FILE_LIST_DIRECTORY
        } else {
            // Both modes pin the file name. Attribute-only opens do not enforce sharing.
            FILE_READ_DATA
        };
        READ_CONTROL | FILE_READ_ATTRIBUTES | data | mode.write_dac()
    }

    fn kind(directory: bool) -> StorageKind {
        if directory {
            StorageKind::Directory
        } else {
            StorageKind::File
        }
    }

    fn open(path: &Path, directory: bool, mode: Access) -> Result<Self, Error> {
        let access = Self::access_mask(directory, mode);
        let kind = Self::kind(directory);
        let at_object = |error: Error| error.at_storage(kind, path, access);
        let file = OpenOptions::new()
            .access_mode(access)
            .share_mode(SHARE)
            .custom_flags(FLAGS)
            .open(path)
            .map_err(|error| {
                let step = match mode {
                    Access::Verify => Step::ReadStorage,
                    Access::Prepare => Step::StorageAcl,
                };
                at_object(Error::io(step, "CreateFileW", &error, ""))
            })?;
        let (identity, actual_directory) = checked_info(&file).map_err(at_object)?;
        if directory != actual_directory {
            return Err(at_object(identity_error()));
        }
        Ok(Self {
            path: path.into(),
            file,
            identity,
            directory,
            access,
        })
    }

    fn open_child(
        &self,
        name: &OsStr,
        file_index: u64,
        directory: bool,
        mode: Access,
    ) -> Result<Self, Error> {
        let path = self.path.join(name);
        let child = Self::open(&path, directory, mode)?;
        if child.identity
            != (Identity {
                volume: self.identity.volume,
                index: file_index,
            })
        {
            return Err(child.at_object(identity_error()));
        }
        Ok(child)
    }

    pub(super) fn named_directory(&self, name: &OsStr, mode: Access) -> Result<Self, Error> {
        let invalid = || {
            identity_error().at_storage(
                StorageKind::Directory,
                &self.path.join(name),
                Self::access_mask(true, mode),
            )
        };
        let mut found = None;
        self.children(|entry, file_index, directory| {
            let entry = OsString::from_wide(entry);
            if entry.eq_ignore_ascii_case(name) {
                if !directory {
                    return Err(invalid());
                }
                found = Some(self.open_child(name, file_index, true, mode)?);
                return Ok(false);
            }
            Ok(true)
        })?;
        found.ok_or_else(invalid)
    }

    fn at_object(&self, error: Error) -> Error {
        error.at_storage(Self::kind(self.directory), &self.path, self.access)
    }

    fn confirm(&self) -> Result<(), Error> {
        let observed = checked_info(&self.file).map_err(|error| self.at_object(error))?;
        if observed != (self.identity, self.directory) {
            return Err(self.at_object(identity_error()));
        }
        Ok(())
    }

    pub(super) fn acl_violation(&self, packages: &PackageSids<'_>) -> Result<Option<Error>, Error> {
        self.confirm()?;
        acl::violation(&self.object(), packages)
    }

    pub(super) fn protect(&self, packages: &PackageSids<'_>) -> Result<bool, Error> {
        if self.acl_violation(packages)?.is_none() {
            return Ok(false);
        }
        acl::protect(&self.object(), packages)?;
        Ok(true)
    }

    fn object(&self) -> acl::Object<'_> {
        acl::Object::file(
            &self.file,
            &self.path,
            Self::kind(self.directory),
            self.access,
        )
    }

    fn children(
        &self,
        mut visit: impl FnMut(&[u16], u64, bool) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        self.confirm()?;
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
                return Err(self.at_object(Error::io(
                    Step::ReadStorage,
                    "GetFileInformationByHandleEx (directory)",
                    &error,
                    "",
                )));
            }
            class = FileIdBothDirectoryInfo;
            let mut offset = 0;
            loop {
                const NAME_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                let remaining = size_of_val(&storage) - offset;
                if remaining < size_of::<FILE_ID_BOTH_DIR_INFO>()
                    || !offset.is_multiple_of(size_of::<u64>())
                {
                    return Err(self.at_object(identity_error()));
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
                    return Err(self.at_object(identity_error()));
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
                        return Err(self.at_object(identity_error()));
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

fn checked_info(file: &File) -> Result<(Identity, bool), Error> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: file is live and info is writable.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &raw mut info) } == 0 {
        return Err(Error::last(
            Step::ReadStorage,
            "GetFileInformationByHandle",
            "",
        ));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(Error::unexpected(
            Step::ReadStorage,
            "GetFileInformationByHandle",
            "reparse point",
        ));
    }
    let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if !directory && info.nNumberOfLinks != 1 {
        return Err(Error::unexpected(
            Step::ReadStorage,
            "GetFileInformationByHandle",
            "multiply linked file",
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

fn identity_error() -> Error {
    Error::unexpected(
        Step::ReadStorage,
        "GetFileInformationByHandle(Ex)",
        "storage identity changed or directory record is invalid",
    )
}

pub(super) fn walk(root: Node, mode: Access, budget: &mut Budget) -> Result<Vec<Node>, Error> {
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
                    mode,
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
    if acl::contains_package(&node.object(), &packages)? {
        return Err(Error::unexpected(
            Step::Environment,
            "GetSecurityInfo",
            format!("package ACE on launch directory {}", path.display()),
        ));
    }
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::ScratchDirectory;
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;

    #[test]
    fn preparation_handle_prevents_renaming_a_retained_file() {
        let scratch = ScratchDirectory::new();
        let profile = scratch.0.join("profile");
        std::fs::create_dir(&profile).expect("create profile witness directory");
        let file = profile.join("retained");
        let moved = scratch.0.join("moved");
        std::fs::write(&file, b"retained").expect("seed retained file");
        let held = Node::open(&file, false, Access::Prepare).expect("open with preparation mask");
        let denial = std::fs::rename(&file, &moved).expect_err("preparation handle allowed rename");
        assert_eq!(
            denial.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION.cast_signed()),
            "{denial}"
        );
        drop(held);

        // Control the actual regression: a metadata-only handle has the same sharing mode.
        let metadata_only = OpenOptions::new()
            .access_mode(Node::access_mask(false, Access::Prepare) & !FILE_READ_DATA)
            .share_mode(SHARE)
            .custom_flags(FLAGS)
            .open(&file)
            .expect("open without FILE_READ_DATA");
        std::fs::rename(&file, &moved).expect("metadata-only handle permits rename");
        drop(metadata_only);
        assert_eq!(std::fs::read(moved).unwrap(), b"retained");
    }
}

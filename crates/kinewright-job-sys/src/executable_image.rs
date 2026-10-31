//! Immutable provider images. No mutable file supplies executable text pages.
use rustix::fs::{MemfdFlags, SealFlags};
use std::{
    fs::File,
    io::{self, Read, Seek},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::Path,
    process::{Child, Command},
};
const SEALS: SealFlags = SealFlags::WRITE
    .union(SealFlags::GROW)
    .union(SealFlags::SHRINK)
    .union(SealFlags::SEAL);

/// Copy and verify the complete image, then exclude every future writer.
/// # Errors
/// Refuses oversized, changed, or unsealable executables.
pub fn seal_executable(path: &Path, digest: &str) -> io::Result<File> {
    let mut source = File::open(path)?;
    if !source.metadata()?.is_file() || source.metadata()?.len() > 512 << 20 {
        return Err(io::Error::other("executable image size"));
    }
    let mut image = File::from(rustix::fs::memfd_create(
        c"kinewright-provider",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?);
    io::copy(
        &mut Read::by_ref(&mut source).take((512 << 20) + 1),
        &mut image,
    )?;
    if image.metadata()?.len() > 512 << 20 {
        return Err(io::Error::other("executable image size"));
    }
    image.rewind()?;
    if kinewright_hash::sha256_reader(&mut image)? != digest {
        return Err(io::Error::other("qualified executable digest changed"));
    }
    rustix::fs::fcntl_add_seals(&image, SEALS)?;
    image.rewind()?;
    Ok(image)
}

/// Verify the host's sealed image before entering the provider namespace.
/// # Errors
/// Refuses mutable or differently qualified descriptors.
pub fn open_executable_image(path: &Path, digest: &str) -> io::Result<File> {
    let mut image = File::open(path)?;
    if !rustix::fs::fcntl_get_seals(&image)?.contains(SEALS) || image.metadata()?.len() > 512 << 20
    {
        return Err(io::Error::other("unsealed executable image"));
    }
    if kinewright_hash::sha256_reader(&mut image)? != digest {
        return Err(io::Error::other("qualified image digest changed"));
    }
    image.rewind()?;
    Ok(image)
}

/// Spawn an ELF image with its descriptor closed by the successful exec.
/// # Errors
/// Refuses a mutable image, invalid arguments or a failed execveat.
pub fn spawn_executable_image(mut command: Command, image: &File) -> io::Result<Child> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    if !rustix::fs::fcntl_get_seals(image)?.contains(SEALS) {
        return Err(io::Error::other("unsealed executable image"));
    }
    let args: Vec<_> = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|s| CString::new(s.as_bytes()))
        .collect::<Result<_, _>>()?;
    let env: Vec<_> = command
        .get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| {
                let mut bytes = key.as_bytes().to_vec();
                bytes.push(b'=');
                bytes.extend_from_slice(value.as_bytes());
                CString::new(bytes)
            })
        })
        .collect::<Result<_, _>>()?;
    if args.len() >= 130
        || env.len() >= 32
        || !rustix::io::fcntl_getfd(image)?.contains(rustix::io::FdFlags::CLOEXEC)
    {
        return Err(io::Error::other("image exec descriptor/argument bound"));
    }
    let fd = image.as_raw_fd();
    // SAFETY: Only async-signal-safe execveat is called after fork. All strings
    // and their pointer arrays live in this closure until exec or its failure.
    unsafe {
        command.pre_exec(move || {
            let mut argument_pointers = [std::ptr::null(); 130];
            let mut envp = [std::ptr::null(); 32];
            for (slot, value) in argument_pointers.iter_mut().zip(&args) {
                *slot = value.as_ptr();
            }
            for (slot, value) in envp.iter_mut().zip(&env) {
                *slot = value.as_ptr();
            }
            libc::syscall(
                libc::SYS_execveat,
                fd,
                c"".as_ptr(),
                argument_pointers.as_ptr(),
                envp.as_ptr(),
                libc::AT_EMPTY_PATH,
            );
            Err(io::Error::last_os_error())
        });
    }
    let child = command.spawn();
    // Consume the command so its pre-exec closure cannot outlive this borrowed image.
    drop(command);
    child
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, os::unix::fs::MetadataExt};

    #[test]
    fn s4b_c5_fix1_image_barrier_binds_qualified_bytes() {
        let root = std::env::temp_dir().join(format!("image-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("provider");
        fs::copy("/usr/bin/true", &source).unwrap();
        let before = fs::metadata(&source).unwrap();
        let digest = kinewright_hash::sha256_reader(&mut fs::File::open(&source).unwrap()).unwrap();
        let image = seal_executable(&source, &digest).unwrap();
        let mut source_fd = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&source)
            .unwrap();
        source_fd
            .write_all(&fs::read("/usr/bin/false").unwrap())
            .unwrap();
        source_fd.sync_all().unwrap();
        drop(source_fd);
        let after = fs::metadata(&source).unwrap();
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        assert!(
            seal_executable(&source, &digest).is_err(),
            "C5_FIX1_CHANGED_SOURCE_REFUSED"
        );
        let mut command = Command::new(&source);
        command.env_clear();
        assert!(
            spawn_executable_image(command, &image)
                .unwrap()
                .wait()
                .unwrap()
                .success(),
            "C5_FIX1_IMMUTABLE_QUALIFIED_EXEC"
        );
        assert!(rustix::fs::fcntl_get_seals(&image).unwrap().contains(SEALS));
        assert!(
            rustix::fs::ftruncate(&image, 0).is_err(),
            "C5_FIX1_IMAGE_SEALED"
        );
        assert!(
            rustix::io::fcntl_getfd(&image)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC)
        );
        fs::remove_dir_all(root).unwrap();
    }
}

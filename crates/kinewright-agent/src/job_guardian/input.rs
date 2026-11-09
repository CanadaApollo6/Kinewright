//! Sealed anonymous stdin. Only the acknowledged bytes cross into the provider.
use crate::job_guardian::protocol::InputBinding;
use rustix::fs::{MemfdFlags, SealFlags};
use std::{
    fs::File,
    io::{self, Read, Seek, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
};
const SEALS: SealFlags = SealFlags::WRITE
    .union(SealFlags::GROW)
    .union(SealFlags::SHRINK)
    .union(SealFlags::SEAL);

#[allow(
    dead_code,
    reason = "host seals stdin; the standalone guardian only verifies it"
)]
pub(crate) fn seal(bytes: &[u8], binding: &InputBinding) -> io::Result<File> {
    if InputBinding::new(bytes)?
        != (InputBinding {
            source: None,
            ..binding.clone()
        })
    {
        return Err(io::Error::other("native input digest/length changed"));
    }
    let mut file = File::from(rustix::fs::memfd_create(
        c"kinewright-input",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?);
    file.write_all(bytes)?;
    rustix::fs::fcntl_add_seals(&file, SEALS)?;
    // Reopen, rather than dup, to give the child a genuinely read-only handle
    // and a fresh file offset. The writable handle never leaves this function.
    open(&source(&file), binding)
}
#[allow(
    dead_code,
    reason = "host supplies this descriptor path; the standalone guardian only opens it"
)]
pub(crate) fn source(file: &File) -> PathBuf {
    PathBuf::from(format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        file.as_raw_fd()
    ))
}
pub(crate) fn open(path: &Path, binding: &InputBinding) -> io::Result<File> {
    binding.validate()?;
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file()
        || file.metadata()?.nlink() != 0
        || file.metadata()?.len() != binding.byte_length
        || !rustix::fs::fcntl_get_seals(&file)?.contains(SEALS)
    {
        return Err(io::Error::other(
            "native input is changed, truncated or unsealed",
        ));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(crate::job_guardian::protocol::INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != binding.byte_length
        || kinewright_hash::sha256_bytes(&bytes) != binding.sha256
    {
        return Err(io::Error::other("native input digest/length changed"));
    }
    file.rewind()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn te15_b2b0_sealed_readonly_exact_and_gone() {
        for bytes in [
            b"exact UTF-8 \"input\"\n".as_slice(),
            &vec![b'x'; 256 * 1024],
        ] {
            let binding = InputBinding::new(bytes).unwrap();
            let mut file = seal(bytes, &binding).unwrap();
            assert_eq!(file.stream_position().unwrap(), 0, "B2B0_ZERO_OFFSET");
            let flags = rustix::fs::fcntl_getfl(&file).unwrap();
            assert_eq!(
                flags & rustix::fs::OFlags::ACCMODE,
                rustix::fs::OFlags::RDONLY,
                "B2B0_READONLY"
            );
            assert!(
                rustix::fs::fcntl_get_seals(&file).unwrap().contains(SEALS),
                "B2B0_IMMUTABLE"
            );
            assert!(file.write_all(b"changed").is_err(), "B2B0_NO_WRITER");
            assert!(rustix::fs::ftruncate(&file, 0).is_err(), "B2B0_NO_TRUNCATE");
            let path = source(&file);
            let mut second = open(&path, &binding).unwrap();
            let mut actual = Vec::new();
            second.read_to_end(&mut actual).unwrap();
            assert_eq!(actual, bytes, "B2B0_SEALED_BYTES");
            let mut changed = binding.clone();
            changed.sha256 = "0".repeat(64);
            assert!(open(&path, &changed).is_err(), "B2B0_DIGEST_REFUSED");
            changed = binding.clone();
            changed.byte_length += 1;
            assert!(open(&path, &changed).is_err(), "B2B0_TRUNCATED_REFUSED");
            assert!(seal(b"changed", &binding).is_err(), "B2B0_SOURCE_REFUSED");
            let object = (
                file.metadata().unwrap().dev(),
                file.metadata().unwrap().ino(),
            );
            drop(second);
            drop(file);
            assert!(
                !std::fs::read_dir("/proc/self/fd")
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(|entry| std::fs::metadata(entry.path())
                        .is_ok_and(|m| (m.dev(), m.ino()) == object)),
                "B2B0_NO_INPUT_HANDLES"
            );
            assert!(
                !std::fs::metadata(&path).is_ok_and(|m| (m.dev(), m.ino()) == object),
                "B2B0_NO_INPUT_FILE"
            );
        }
        assert_eq!(
            crate::job_guardian::protocol::INPUT_TOO_LONG,
            "This feedback is too long to send in one run. Shorten or split the notes, then start the run again.",
            "B2B0_TRUE_WORDS"
        );
        let over = InputBinding::new(&vec![b'x'; 256 * 1024 + 1]);
        assert!(over.is_err(), "B2B0_INPUT_LIMIT");
        assert!(
            over.unwrap_err()
                .to_string()
                .contains(crate::job_guardian::protocol::INPUT_TOO_LONG)
        );
        let mut unsealed =
            File::from(rustix::fs::memfd_create(c"b2b0-unsealed", MemfdFlags::CLOEXEC).unwrap());
        unsealed.write_all(b"x").unwrap();
        assert!(
            open(&source(&unsealed), &InputBinding::new(b"x").unwrap()).is_err(),
            "B2B0_UNSEALED_REFUSED"
        );
        let path = std::env::current_exe().unwrap();
        let binding = InputBinding::new(b"x").unwrap();
        assert!(open(&path, &binding).is_err(), "B2B0_NAMED_FILE_REFUSED");
    }
    #[test]
    fn te15_b2b0_crash_leaves_no_input_to_reopen() {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        const MARKER: &str = "KINEWRIGHT_B2B0_CRASH_INPUT";
        if let Some(marker) = std::env::var_os(MARKER) {
            let file = seal(
                b"unpaid crash input",
                &InputBinding::new(b"unpaid crash input").unwrap(),
            )
            .unwrap();
            let marker = PathBuf::from(marker);
            let partial = marker.with_extension("partial");
            std::fs::write(&partial, source(&file).as_os_str().as_encoded_bytes()).unwrap();
            std::fs::rename(partial, &marker).unwrap();
            loop {
                std::thread::park_timeout(Duration::from_millis(10));
            }
        }
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "b2b0-crash",
        );
        let marker = temp.path("input");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "job_guardian::input::tests::te15_b2b0_crash_leaves_no_input_to_reopen",
                "--nocapture",
            ])
            .env(MARKER, &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() {
            if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("B2B0_CRASH_CHILD");
            }
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let path = PathBuf::from(std::fs::read_to_string(&marker).unwrap());
        let exists = path.exists();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(exists, "B2B0_ANONYMOUS_INPUT_LIVE");
        assert!(!path.exists(), "B2B0_CRASH_NO_INPUT");
        assert!(
            open(&path, &InputBinding::new(b"unpaid crash input").unwrap()).is_err(),
            "B2B0_RESTART_NO_OLD_INPUT"
        );
        assert_eq!(
            std::fs::read_dir(temp.root()).unwrap().count(),
            1,
            "B2B0_NO_STAGED_TEXT"
        );
    }
}

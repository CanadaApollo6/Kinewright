//! Fresh-process probe memory collector. Build every binary before running it.
#[cfg(target_os = "linux")]
mod native {
    use kinewright_job_sys::media_host;
    use kinewright_media_client::{Config, Limits, ProbeMemory, Request, Runtime, Source};
    use kinewright_media_ipc::Kind;
    use std::{
        fs::File,
        io,
        path::{Path, PathBuf},
        sync::{Arc, atomic::AtomicBool, mpsc},
    };
    struct Input {
        file: File,
        length: u64,
    }
    impl Source for Input {
        fn file(&self) -> &File {
            &self.file
        }
        fn identity(&self) -> [u8; 32] {
            [46; 32]
        }
        fn check(&self) -> Result<(), kinewright_core::MediaError> {
            if self
                .file
                .metadata()
                .is_ok_and(|data| data.len() == self.length)
            {
                Ok(())
            } else {
                Err(kinewright_core::MediaError::MediaAdmissionRequired)
            }
        }
    }
    fn fields(value: &media_host::MemoryProfile) -> String {
        format!(
            "rss={} pss={} private_clean={} private_dirty={} shared_clean={} shared_dirty={} vm_rss_status={} vm_hwm_status={} vm_peak={} vm_size={}",
            value.rss,
            value.pss,
            value.private_clean,
            value.private_dirty,
            value.shared_clean,
            value.shared_dirty,
            value.vm_rss_status,
            value.vm_hwm_status,
            value.vm_peak,
            value.vm_size
        )
    }
    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            media_host::children_peak_resident()?,
            0,
            "fresh collector app"
        );
        let source = PathBuf::from(std::env::args_os().nth(1).ok_or("source path required")?);
        let name = source
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("fixture name")?;
        let binaries = std::env::var_os("KINEWRIGHT_MS2_TEST_BIN_DIR")
            .map_or_else(
                || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release"),
                PathBuf::from,
            )
            .canonicalize()?;
        let (send, receive) = mpsc::sync_channel::<ProbeMemory>(1);
        let observer = Arc::new(move |memory| send.try_send(memory).map_err(io::Error::other));
        let config = Config::new(
            binaries.join("kinewright-media-helper"),
            binaries.join("kinewright-media-supervisor"),
            Limits::PROBE,
        )
        .observe(observer);
        let runtime = Runtime::start(config).map_err(|e| io::Error::other(format!("{e:?}")))?;
        let file = File::open(&source)?;
        let length = file.metadata()?.len();
        let outcome = runtime.probe(Request {
            source: Box::new(Input { file, length }),
            video: None,
            audio: None,
            cancel: Arc::new(AtomicBool::new(false)),
            explicit_retry: false,
        });
        let probe = outcome?;
        let expected = if name.starts_with("fhd-") {
            [1920, 1080]
        } else {
            [3840, 2160]
        };
        assert_eq!(probe.metadata().resolution, Some(expected));
        assert_eq!(
            probe.metadata().kind,
            if source
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
            {
                Kind::Video
            } else {
                Kind::Image
            }
        );
        assert!(
            !runtime.snapshot().degraded,
            "primary cgroup measurements required"
        );
        // The supervisor is still live and has not been waited for; this kernel
        // child peak accounts for the reaped shim/payload tree, excluding it.
        let kernel_peak = media_host::children_peak_resident()?;
        let memory = receive.try_recv()?;
        let group = memory.cgroup.ok_or("missing native cgroup measurement")?;
        for (key, value) in &group.events {
            if key == "oom_kill" || key == "oom" {
                assert_eq!(*value, 0);
            }
        }
        runtime
            .shutdown()
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        println!(
            "MS2_PROBE_MEMORY source={name} process=shim {}",
            fields(&memory.shim)
        );
        println!(
            "MS2_PROBE_MEMORY source={name} process=payload {}",
            fields(&memory.payload)
        );
        print!(
            "MS2_PROBE_GROUP source={name} current={} peak={} kernel_probe_child_peak={kernel_peak}",
            group.current, group.peak
        );
        for (key, value) in group.events {
            print!(" {key}={value}");
        }
        println!();
        Ok(())
    }
}
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    native::run()
}
#[cfg(not(target_os = "linux"))]
fn main() {
    std::process::exit(125);
}

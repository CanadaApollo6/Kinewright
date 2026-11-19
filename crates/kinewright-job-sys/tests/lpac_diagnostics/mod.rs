//! Temporary native experiment. Only an explicitly selected hosted witness creates a profile.

use super::{
    Canary, Fixture, SPIKE_CONTAINER_NAME, Sid, fixture_limits, query_container_storage, unique,
    wide,
};
use kinewright_job_sys::media_lpac::diagnostic::{Inspection, Variant, inspect};
use std::{
    collections::BTreeSet,
    env,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io,
    mem::size_of,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::{
    Wdk::{Foundation::OBJECT_ATTRIBUTES, System::Registry::NtOpenKey},
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, OBJ_CASE_INSENSITIVE,
            RtlNtStatusToDosError, UNICODE_STRING,
        },
        Security::{
            EqualSid,
            Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile},
        },
        Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT,
        System::Registry::{KEY_READ, RegCloseKey},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Selection {
    NoProfile,
    Plain,
    ParentEnvironment,
    Profile,
    ProfileProbe,
}

impl Selection {
    pub(super) fn from_env() -> Self {
        match env::var("KINEWRIGHT_LPAC_TEST_VARIANT").as_deref() {
            Ok("a" | "none") | Err(env::VarError::NotPresent) => Self::NoProfile,
            Ok("b") => Self::Plain,
            Ok("d") => Self::ParentEnvironment,
            Ok("f") => Self::Profile,
            value => panic!("invalid hosted LPAC selection {value:?}"),
        }
    }

    pub(super) fn launch_variant(self) -> Variant {
        match self {
            Self::NoProfile | Self::Profile | Self::ProfileProbe => Variant::Lpac,
            Self::Plain => Variant::Plain,
            Self::ParentEnvironment => Variant::ParentEnvironment,
        }
    }
}

pub(super) struct Profile {
    name: Vec<u16>,
    owned: bool,
}

impl Profile {
    pub(super) fn create(expected: &Sid) -> Result<Self, String> {
        let name = wide(SPIKE_CONTAINER_NAME);
        let mut returned = ptr::null_mut();
        // SAFETY: the name strings are terminated, capabilities are empty and returned is writable.
        let hr = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                name.as_ptr(),
                name.as_ptr(),
                ptr::null(),
                0,
                &raw mut returned,
            )
        };
        // An existing profile is somebody else's state. Never delete it.
        if hr != 0 {
            return Err(format!("CreateAppContainerProfile {}", hresult(hr)));
        }
        let owner = Self { name, owned: true };
        if returned.is_null() {
            return Err("CreateAppContainerProfile HRESULT 0 returned a null SID".into());
        }
        let returned = Sid(returned);
        // SAFETY: both APIs supplied valid and live SIDs.
        let equal = unsafe { EqualSid(returned.0, expected.0) != 0 };
        eprintln!(
            "CreateAppContainerProfile HRESULT 0 returned={} derived={} EqualSid={equal}",
            returned.text().display(),
            expected.text().display()
        );
        if !equal {
            return Err("CreateAppContainerProfile HRESULT 0 returned a different SID".into());
        }
        Ok(owner)
    }

    fn delete(&mut self) -> Result<(), String> {
        // SAFETY: the name is terminated and belongs solely to this profile owner.
        let hr = unsafe { DeleteAppContainerProfile(self.name.as_ptr()) };
        if hr != 0 {
            return Err(format!("DeleteAppContainerProfile {}", hresult(hr)));
        }
        self.owned = false;
        Ok(())
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        if self.owned {
            match self.delete() {
                Ok(()) => eprintln!("DeleteAppContainerProfile HRESULT 0 (cleanup)"),
                Err(error) => {
                    eprintln!("{error}; retrying cleanup");
                    if let Err(error) = self.delete() {
                        eprintln!("{error}");
                        assert!(std::thread::panicking(), "profile cleanup failed: {error}");
                    }
                }
            }
        }
    }
}

fn hresult(status: i32) -> String {
    let raw = status.cast_unsigned();
    if raw >> 16 == 0x8007 || raw == 0 {
        format!("HRESULT {raw:#010x}, Win32 {}", raw & 0xffff)
    } else {
        format!("HRESULT {raw:#010x}")
    }
}

pub(super) fn run() {
    let mut fixture = Fixture::stage();
    describe_folder("pre-experiment", &fixture.sid.text());
    let attempts = [
        ("a LPAC minimal, no profile", Variant::Lpac),
        ("b plain AppContainer, no profile", Variant::Plain),
        (
            "c security capabilities only, no profile or inheritance",
            Variant::SecurityOnly,
        ),
        (
            "d LPAC parent environment, no profile",
            Variant::ParentEnvironment,
        ),
    ];
    let [a, b, _c, d] = attempts.map(|(label, variant)| {
        let outcome = inspect_variant(&fixture, variant);
        print_variant(label, &outcome);
        outcome.ok().map(|inspection| inspection.verified)
    });

    fixture.variant = Selection::ProfileProbe;
    let (f, cleanup) = profile_variant(&fixture);
    let choices = [("a", a), ("d", d), ("f", f), ("b", b)];
    // Prefer a full suspended verification. If all inspections fail, keep the closest
    // successful creation so the original witnesses expose their actual inspection failure.
    let selected = choices
        .iter()
        .find(|(_, result)| *result == Some(true))
        .or_else(|| choices.iter().find(|(_, result)| result.is_some()))
        .map_or("none", |(letter, _)| *letter);
    eprintln!("LPAC_TEST_VARIANT={selected}");
    if let Some(path) = env::var_os("KINEWRIGHT_LPAC_SELECTION_FILE") {
        fs::write(path, selected).expect("WriteFile (hosted LPAC selection)");
    }
    assert!(cleanup.is_ok(), "{cleanup:?}");
}

fn print_variant(label: &str, result: &Result<Inspection, String>) {
    eprintln!(
        "LPAC diagnostic {label}: {}",
        match result {
            Ok(inspection) => &inspection.detail,
            Err(error) => error,
        }
    );
}

fn inspect_variant(fixture: &Fixture, variant: Variant) -> Result<Inspection, String> {
    let source = File::open(&fixture.source)
        .map_err(|error| format!("CreateFileW (diagnostic source): {error}"))?;
    let (request, _write) =
        io::pipe().map_err(|error| format!("CreatePipe (diagnostic request): {error}"))?;
    let (_read, reply) =
        io::pipe().map_err(|error| format!("CreatePipe (diagnostic reply): {error}"))?;
    let args = [
        OsString::from("ready"),
        fixture.image_dir().as_os_str().to_os_string(),
    ];
    inspect(
        &fixture.exe,
        &args,
        (&source, &request, &reply),
        fixture_limits()?,
        variant,
    )
    .map_err(|error| error.to_string())
}

fn profile_variant(fixture: &Fixture) -> (Option<bool>, Result<(), String>) {
    let mut profile = match Profile::create(&fixture.sid) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("LPAC diagnostic e: {error}");
            eprintln!("LPAC diagnostic f: same inputs as e; {error}");
            return (None, Ok(()));
        }
    };
    let outcome = inspect_variant(fixture, Variant::Lpac);
    print_variant("e LPAC minimal, with profile", &outcome);
    print_variant("f identical to e (already SystemRoot only)", &outcome);

    let sid = fixture.sid.text();
    let (locations, registry) = observe_profile_storage(fixture, &sid, outcome.is_ok());

    if let Some(name) = &registry {
        eprintln!("pre-delete registry {name}: {}", registry_state(name));
    }
    let cleanup = profile.delete().or_else(|error| {
        eprintln!("{error}; retrying cleanup before the checks");
        profile.delete()
    });
    match &cleanup {
        Ok(()) => eprintln!("DeleteAppContainerProfile HRESULT 0"),
        Err(error) => eprintln!("{error}"),
    }
    // Inspect after releasing every child and registry handle. These are read-only probes.
    for path in locations {
        eprintln!(
            "post-delete folder {} {}",
            path.display(),
            folder_state(&path)
        );
    }
    describe_folder("post-delete", &sid);
    if let Some(name) = registry {
        eprintln!("post-delete registry {name}: {}", registry_state(&name));
    } else {
        eprintln!(
            "post-delete registry storage unverified: child did not report an exact key name"
        );
    }
    (outcome.ok().map(|inspection| inspection.verified), cleanup)
}

fn describe_folder(step: &str, sid: &OsStr) {
    match query_container_storage(sid) {
        Ok(Some(path)) => eprintln!(
            "{step} GetAppContainerFolderPath HRESULT 0 path={} {}",
            path.display(),
            folder_state(&path)
        ),
        Ok(None) => eprintln!("{step} GetAppContainerFolderPath HRESULT 0 returned a null path"),
        Err(hr) => eprintln!("{step} GetAppContainerFolderPath {}", hresult(hr)),
    }
}

fn observe_profile_storage(
    fixture: &Fixture,
    sid: &OsStr,
    created: bool,
) -> (BTreeSet<PathBuf>, Option<String>) {
    let mut locations = BTreeSet::new();
    let mut registry = None;
    match query_container_storage(sid) {
        Ok(Some(path)) if path.is_absolute() => {
            eprintln!(
                "e/f GetAppContainerFolderPath HRESULT 0 path={}",
                path.display()
            );
            let root = path.ancestors().find(|dir| {
                dir.file_name().is_some_and(|name| {
                    name.eq_ignore_ascii_case(SPIKE_CONTAINER_NAME)
                        || name.eq_ignore_ascii_case(sid)
                })
            });
            if let Some(root) = root {
                collect_storage(root, &mut locations);
                if folder_state(root) == "directory"
                    && path
                        .ancestors()
                        .take_while(|dir| *dir != root)
                        .all(|dir| folder_state(dir) == "directory")
                {
                    locations.insert(path);
                }
            } else {
                eprintln!(
                    "e/f storage writes unavailable: folder path did not identify the owned profile"
                );
            }
        }
        Ok(_) => eprintln!("e/f GetAppContainerFolderPath HRESULT 0 returned an invalid path"),
        Err(hr) => eprintln!("e/f GetAppContainerFolderPath {}", hresult(hr)),
    }

    if created {
        for path in &locations {
            let state = folder_state(path);
            if state == "directory" {
                let canary = Canary::new(path);
                match fixture.storage_probe("create", &canary.0) {
                    Ok(report) => eprintln!(
                        "e/f storage {} {state}: {} Win32 {}; canary={}",
                        path.display(),
                        report.api,
                        report.code,
                        folder_state(&canary.0)
                    ),
                    Err(error) => eprintln!("e/f storage {} {state}: {error}", path.display()),
                }
            } else {
                eprintln!("e/f storage {} {state}", path.display());
            }
        }
        match fixture.try_single("profile-registry", unique(), true) {
            Ok(report) => {
                eprintln!(
                    "e/f registry: {} code {} ({:#010x}); location={:?}",
                    report.api, report.code, report.code, report.detail
                );
                registry = report.detail.filter(|value| {
                    value.starts_with(r"\REGISTRY\") || value.starts_with(r"\Registry\")
                });
            }
            Err(error) => eprintln!("e/f registry: {error}"),
        }
    } else {
        for path in &locations {
            eprintln!(
                "e/f storage {} {}; create unavailable after launch failure",
                path.display(),
                folder_state(path)
            );
        }
        eprintln!("e/f GetAppContainerRegistryLocation unavailable after launch failure");
    }
    (locations, registry)
}

fn collect_storage(root: &Path, locations: &mut BTreeSet<PathBuf>) {
    locations.insert(root.into());
    if folder_state(root) != "directory" {
        return;
    }
    let ac = root.join("AC");
    let mut pending = vec![ac];
    while let Some(path) = pending.pop() {
        if folder_state(&path) == "directory" {
            match fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(entry) if entry.file_type().is_ok_and(|kind| kind.is_dir()) => {
                                pending.push(entry.path());
                            }
                            Ok(_) => {}
                            Err(error) => eprintln!("FindNextFileW (profile storage): {error}"),
                        }
                    }
                }
                Err(error) => eprintln!("FindFirstFileW (profile storage): {error}"),
            }
        }
        locations.insert(path);
    }
}

fn folder_state(path: &Path) -> String {
    match fs::symlink_metadata(path) {
        Ok(info) if info.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
            "reparse point".into()
        }
        Ok(info) if info.is_dir() => "directory".into(),
        Ok(_) => "file".into(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => "absent".into(),
        Err(error) => format!("GetFileAttributesExW: {error}"),
    }
}

fn registry_state(name: &str) -> String {
    let mut name = wide(OsStr::new(name));
    let Ok(length) = u16::try_from((name.len() - 1) * 2) else {
        return "NtOpenKey: key name too long".into();
    };
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>()
            .try_into()
            .expect("OBJECT_ATTRIBUTES size"),
        ObjectName: &raw const name,
        Attributes: OBJ_CASE_INSENSITIVE,
        ..Default::default()
    };
    let mut handle = ptr::null_mut();
    // SAFETY: the name and attributes are live, handle is writable, and the open requests read access.
    let status = unsafe { NtOpenKey(&raw mut handle, KEY_READ, &raw const attributes) };
    if status >= 0 {
        // SAFETY: success transferred this key handle.
        unsafe { RegCloseKey(handle) };
        return "NtOpenKey Win32 0 (still present)".into();
    }
    // SAFETY: the conversion takes an NTSTATUS value with no pointer arguments.
    let code = unsafe { RtlNtStatusToDosError(status) };
    let state = if matches!(code, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) {
        "absent"
    } else {
        "unverified"
    };
    format!(
        "NtOpenKey Win32 {code}, NTSTATUS {:#010x} ({state})",
        status.cast_unsigned()
    )
}

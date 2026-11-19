//! Temporary hosted witnesses and diagnostics for the spike-only profile.

use super::{
    Canary, Fixture, SPIKE_CONTAINER_NAME, Sid, fixture_limits, query_container_storage, unique,
    wide,
};
use kinewright_job_sys::media_lpac::diagnostic::{
    CANDIDATE_ENVIRONMENT, Inspection, ProfileRegistryLocation, Variant, inspect,
};
use std::{
    collections::BTreeSet,
    env,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::ERROR_ALREADY_EXISTS,
    Security::{
        EqualSid,
        Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile},
    },
    Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT,
};

mod hardening;

pub(super) fn grant_package_read(directory: &Path, file: &Path) -> Result<(), String> {
    hardening::grant_package_read(directory, file)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfileEnvironment {
    Parent,
    Added(u8),
    OwnedLocal,
}

impl ProfileEnvironment {
    fn variant(self, fixture: &Fixture) -> Variant<'_> {
        match self {
            Self::Parent => Variant::ParentEnvironment,
            Self::Added(mask) => Variant::AddedEnvironment(mask),
            Self::OwnedLocal => Variant::ProfileEnvironment {
                local: &fixture.private_local,
                lpac: true,
            },
        }
    }

    fn selection(self) -> String {
        match self {
            Self::Parent => "g".into(),
            Self::Added(0) => "f".into(),
            Self::Added(mask) => format!("h:{mask}"),
            Self::OwnedLocal => "h:owned-local".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfileMode {
    Probe,
    Full,
    Hardened,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Selection {
    NoProfile,
    Plain,
    ParentEnvironment,
    PackageControl(ProfileEnvironment),
    Profile(ProfileEnvironment, ProfileMode),
}

impl Selection {
    fn from_env(value: &str) -> Self {
        let (value, mode) = value
            .strip_prefix("j:")
            .map_or((value, ProfileMode::Full), |env| {
                (env, ProfileMode::Hardened)
            });
        match value {
            "a" | "none" => Self::NoProfile,
            "b" => Self::Plain,
            "d" => Self::ParentEnvironment,
            "f" => Self::Profile(ProfileEnvironment::Added(0), mode),
            "g" => Self::Profile(ProfileEnvironment::Parent, mode),
            "h:owned-local" => Self::Profile(ProfileEnvironment::OwnedLocal, mode),
            value if value.starts_with("h:") => {
                let mask = value[2..]
                    .parse::<u8>()
                    .expect("hosted LPAC environment mask");
                assert!(usize::from(mask) < (1 << CANDIDATE_ENVIRONMENT.len()));
                Self::Profile(ProfileEnvironment::Added(mask), mode)
            }
            _ => panic!("invalid hosted LPAC selection {value:?}"),
        }
    }

    pub(super) fn launch_variant(self, fixture: &Fixture) -> Variant<'_> {
        match self {
            Self::NoProfile => Variant::Lpac,
            Self::Plain => Variant::Plain,
            Self::ParentEnvironment => Variant::ParentEnvironment,
            Self::Profile(environment, _) => environment.variant(fixture),
            Self::PackageControl(environment) => plain_variant(fixture, environment),
        }
    }

    pub(super) fn uses_extra_environment(self) -> bool {
        match self {
            Self::ParentEnvironment
            | Self::Profile(ProfileEnvironment::Parent | ProfileEnvironment::OwnedLocal, _) => true,
            Self::Profile(ProfileEnvironment::Added(mask), _) => mask != 0,
            _ => false,
        }
    }
}

fn plain_variant(fixture: &Fixture, environment: ProfileEnvironment) -> Variant<'_> {
    Variant::ProfileEnvironment {
        local: if environment == ProfileEnvironment::OwnedLocal {
            &fixture.private_local
        } else {
            &fixture.real_local
        },
        lpac: false,
    }
}

pub(super) fn witness_environment(fixture: &mut Fixture) -> ProfileEnvironment {
    let original = fixture.variant;
    fixture.variant = Selection::Profile(ProfileEnvironment::OwnedLocal, ProfileMode::Probe);
    let trial = fixture.try_single("profile-environment", "", true);
    fixture.variant = original;
    let started = trial
        .as_ref()
        .is_ok_and(|report| report.api == "GetEnvironmentStringsW" && report.code == 0);
    report_probe(
        "h owned LOCALAPPDATA",
        &fixture.private_local.display().to_string(),
        trial,
    );
    if started {
        ProfileEnvironment::OwnedLocal
    } else {
        ProfileEnvironment::Added(1)
    }
}

pub(super) struct Profile {
    name: Vec<u16>,
    owned: bool,
    _lease: std::sync::MutexGuard<'static, ()>,
}

static PROFILE_LEASE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn clear_stale_profile() -> Result<(), String> {
    let name = wide(SPIKE_CONTAINER_NAME);
    // SAFETY: this always targets the exact spike-only profile name, never a discovered name.
    let hr = unsafe { DeleteAppContainerProfile(name.as_ptr()) };
    match hr.cast_unsigned() {
        0 => {
            eprintln!("DeleteAppContainerProfile (stale spike) HRESULT 0");
            Ok(())
        }
        0x8007_0002 | 0x8007_0003 | 0x8007_0490 => Ok(()),
        _ => Err(format!(
            "DeleteAppContainerProfile (stale spike) {}",
            hresult(hr)
        )),
    }
}

impl Profile {
    pub(super) fn create(expected: &Sid) -> Result<Self, String> {
        let lease = PROFILE_LEASE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_stale_profile()?;
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
        if hr != 0 {
            let raced = hr.cast_unsigned() & 0xffff == ERROR_ALREADY_EXISTS;
            return Err(format!(
                "CreateAppContainerProfile {} (concurrent owner={raced})",
                hresult(hr)
            ));
        }
        let owner = Self {
            name,
            owned: true,
            _lease: lease,
        };
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
    let lease = PROFILE_LEASE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let initial_cleanup = clear_stale_profile();
    if let Err(error) = &initial_cleanup {
        eprintln!("{error}; pre-experiment profile state unverified");
    }
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
    for (label, variant) in attempts {
        print_variant(label, &inspect_variant(&fixture, variant));
    }

    drop(lease);
    let cleanup = profile_variants(&mut fixture);
    assert!(
        initial_cleanup.is_ok() && cleanup.is_ok(),
        "pre-cleanup={initial_cleanup:?} post-cleanup={cleanup:?}"
    );
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

fn inspect_variant(fixture: &Fixture, variant: Variant<'_>) -> Result<Inspection, String> {
    with_launch_inputs(fixture, |inputs, limits| {
        let args = [
            OsString::from("ready"),
            fixture.image_dir().as_os_str().to_os_string(),
        ];
        inspect(&fixture.exe, &args, inputs, limits, variant)
    })
}

fn with_launch_inputs<T>(
    fixture: &Fixture,
    call: impl FnOnce((&File, &io::PipeReader, &io::PipeWriter), super::Limits) -> io::Result<T>,
) -> Result<T, String> {
    let source = File::open(&fixture.source)
        .map_err(|error| format!("CreateFileW (diagnostic source): {error}"))?;
    let (request, _write) =
        io::pipe().map_err(|error| format!("CreatePipe (diagnostic request): {error}"))?;
    let (_read, reply) =
        io::pipe().map_err(|error| format!("CreatePipe (diagnostic reply): {error}"))?;
    let _spawning = super::SPAWNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    call((&source, &request, &reply), fixture_limits()?).map_err(|error| error.to_string())
}

fn profile_registry(
    fixture: &Fixture,
    environment: ProfileEnvironment,
) -> Result<ProfileRegistryLocation, String> {
    use kinewright_job_sys::media_lpac::diagnostic::{
        profile_registry_by_sid, profile_registry_location,
    };
    let lookup = |variant| {
        with_launch_inputs(fixture, |inputs, limits| {
            profile_registry_location(&fixture.exe, inputs, limits, variant)
        })
    };
    for (label, variant) in [
        ("LPAC", environment.variant(fixture)),
        ("ordinary AppContainer", plain_variant(fixture, environment)),
    ] {
        match lookup(variant) {
            Ok(key) => {
                eprintln!(
                    "GetAppContainerRegistryLocation (host {label}) HRESULT 0; NtQueryKey Win32 0 path={}",
                    key.name
                );
                return Ok(key);
            }
            Err(error) => eprintln!("profile registry lookup (host {label}): {error}"),
        }
    }
    let key = profile_registry_by_sid(&fixture.sid.text()).map_err(|error| error.to_string())?;
    eprintln!(
        "RegOpenKeyExW (host SID lookup) Win32 0; NtQueryKey Win32 0 path={}",
        key.name
    );
    Ok(key)
}

fn profile_variants(fixture: &mut Fixture) -> Result<(), String> {
    let mut profile = match Profile::create(&fixture.sid) {
        Ok(profile) => profile,
        Err(error) => {
            for letter in ["e", "f", "g", "h", "i", "j"] {
                eprintln!("LPAC diagnostic {letter}: {error}");
            }
            return Ok(());
        }
    };
    let minimum = inspect_variant(fixture, Variant::Lpac);
    print_variant("e LPAC minimal, with profile", &minimum);
    print_variant("f identical to e (already SystemRoot only)", &minimum);
    let inherited = inspect_variant(fixture, Variant::ParentEnvironment);
    print_variant("g LPAC parent environment, with profile", &inherited);
    if inherited.is_ok() {
        fixture.variant = Selection::Profile(ProfileEnvironment::Parent, ProfileMode::Probe);
        report_probe(
            "g",
            "profile-environment",
            fixture.try_single("profile-environment", "", true),
        );
    }
    let subset = search_environment(fixture, minimum.is_ok());
    let owned = witness_environment(fixture);
    let best = (owned == ProfileEnvironment::OwnedLocal)
        .then_some(owned)
        .or(subset)
        .or_else(|| inherited.is_ok().then_some(ProfileEnvironment::Parent));
    let observation = compare_storage(fixture, best);
    if let Ok(value) = env::var("KINEWRIGHT_LPAC_TEST_VARIANT") {
        let selection = Selection::from_env(&value);
        if matches!(selection, Selection::Profile(..)) {
            print_variant(
                &format!("override {value}"),
                &inspect_variant(fixture, selection.launch_variant(fixture)),
            );
        }
    }
    let cleanup = profile.delete().or_else(|error| {
        eprintln!("{error}; retrying cleanup before the checks");
        profile.delete()
    });
    match &cleanup {
        Ok(()) => eprintln!("DeleteAppContainerProfile HRESULT 0"),
        Err(error) => eprintln!("{error}"),
    }
    describe_folder("post-delete", &fixture.sid.text());
    for path in observation.folders {
        eprintln!(
            "post-delete folder {} {}",
            path.display(),
            folder_state(&path)
        );
    }
    if let Some(name) = observation.registry {
        eprintln!(
            "post-delete registry {name}: {}",
            hardening::registry_state(&name)
        );
    } else {
        eprintln!(
            "post-delete registry storage unverified: host lookup did not confirm an exact key name"
        );
    }
    cleanup
}

fn search_environment(
    fixture: &mut Fixture,
    system_root_worked: bool,
) -> Option<ProfileEnvironment> {
    let available = CANDIDATE_ENVIRONMENT
        .iter()
        .enumerate()
        .filter(|(_, key)| env::var_os(key).is_some())
        .fold(0u8, |mask, (bit, _)| mask | (1 << bit));
    let mut attempts = 0;
    let mut last = String::from("no candidates present");
    for size in 0..=available.count_ones() {
        for mask in 0..=available {
            if mask & !available != 0
                || mask.count_ones() != size
                || (mask == 0 && !system_root_worked)
            {
                continue;
            }
            let configuration = ProfileEnvironment::Added(mask);
            let outcome = inspect_variant(fixture, configuration.variant(fixture));
            attempts += 1;
            match outcome {
                Ok(inspection) => {
                    let keys = ["SystemRoot"]
                        .into_iter()
                        .chain(
                            CANDIDATE_ENVIRONMENT
                                .into_iter()
                                .enumerate()
                                .filter_map(|(bit, key)| (mask & (1 << bit) != 0).then_some(key)),
                        )
                        .collect::<Vec<_>>();
                    eprintln!(
                        "LPAC diagnostic h smallest successful candidate subset {keys:?} after {attempts} attempts: {}",
                        inspection.detail
                    );
                    fixture.variant = Selection::Profile(configuration, ProfileMode::Probe);
                    report_probe(
                        "h",
                        "profile-environment",
                        fixture.try_single("profile-environment", "", true),
                    );
                    return Some(configuration);
                }
                Err(error) => last = error,
            }
        }
    }
    eprintln!(
        "LPAC diagnostic h: no candidate subset created a child ({attempts} attempts after SystemRoot baseline); last={last}"
    );
    None
}

#[derive(Default)]
struct ProfileObservation {
    folders: BTreeSet<PathBuf>,
    registry: Option<String>,
}

fn compare_storage(fixture: &mut Fixture, best: Option<ProfileEnvironment>) -> ProfileObservation {
    let mut observation = ProfileObservation {
        folders: profile_folders(&fixture.sid),
        ..Default::default()
    };
    let Some(environment) = best else {
        eprintln!("LPAC diagnostic i: CreateProcessW unavailable in g and h");
        eprintln!("LPAC diagnostic j: CreateProcessW unavailable in g and h");
        return observation;
    };
    fixture.variant = Selection::Profile(environment, ProfileMode::Probe);
    print_variant(
        &format!("i unhardened profile with {}", environment.selection()),
        &inspect_variant(fixture, environment.variant(fixture)),
    );
    let registry = profile_registry(fixture, environment);
    observation.registry = registry.as_ref().ok().map(|key| key.name.clone());
    probe_registry(fixture, "i");
    let seeds = seed_files(&observation.folders);
    let value = unique();
    let registry_value = observation
        .registry
        .as_deref()
        .map(|name| (name, value.as_str()));
    probe_storage(fixture, "i", &observation.folders, &seeds, registry_value);

    // Include directories materialized by the first real confined operations.
    observation.folders.extend(profile_folders(&fixture.sid));
    let hardened = apply_hardening(
        fixture,
        &observation.folders,
        registry.as_ref().map(|key| key.name.as_str()),
    );
    let launch = inspect_variant(fixture, environment.variant(fixture));
    eprintln!(
        "LPAC diagnostic j: profile ACL writes {}; {}",
        hardened.status(),
        match &launch {
            Ok(result) => result.detail.as_str(),
            Err(error) => error,
        },
    );
    if launch.is_ok() {
        probe_registry(fixture, "j");
        probe_storage(fixture, "j", &observation.folders, &seeds, registry_value);
        probe_restoration(fixture, hardened);
    }
    observation
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

fn profile_folders(sid: &Sid) -> BTreeSet<PathBuf> {
    let sid = sid.text();
    let mut folders = BTreeSet::new();
    match query_container_storage(&sid) {
        Ok(Some(path)) if path.is_absolute() => {
            eprintln!(
                "profile GetAppContainerFolderPath HRESULT 0 path={}",
                path.display()
            );
            if let Some(root) = path.ancestors().find(|dir| {
                dir.file_name().is_some_and(|name| {
                    name.eq_ignore_ascii_case(SPIKE_CONTAINER_NAME)
                        || name.eq_ignore_ascii_case(&sid)
                })
            }) {
                collect_folders(root, &mut folders);
            }
        }
        Ok(_) => eprintln!("profile GetAppContainerFolderPath HRESULT 0 returned an invalid path"),
        Err(hr) => eprintln!("profile GetAppContainerFolderPath {}", hresult(hr)),
    }
    folders
}

fn collect_folders(root: &Path, folders: &mut BTreeSet<PathBuf>) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let state = folder_state(&path);
        eprintln!("profile folder {} {state}", path.display());
        if state == "directory" {
            match fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(entry)
                                if entry
                                    .file_type()
                                    .is_ok_and(|kind| kind.is_dir() || kind.is_symlink()) =>
                            {
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
        folders.insert(path);
    }
}

fn report_probe(stage: &str, target: &str, result: Result<super::Report, String>) {
    match result {
        Ok(reply) => eprintln!(
            "{stage} [{target}] {} code {} ({:#010x}) {}",
            reply.api,
            reply.code,
            reply.code,
            reply.detail.as_deref().unwrap_or("")
        ),
        Err(error) => eprintln!("{stage} [{target}] {error}"),
    }
}

fn probe_registry(fixture: &Fixture, stage: &str) {
    report_probe(
        stage,
        "profile registry create subkey",
        fixture.try_single("profile-registry", unique(), true),
    );
}

fn seed_files(folders: &BTreeSet<PathBuf>) -> Vec<Canary> {
    folders
        .iter()
        .filter(|path| folder_state(path) == "directory")
        .filter_map(|path| {
            let canary = Canary::new(path);
            match fs::write(&canary.0, b"baseline") {
                Ok(()) => Some(canary),
                Err(error) => {
                    eprintln!(
                        "WriteFile (host profile seed {}) {error}",
                        canary.0.display()
                    );
                    None
                }
            }
        })
        .collect()
}

fn probe_storage(
    fixture: &Fixture,
    stage: &str,
    folders: &BTreeSet<PathBuf>,
    seeds: &[Canary],
    registry_value: Option<(&str, &str)>,
) {
    for path in folders
        .iter()
        .filter(|path| folder_state(path) == "directory")
    {
        let canary = Canary::new(path);
        report_probe(
            stage,
            &format!("create {}", canary.0.display()),
            fixture.storage_probe("create", &canary.0),
        );
    }
    for seed in seeds {
        for case in ["grow", "mutate"] {
            if let Err(error) = fs::write(&seed.0, b"baseline") {
                eprintln!(
                    "{stage} WriteFile (reset host seed {}) {error}",
                    seed.0.display()
                );
                continue;
            }
            report_probe(
                stage,
                &format!("{case} {}", seed.0.display()),
                fixture.storage_probe(case, &seed.0),
            );
        }
    }
    report_probe(
        stage,
        "new registry value",
        fixture.try_single("profile-registry-value", unique(), true),
    );
    if let Some((key, name)) = registry_value {
        for case in ["profile-registry-grow", "profile-registry-change"] {
            match hardening::seed_registry(key, name) {
                Ok(()) => report_probe(
                    stage,
                    &format!("{case} {key} {name}"),
                    fixture.try_single(case, name, true),
                ),
                Err(error) => eprintln!("{stage} {case} {key} {name}: {error}"),
            }
        }
    } else {
        eprintln!("{stage} registry grow/change unavailable: no exact key name for host seed");
    }
    report_probe(
        stage,
        "object namespace",
        fixture.try_single("profile-objects", unique(), true),
    );
}

#[derive(Default)]
struct ProfileHardening {
    first_failure: Option<String>,
    files: Vec<(PathBuf, String)>,
    registry: Option<String>,
}

impl ProfileHardening {
    fn fail(&mut self, error: String) {
        eprintln!("j profile hardening: {error}");
        self.first_failure.get_or_insert(error);
    }

    fn status(&self) -> String {
        self.first_failure.as_ref().map_or_else(
            || "complete=true".into(),
            |error| format!("complete=false step=[{error}]"),
        )
    }
}

fn apply_hardening(
    fixture: &Fixture,
    folders: &BTreeSet<PathBuf>,
    registry: Result<&str, &String>,
) -> ProfileHardening {
    let mut result = ProfileHardening::default();
    if folders.is_empty() {
        result.fail("GetAppContainerFolderPath did not identify a profile folder tree".into());
    }
    // Children first preserves the original descriptors before parent inheritance changes.
    for path in folders.iter().rev() {
        let state = folder_state(path);
        if state != "directory" {
            result.fail(format!(
                "inspect profile directory {}: {state}",
                path.display()
            ));
            continue;
        }
        match fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries {
                    match entry.and_then(|entry| Ok((entry.path(), entry.file_type()?))) {
                        Ok((entry, kind)) if kind.is_file() && folder_state(&entry) == "file" => {
                            record_file_acl(&mut result, &entry, false, &fixture.sid);
                        }
                        Ok((_, kind)) if kind.is_dir() => {}
                        Ok((entry, _)) => result.fail(format!(
                            "inspect unsupported profile entry {}",
                            entry.display()
                        )),
                        Err(error) => {
                            result.fail(format!("FindNextFileW {}: {error}", path.display()));
                        }
                    }
                }
            }
            Err(error) => result.fail(format!("FindFirstFileW {}: {error}", path.display())),
        }
        record_file_acl(&mut result, path, true, &fixture.sid);
    }
    let failed_sd = result
        .files
        .iter()
        .filter_map(|(path, _)| match hardening::final_file_sddl(path) {
            Ok(sddl) => {
                eprintln!("j final SDDL after inheritance [{}] {sddl}", path.display());
                None
            }
            Err(error) => Some(format!(
                "final SDDL [{}] unavailable: {error}",
                path.display()
            )),
        })
        .collect::<Vec<_>>();
    for error in failed_sd {
        result.fail(error);
    }
    match registry {
        Ok(name) => match hardening::registry(name, &fixture.sid) {
            Ok(sddl) => result.registry = Some(sddl),
            Err(error) => result.fail(format!("registry {name}: {error}")),
        },
        Err(error) => result.fail(error.clone()),
    }
    result
}

fn record_file_acl(result: &mut ProfileHardening, path: &Path, is_dir: bool, sid: &Sid) {
    match hardening::file(path, is_dir, sid) {
        Ok(sddl) => result.files.push((path.into(), sddl)),
        Err(error) => result.fail(format!("folder/file {}: {error}", path.display())),
    }
}

fn probe_restoration(fixture: &Fixture, hardening: ProfileHardening) {
    for (path, sddl) in hardening.files {
        let mut target = path.as_os_str().to_os_string();
        target.push("\t");
        target.push(sddl);
        report_probe(
            "j",
            &format!("restore {}", path.display()),
            fixture.try_single("profile-file-restore", target, true),
        );
    }
    if let Some(sddl) = hardening.registry {
        report_probe(
            "j",
            "restore registry DACL",
            fixture.try_single("profile-registry-restore", sddl, true),
        );
    }
}

pub(super) fn harden_witness(
    fixture: &mut Fixture,
    environment: ProfileEnvironment,
) -> Result<(), String> {
    let registry = profile_registry(fixture, environment);
    let applied = apply_hardening(
        fixture,
        &profile_folders(&fixture.sid),
        registry.as_ref().map(|key| key.name.as_str()),
    );
    fixture.variant = Selection::Profile(environment, ProfileMode::Hardened);
    applied.first_failure.map_or(Ok(()), |error| {
        Err(format!(
            "j fresh profile hardening complete=false step=[{error}]"
        ))
    })
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

#![cfg(windows)]
#![allow(unsafe_code)]

use kinewright_job_sys::media_lpac::{
    self, Inputs, Limits, Preparation, Profile, Step, VerifiedProfile,
};
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, PipeReader, PipeWriter, Read, Write},
    net::{Ipv4Addr, TcpListener},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Component, Path, PathBuf, Prefix},
    process::{Command, Stdio},
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_ACCESS_DISABLED_BY_POLICY, ERROR_CHILD_PROCESS_BLOCKED,
        ERROR_COMMITMENT_LIMIT, ERROR_FILE_NOT_FOUND, ERROR_INVALID_FUNCTION, ERROR_INVALID_HANDLE,
        ERROR_NOT_ENOUGH_MEMORY, ERROR_NOT_ENOUGH_QUOTA, ERROR_NOT_FOUND, ERROR_PATH_NOT_FOUND,
        ERROR_WORKING_SET_QUOTA, INVALID_HANDLE_VALUE, LocalFree, MAX_PATH, WAIT_TIMEOUT,
    },
    Security::{
        ACL,
        Authorization::{
            ConvertSidToStringSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW,
            SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
            TRUSTEE_IS_USER,
        },
        DACL_SECURITY_INFORMATION, FreeSid,
        Isolation::{DeriveAppContainerSidFromAppContainerName, GetAppContainerFolderPath},
        PSECURITY_DESCRIPTOR, PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    },
    Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        PIPE_ACCESS_INBOUND,
    },
    System::{
        Com::CoTaskMemFree,
        Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PeekNamedPipe},
        Registry::{KEY_CREATE_SUB_KEY, KEY_READ},
        Threading::{ALL_PROCESSOR_GROUPS, DETACHED_PROCESS, GetActiveProcessorCount},
    },
};

#[path = "fixtures/profile_controls.rs"]
mod profile_controls;

const TEST_PROFILE: &str = "kinewright.mediaprobe.launchtest";

const TIMEOUT: Duration = Duration::from_secs(8);
const MEMORY_BYTES: usize = 64 * 1024 * 1024;
const SOURCE: &[u8] = b"LPAC source bytes";
const RETAINED_AC_FOLDERS: [&str; 4] = ["INetCache", "INetCookies", "INetHistory", "Temp"];
const EXIT_AFTER_MEMORY_LIMIT: u32 = ERROR_NOT_ENOUGH_MEMORY;
const FILE_DENIALS: &[u32] = &[ERROR_ACCESS_DENIED];
static SERIAL: AtomicU64 = AtomicU64::new(0);
// std::process::Command and the LPAC launcher briefly own inheritable duplicates.
static SPAWNING: Mutex<()> = Mutex::new(());

#[test]
fn suspended_lpac_passes_inspection_and_runs_with_minimal_environment() {
    let fixture = Fixture::new();
    let unconfined = fixture.startup(false);
    let confined = fixture.startup(true);
    assert!(
        unconfined.is_ok() && confined.is_ok(),
        "unconfined startup: {unconfined:?}; LPAC startup: {confined:?}"
    );
}

#[test]
fn only_ordinary_appcontainer_reads_the_all_application_packages_canary() {
    let fixture = Fixture::new();
    let directory = fixture.root.0.join("all application packages control");
    fs::create_dir(&directory).expect("CreateDirectoryW (ALL APPLICATION PACKAGES canary)");
    let file = directory.join("read-only.txt");
    fs::write(&file, b"packages-only").expect("WriteFile (ALL APPLICATION PACKAGES canary)");
    profile_controls::grant_package_read(&directory, &file);
    let ordinary = profile_controls::plain_read(&fixture, &file);
    let lpac = fixture.try_single("package-only-read", &file, true);
    eprintln!("ordinary AppContainer package-only read: {ordinary:?}");
    eprintln!("LPAC package-only read: {lpac:?}");
    assert_eq!(
        ordinary.expect("ordinary AppContainer control"),
        0,
        "ordinary AppContainer reads exact canary bytes"
    );
    lpac.expect("LPAC package-only canary").expect(
        &["CreateFileW(package-only canary)"],
        FILE_DENIALS,
        "LPAC package-only read",
    );
}

#[test]
fn inherited_source_is_readable_but_read_only_through_the_handle_list() {
    let fixture = Fixture::new();
    fixture.single("source", "", true).expect(
        &["WriteFile(read-only inherited source)"],
        FILE_DENIALS,
        "read source sentinel then try writing through the inherited handle",
    );
    assert_eq!(fs::read(&fixture.source).unwrap(), SOURCE);
}

#[test]
fn a_handle_outside_the_list_is_not_inherited() {
    let fixture = Fixture::new();
    let (_inheritable, target) = profile_controls::inherited_canary(&fixture);
    let control = profile_controls::ordinary_inheritance(&fixture, &target);
    let confined = fixture.try_single("inherited-canary", &target, true);
    eprintln!("ordinary inheritance control: {control:?}; LPAC unlisted handle: {confined:?}");
    assert_eq!(
        control.expect("ordinary inheriting control"),
        0,
        "control reads exact canary"
    );
    confined.expect("LPAC handle probe").expect(
        &[
            "GetFileInformationByHandle(unlisted canary)",
            "unlisted handle names a different object",
        ],
        &[
            ERROR_INVALID_HANDLE,
            ERROR_INVALID_FUNCTION,
            ERROR_ACCESS_DENIED,
        ],
        "unlisted handle refusal",
    );
}

#[test]
fn helpers_with_the_same_sid_cannot_open_each_others_process_or_main_thread() {
    let fixture = Fixture::new();
    let mut sibling = fixture.start("hold", "", true);
    let report = sibling.reply();
    report.expect(&["ReadFile(hold)"], &[0], "sibling ready in its own Job");
    let identity = report.detail.expect("sibling PID and main thread ID");
    let control = fixture.try_single("sibling", &identity, false);
    let confined = fixture.try_single("sibling", &identity, true);
    eprintln!("same-SID sibling {identity}; unconfined control: {control:?}; LPAC: {confined:?}");
    sibling.release();
    assert_eq!(sibling.finish(), 0, "sibling exit");
    assert_eq!(
        sibling_codes(control.expect("unconfined sibling probe")),
        [0; 4]
    );
    assert_eq!(
        sibling_codes(confined.expect("LPAC sibling probe")),
        [ERROR_ACCESS_DENIED; 4]
    );
}

fn sibling_codes(report: Report) -> Vec<u32> {
    report.expect(&["sibling access"], &[0], "read all sibling access results");
    report
        .detail
        .expect("per-right sibling result")
        .split_ascii_whitespace()
        .map(|field| {
            field
                .split_once('=')
                .expect("sibling operation and code")
                .1
                .parse()
                .expect("Win32 code")
        })
        .collect()
}

#[test]
fn image_directory_refuses_create_growth_and_mutation() {
    let fixture = Fixture::new();
    fixture.refuse_writes(fixture.image_dir());
}

#[test]
fn user_temp_directory_refuses_create_growth_and_mutation() {
    let fixture = Fixture::new();
    fixture.refuse_writes(&std::env::temp_dir());
}

#[test]
fn user_profile_directory_refuses_create_growth_and_mutation() {
    let fixture = Fixture::new();
    let profile = env_path("USERPROFILE");
    fixture.refuse_writes(&profile);
}

#[test]
fn existing_appcontainer_storage_refuses_create_growth_and_mutation() {
    let fixture = Fixture::new();
    let root = fixture.profile_root();
    let retained = Canary::new(&root);
    fs::write(&retained.0, b"retained").expect("WriteFile (retained owner-rights canary)");
    fixture
        .profile
        .api
        .prepare()
        .expect("protect retained owner-rights canary");
    let mut child = fixture.start("hold", "", true);
    child.reply().expect(
        &["ReadFile(hold)"],
        &[0],
        "start LPAC before storage discovery",
    );
    let mut checked = BTreeSet::new();
    check_storage_tree(&fixture, &root, &mut checked);
    assert!(
        checked.contains(&root),
        "installed profile root was checked"
    );
    for name in RETAINED_AC_FOLDERS {
        assert!(
            checked.contains(&root.join("AC").join(name)),
            "retained {name} was checked"
        );
    }
    let key = profile_controls::classes_key(&fixture.profile.api);
    fixture.refuse_registry_write(&key);
    eprintln!(
        "transient LPAC object namespace: {:?}",
        fixture.try_single("profile-objects", unique(), true)
    );
    child.release();
    assert_eq!(child.finish(), 0, "storage discovery child exit");
    assert_owner_access_refused(&fixture, &root, &retained.0, &key);
}

fn assert_owner_access_refused(fixture: &Fixture, root: &Path, retained: &Path, key: &str) {
    use windows_sys::Win32::Storage::FileSystem::{READ_CONTROL, WRITE_OWNER};

    let user = profile_controls::current_user_sid();
    let package = fixture.profile.api.sid();
    let profile = &fixture.profile.api;
    for path in [root, retained] {
        profile_controls::file_owner(path, &user, true);
    }
    {
        let registry = profile_controls::Key::open(profile, READ_CONTROL | WRITE_OWNER)
            .unwrap_or_else(|code| panic!("RegOpenKeyExW (owner setup) Win32 {code}"));
        registry.owner(&user, true);
    }
    profile
        .prepare()
        .expect("prepare after setting the current user owner");
    profile.verify().expect("verify current-user-owned profile");
    for path in [root, retained] {
        profile_controls::file_owner(path, &user, false);
    }
    profile_controls::Key::open(profile, READ_CONTROL)
        .unwrap_or_else(|code| panic!("RegOpenKeyExW (owner readback) Win32 {code}"))
        .owner(&user, false);
    eprintln!("Owner readback after prepare: file, directory and key all owned by {user}");

    let scratch = fixture.root.0.join("owner control");
    fs::write(&scratch, b"owner control").expect("WriteFile (owner control)");
    profile_controls::file_owner(&scratch, &user, true);
    profile_controls::set_owner_control_dacl(&scratch);
    let result = fixture.try_single(
        "owner-access",
        format!(
            "file-control\tWRITE_DAC\t{user}\t{package}\t{}",
            scratch.display()
        ),
        false,
    );
    eprintln!("owner descriptor control: {result:?}");
    let mut outcomes = Vec::new();
    for (kind, path) in [
        ("file", root.display().to_string()),
        ("file", retained.display().to_string()),
        ("key", key.into()),
    ] {
        for right in ["WRITE_DAC", "WRITE_OWNER"] {
            let target = format!("{kind}\t{right}\t{user}\t{package}\t{path}");
            let probe = fixture.try_single("owner-access", target, true);
            eprintln!("same-user owner {kind} [{path}] {right}: {probe:?}");
            if probe.as_ref().is_ok_and(|report| report.code == 0) {
                // Undo any installed package entry so we can run the remaining probes.
                eprintln!(
                    "restore owner witness after unexpected open: {:?}",
                    profile.prepare()
                );
            }
            let api = if kind == "file" {
                "CreateFileW"
            } else {
                "RegOpenKeyExW"
            };
            outcomes.push((format!("{api}({right})"), probe));
        }
    }
    let registry_control = fixture.try_single(
        "owner-access",
        format!("key\tWRITE_DAC\t{user}\t{package}\t{key}"),
        false,
    );
    eprintln!("unconfined registry descriptor control: {registry_control:?}");
    for (control, api) in [
        (result, "CreateFileW(WRITE_DAC)"),
        (registry_control, "RegOpenKeyExW(WRITE_DAC)"),
    ] {
        let control = control.expect("unconfined owner control");
        control.expect(&[api], &[0], "unconfined descriptor write");
        assert_eq!(
            control.detail.as_deref(),
            Some("TokenUser matches owner SID=true SetSecurityInfo(DACL)=0")
        );
    }
    assert!(
        profile_controls::file_sddl(&scratch).contains(package),
        "control installed the package ACE"
    );
    assert!(
        profile_controls::registry_sddl(profile).contains(package),
        "registry control installed the package ACE"
    );
    profile
        .prepare()
        .expect("repair registry descriptor after unconfined control");
    profile
        .verify()
        .expect("verify registry after unconfined control");
    for (api, outcome) in outcomes {
        let report = outcome.expect("LPAC same-user owner probe");
        report.expect(&[&api], FILE_DENIALS, "same-user owner refusal");
        assert_eq!(
            report.detail.as_deref(),
            Some("TokenUser matches owner SID=true")
        );
    }
}

#[test]
fn preparation_and_two_launches_leave_retained_acl_state_unchanged() {
    let fixture = Fixture::new();
    let root = fixture.profile_root();
    let snapshot = || {
        let mut descriptors = vec![
            profile_controls::file_sddl(&root),
            profile_controls::file_sddl(&root.join("AC")),
        ];
        descriptors.extend(
            RETAINED_AC_FOLDERS
                .map(|name| profile_controls::file_sddl(&root.join("AC").join(name))),
        );
        descriptors.push(profile_controls::registry_sddl(&fixture.profile.api));
        descriptors
    };
    let before = snapshot();
    assert_eq!(
        fixture.profile.api.prepare().expect("repeat preparation"),
        Preparation::Unchanged
    );
    assert_eq!(before, snapshot());
    for _ in 0..2 {
        fixture
            .startup(true)
            .expect("LPAC startup with retained AC storage");
        fixture
            .profile
            .api
            .verify()
            .expect("verify retained storage after launch");
        assert_eq!(before, snapshot());
    }
}

#[test]
fn a_package_grant_on_retained_ac_blocks_verification_and_launch() {
    let fixture = Fixture::new();
    let verified = fixture
        .profile
        .api
        .verify()
        .expect("safe profile before tampering");
    let ac = fixture.profile_root().join("AC");
    grant_image_access(&ac, &fixture.sid);
    assert_refused_then_repaired(&fixture, &verified);
}

#[test]
fn a_package_grant_on_registry_storage_blocks_verification_and_launch() {
    let fixture = Fixture::new();
    let verified = fixture
        .profile
        .api
        .verify()
        .expect("safe profile before tampering");
    {
        let key = profile_controls::Key::open(
            &fixture.profile.api,
            windows_sys::Win32::Storage::FileSystem::WRITE_DAC,
        )
        .unwrap_or_else(|code| panic!("RegOpenKeyExW (restore package ACE) Win32 {code}"));
        key.grant(fixture.profile.api.sid());
    }
    assert_refused_then_repaired(&fixture, &verified);
}

#[test]
fn a_junction_at_the_profile_root_leaves_the_target_untouched() {
    let fixture = Fixture::new();
    assert_junction_refused(&fixture, &fixture.profile_root());
}

#[test]
fn a_junction_at_ac_leaves_the_target_untouched() {
    let fixture = Fixture::new();
    let root = fixture.profile_root();
    assert_junction_refused(&fixture, &root.join("AC"));
}

#[test]
fn junctions_at_known_ac_subfolders_leave_the_target_untouched() {
    let fixture = Fixture::new();
    let ac = fixture.profile_root().join("AC");
    for name in RETAINED_AC_FOLDERS {
        assert_junction_refused(&fixture, &ac.join(name));
    }
}

fn assert_junction_refused(fixture: &Fixture, path: &Path) {
    let target = fixture.root.0.join(unique());
    let cache = target.join("INetCache");
    fs::create_dir_all(&cache).expect("CreateDirectoryW (junction target and empty INetCache)");
    let before = (
        profile_controls::file_sddl(&target),
        profile_controls::file_sddl(&cache),
    );
    let saved = path.with_file_name(unique());
    fs::rename(path, &saved).expect("MoveFileExW (preserve junction witness directory)");
    let original = MovedDirectory { path, saved };
    let junction = profile_controls::junction(path, &target);
    assert!(path.join("INetCache").is_dir(), "junction positive control");
    let rejected = fixture
        .profile
        .api
        .prepare()
        .expect_err("preparation followed junction");
    eprintln!("prepare [{}]: {rejected}", path.display());
    assert_eq!(rejected.step, Step::ReadStorage);
    assert!(rejected.to_string().contains("reparse point"), "{rejected}");
    assert!(
        cache.is_dir(),
        "preparation removed an entry outside its profile"
    );
    assert_eq!(
        before,
        (
            profile_controls::file_sddl(&target),
            profile_controls::file_sddl(&cache)
        )
    );
    drop(junction);
    drop(original);
    assert!(
        path.is_dir(),
        "original junction witness directory was restored"
    );
}

struct MovedDirectory<'a> {
    path: &'a Path,
    saved: PathBuf,
}

impl Drop for MovedDirectory<'_> {
    fn drop(&mut self) {
        if let Err(error) = fs::rename(&self.saved, self.path) {
            eprintln!("MoveFileExW (restore junction witness directory): {error}");
        }
    }
}

#[test]
fn a_package_grant_on_retained_profile_files_blocks_launch_until_repaired() {
    let fixture = Fixture::new();
    let verified = fixture
        .profile
        .api
        .verify()
        .expect("safe profile before tampering");
    let retained = Canary::new(&fixture.profile_root());
    fs::write(&retained.0, b"baseline").expect("WriteFile (retained file)");
    grant_file_access(&retained.0, &fixture.sid, FILE_GENERIC_WRITE, 0);
    assert_refused_then_repaired(&fixture, &verified);
    fixture.single("grow", &retained.0, false).expect(
        &["WriteFile(canary)"],
        &[0],
        "unconfined retained file write",
    );
    fs::write(&retained.0, b"baseline").expect("restore retained file sentinel");
    fixture.file_refusal("grow", &retained.0);
    assert_eq!(
        fs::read(&retained.0).expect("read retained file"),
        b"baseline"
    );
}

#[test]
fn a_package_grant_on_registry_subkeys_blocks_launch_until_repaired() {
    let fixture = Fixture::new();
    let verified = fixture
        .profile
        .api
        .verify()
        .expect("safe profile before tampering");
    {
        let key = profile_controls::Key::open(&fixture.profile.api, KEY_CREATE_SUB_KEY)
            .unwrap_or_else(|code| panic!("RegOpenKeyExW (subkey parent) Win32 {code}"));
        key.create_subkey("retain-child")
            .grant(fixture.profile.api.sid());
    }
    assert_refused_then_repaired(&fixture, &verified);
    let key = format!(
        r"{}\retain-child",
        profile_controls::classes_key(&fixture.profile.api)
    );
    fixture.refuse_registry_write(&key);
}

fn assert_refused_then_repaired(fixture: &Fixture, stale: &VerifiedProfile<'_>) {
    fixture.startup(false).expect("unconfined image control");
    assert_refusal_then_prepare(fixture, stale, fixture.profile.api.sid());
    fixture.startup(true).expect("LPAC startup after repair");
}

fn assert_refusal_then_prepare(fixture: &Fixture, stale: &VerifiedProfile<'_>, detail: &str) {
    let profile = &fixture.profile.api;
    let observed = profile
        .verify()
        .expect_err("verification accepted unsafe profile storage");
    let rejected = launch_refusal(fixture, stale);
    eprintln!("verification refused: {observed}; launch refused: {rejected}");
    for error in [observed, rejected] {
        assert_eq!(error.step, Step::StorageAcl, "{error}");
        assert!(error.to_string().contains(detail), "{error}");
    }
    assert_eq!(
        profile.prepare().expect("repair unsafe profile storage"),
        Preparation::Prepared
    );
    profile.verify().expect("verify repaired profile");
}

fn launch_refusal(fixture: &Fixture, stale: &VerifiedProfile<'_>) -> media_lpac::Error {
    let (request, _send) = io::pipe().expect("CreatePipe (request)");
    let (_read, reply) = io::pipe().expect("CreatePipe (reply)");
    let source = File::open(&fixture.source).expect("CreateFileW (source)");
    match fixture.launch(
        stale,
        &[
            OsString::from("ready"),
            fixture.image_dir().as_os_str().to_os_string(),
        ],
        Inputs {
            source: &source,
            request: &request,
            reply: &reply,
        },
    ) {
        Ok(_) => panic!("production launch accepted unsafe profile"),
        Err(error) => error,
    }
}

#[test]
fn preparation_keeps_unexpected_ac_content_and_refuses_mutation() {
    let fixture = Fixture::new();
    let ac = fixture.profile_root().join("AC");
    let unexpected = ac.join("keep this directory");
    fs::create_dir_all(&unexpected).expect("CreateDirectoryW (unexpected content)");
    let file = unexpected.join("data");
    fs::write(&file, b"baseline").expect("WriteFile (unexpected content)");
    grant_image_access(&unexpected, &fixture.sid);
    assert_eq!(
        fixture
            .profile
            .api
            .prepare()
            .expect("protect retained AC content"),
        Preparation::Prepared
    );
    assert_eq!(
        fs::read(&file).expect("ReadFile (preserved content)"),
        b"baseline"
    );
    fixture.refuse_writes(&unexpected);
    for case in ["grow", "mutate"] {
        fixture.single(case, &file, false).expect(
            &["WriteFile(canary)"],
            &[0],
            "unconfined existing file write",
        );
        fs::write(&file, b"baseline").expect("reset preserved file canary");
        fixture.file_refusal(case, &file);
        assert_eq!(
            fs::read(&file).expect("ReadFile (preserved content)"),
            b"baseline"
        );
    }
}

#[test]
fn preparation_preserves_a_directory_stream_and_the_child_cannot_write_it() {
    let fixture = Fixture::new();
    let directory = fixture.profile_root().join("AC").join("Temp");
    let mut stream = directory.as_os_str().to_os_string();
    stream.push(":keep");
    let stream = PathBuf::from(stream);
    let sentinel = b"retained stream data";
    fs::write(&stream, sentinel).expect("WriteFile (test directory stream)");
    grant_file_access(&directory, &fixture.sid, FILE_GENERIC_WRITE, 0);
    assert_eq!(
        fixture
            .profile
            .api
            .prepare()
            .expect("protect directory stream"),
        Preparation::Prepared
    );
    fixture
        .profile
        .api
        .verify()
        .expect("verify directory with stream");
    assert_eq!(
        fs::read(&stream).expect("ReadFile (preserved directory stream)"),
        sentinel
    );
    for (case, expected) in [
        ("mutate", b"canaryed stream data".as_slice()),
        ("replace", b"canary".as_slice()),
    ] {
        let control = fixture.single(case, &stream, false);
        eprintln!("{case} unconfined directory stream control: {control:?}");
        control.expect(
            &["WriteFile(canary)"],
            &[0],
            "unconfined directory stream control",
        );
        assert_eq!(fs::read(&stream).unwrap(), expected);
        fs::write(&stream, sentinel).expect("reset directory stream");
        fixture.file_refusal(case, &stream);
        assert_eq!(fs::read(&stream).unwrap(), sentinel);
    }
}

#[test]
fn the_combined_storage_cap_blocks_verification_and_launch() {
    let fixture = Fixture::new();
    let stale = fixture
        .profile
        .api
        .verify()
        .expect("safe profile before seeding");
    let root = fixture.profile_root();
    for index in 0..127 {
        File::create_new(root.join(index.to_string())).expect("CreateFileW (combined cap file)");
    }
    {
        let key = profile_controls::Key::open(&fixture.profile.api, KEY_CREATE_SUB_KEY)
            .unwrap_or_else(|code| panic!("RegOpenKeyExW (combined cap) Win32 {code}"));
        for index in 0..128 {
            key.create_subkey(&index.to_string());
        }
    }
    fixture.startup(false).expect("unconfined image control");
    let observed = fixture
        .profile
        .api
        .verify()
        .expect_err("combined storage cap was not checked");
    let rejected = launch_refusal(&fixture, &stale);
    eprintln!("verification refused: {observed}; launch refused: {rejected}");
    assert_eq!(observed.step, Step::StorageLimit);
    assert_eq!(rejected.step, Step::StorageLimit);
}

#[test]
fn removal_leaves_no_profile_folder_or_registry_key() {
    let fixture = Fixture::new();
    let root = fixture.profile_root();
    assert!(root.is_dir(), "installed profile folder exists");
    drop(
        profile_controls::Key::open(&fixture.profile.api, KEY_READ)
            .unwrap_or_else(|code| panic!("RegOpenKeyExW (installed profile) Win32 {code}")),
    );
    fixture
        .profile
        .api
        .remove()
        .expect("DeleteAppContainerProfile (uninstall)");
    assert_eq!(
        fs::symlink_metadata(root)
            .expect_err("profile folder survived removal")
            .kind(),
        io::ErrorKind::NotFound
    );
    let opened = profile_controls::Key::open(&fixture.profile.api, KEY_READ);
    assert!(
        matches!(opened, Err(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)),
        "RegOpenKeyExW (removed profile) Win32 {}",
        opened.err().unwrap_or(0)
    );
    assert_eq!(
        fixture
            .profile
            .api
            .verify()
            .expect_err("removed profile was verified")
            .step,
        Step::ReadStorage
    );
}

#[test]
fn current_user_registry_key_creation_is_refused() {
    let fixture = Fixture::new();
    fixture.refused(
        "registry",
        unique(),
        &["RegCreateKeyExW(HKCU)"],
        FILE_DENIALS,
    );
}

#[test]
fn loopback_tcp_connection_is_refused() {
    let fixture = Fixture::new();
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback control listener");
    let address = listener.local_addr().unwrap().to_string();
    let control = fixture.try_single("connect", &address, false);
    let confined = fixture.try_single("connect", &address, true);
    eprintln!("connect unconfined control: {control:?}");
    eprintln!("connect LPAC confinement: {confined:?}");
    let control = control.unwrap_or_else(|error| panic!("{error}"));
    control.expect(&["connect"], &[0], "unconfined Winsock control");
    assert_eq!(control.detail.as_deref(), Some("WSAStartup=0 socket=0"));
    let confined = confined.unwrap_or_else(|error| panic!("{error}"));
    assert!(
        matches!(confined.api.as_str(), "WSAStartup" | "socket" | "connect") && confined.code != 0,
        "LPAC loopback Winsock refusal: {confined:?}"
    );
    listener.set_nonblocking(true).unwrap();
    listener
        .accept()
        .expect("accept the unconfined control connection");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock,
        "the listener received another connection"
    );
}

#[test]
fn opening_the_host_for_handle_duplication_is_refused() {
    let fixture = Fixture::new();
    fixture.refused(
        "process",
        std::process::id().to_string(),
        &["OpenProcess(PROCESS_DUP_HANDLE)"],
        FILE_DENIALS,
    );
}

#[test]
fn opening_the_host_for_limited_query_is_refused() {
    let fixture = Fixture::new();
    fixture.refused(
        "process-query",
        std::process::id().to_string(),
        &["OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)"],
        FILE_DENIALS,
    );
}

#[test]
fn opening_a_default_descriptor_named_pipe_is_refused() {
    let fixture = Fixture::new();
    let name = format!(r"\\.\pipe\{}", unique());
    let text = wide(&name);
    let _first = default_pipe_instance(&text);
    // The control can leave its instance connected until the host closes it.
    let _second = default_pipe_instance(&text);
    fixture.refused("pipe", name, &["CreateFileW(named pipe)"], FILE_DENIALS);
}

fn default_pipe_instance(name: &[u16]) -> OwnedHandle {
    // SAFETY: the name is terminated, and null security selects the default descriptor.
    let pipe = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND,
            PIPE_REJECT_REMOTE_CLIENTS,
            2,
            4096,
            4096,
            0,
            ptr::null(),
        )
    };
    assert_ne!(
        pipe,
        INVALID_HANDLE_VALUE,
        "CreateNamedPipeW (default descriptor): {}",
        io::Error::last_os_error()
    );
    // SAFETY: success transfers ownership of a new pipe instance.
    unsafe { OwnedHandle::from_raw_handle(pipe) }
}

#[test]
fn starting_a_second_process_is_refused() {
    let fixture = Fixture::new();
    fixture.refused(
        "spawn",
        &fixture.exe,
        &["CreateProcessW(second process)"],
        &[
            ERROR_CHILD_PROCESS_BLOCKED,
            ERROR_ACCESS_DENIED,
            ERROR_ACCESS_DISABLED_BY_POLICY,
        ],
    );
}

#[test]
fn a_job_memory_event_ends_a_child_that_exceeds_the_commit_cap() {
    let fixture = Fixture::new();
    let bytes = (MEMORY_BYTES * 2).to_string();
    let mut open = fixture.start("memory", &bytes, false);
    let control = open.reply();
    eprintln!("unconfined memory control: {control:?}");
    control.expect(&["VirtualAlloc"], &[0], "unconfined child commits 128 MiB");
    open.release();
    assert_eq!(open.finish(), 0, "unconfined memory control exit");
    let mut confined = fixture.start("memory", &bytes, true);
    let outcome = confined.reply();
    eprintln!("LPAC memory limit: {outcome:?}");
    outcome.expect(
        &["VirtualAlloc"],
        &[
            ERROR_NOT_ENOUGH_MEMORY,
            ERROR_COMMITMENT_LIMIT,
            ERROR_WORKING_SET_QUOTA,
            ERROR_NOT_ENOUGH_QUOTA,
        ],
        "LPAC child exceeds its 64 MiB Job commit limit",
    );
    let Process::Lpac(child) = &confined.process else {
        unreachable!()
    };
    let code = child
        .terminate_on_memory_limit(TIMEOUT)
        .unwrap_or_else(|error| {
            panic!("Job memory event / termination after {outcome:?}: {error}")
        });
    assert_eq!(
        code, EXIT_AFTER_MEMORY_LIMIT,
        "GetExitCodeProcess after Job memory event"
    );
}

#[test]
fn closing_the_last_job_handle_ends_a_running_child() {
    let fixture = Fixture::new();
    let mut running = fixture.start("hold-directory", "", true);
    let ready = running.reply();
    ready.expect(
        &["ReadFile(hold)"],
        &[0],
        "child waits with request pipe open",
    );
    let projected = PathBuf::from(ready.detail.expect("child's projected LocalAppData"));
    let directory = projected.ancestors().nth(3).expect("host launch directory");
    assert!(
        directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("kinewright-lpac-")
    );
    let refusal = fs::remove_dir(directory).expect_err("launch directory was not pinned");
    eprintln!("RemoveDirectoryW (live LPAC directory): {refusal}");
    assert!(matches!(refusal.raw_os_error(), Some(5 | 32)), "{refusal}");
    let Process::Lpac(child) = running.process else {
        unreachable!()
    };
    let result = child.wait(Duration::ZERO).unwrap_err();
    assert!(
        result.kind() == io::ErrorKind::TimedOut,
        "WaitForSingleObject before Job close expected WAIT_TIMEOUT: {result}"
    );
    let exit = child
        .close_job(TIMEOUT)
        .unwrap_or_else(|e| panic!("CloseHandle(Job) then wait: {e}"));
    eprintln!("CloseHandle(Job): child signaled with exit code {exit:#010x}");
    assert_eq!(
        directory
            .symlink_metadata()
            .expect_err("launch directory survived child teardown")
            .kind(),
        io::ErrorKind::NotFound
    );
}

struct TestProfile {
    api: Profile,
    _lease: std::sync::MutexGuard<'static, ()>,
}

impl TestProfile {
    fn new() -> Self {
        static LEASE: Mutex<()> = Mutex::new(());
        let lease = LEASE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let api = Profile::new(TEST_PROFILE).unwrap_or_else(|error| panic!("{error}"));
        api.remove()
            .unwrap_or_else(|error| panic!("remove stale profile: {error}"));
        let profile = Self { api, _lease: lease };
        profile
            .api
            .prepare()
            .unwrap_or_else(|error| panic!("prepare profile: {error}"));
        profile
    }
}

impl Drop for TestProfile {
    fn drop(&mut self) {
        if let Err(error) = self.api.remove() {
            eprintln!("remove test profile: {error}");
            assert!(thread::panicking(), "remove test profile: {error}");
        }
    }
}

struct Fixture {
    profile: TestProfile,
    exe: PathBuf,
    source: PathBuf,
    sid: Sid,
    root: Temp,
}

impl Fixture {
    fn new() -> Self {
        let profile = TestProfile::new();
        let root = Temp::new();
        let directory = root.0.join("image directory");
        fs::create_dir(&directory).expect("CreateDirectoryW (private fixture image directory)");
        let sid = Sid::derive();
        assert_eq!(sid.text(), OsStr::new(profile.api.sid()));
        grant_image_access(&directory, &sid);
        let exe = directory.join("fixture.exe");
        let input = env!("CARGO_BIN_EXE_kinewright-lpac-spike-child");
        copy_image(Path::new(input), &exe);
        let source = root.0.join("source");
        fs::write(&source, SOURCE).expect("CreateFileW/WriteFile (fixture source)");
        Self {
            profile,
            exe,
            source,
            sid,
            root,
        }
    }

    fn profile_root(&self) -> PathBuf {
        let sid = OsStr::new(self.profile.api.sid());
        container_storage(sid)
            .expect("GetAppContainerFolderPath (registered profile)")
            .parent()
            .expect("registered profile root")
            .to_path_buf()
    }

    fn launch(
        &self,
        verified: &VerifiedProfile<'_>,
        args: &[OsString],
        inputs: Inputs<'_>,
    ) -> Result<media_lpac::Child, media_lpac::Error> {
        media_lpac::launch(verified, &self.exe, args, inputs, fixture_limits())
    }

    fn image_dir(&self) -> &Path {
        self.exe.parent().unwrap()
    }

    fn start(&self, case: &str, target: impl AsRef<OsStr>, lpac: bool) -> Running {
        self.try_start(case, target, lpac)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn try_start(
        &self,
        case: &str,
        target: impl AsRef<OsStr>,
        lpac: bool,
    ) -> Result<Running, String> {
        let (child_request, mut request) =
            io::pipe().map_err(|error| format!("{case} CreatePipe (fixture request): {error}"))?;
        let (reply, child_reply) =
            io::pipe().map_err(|error| format!("{case} CreatePipe (fixture reply): {error}"))?;
        let process = {
            // The lock serializes temporary handle inheritance and guards no data.
            let _spawning = SPAWNING
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if lpac {
                let source = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&self.source)
                    .map_err(|error| format!("{case} CreateFileW (read/write source): {error}"))?;
                let args = [OsString::from(case), target.as_ref().to_os_string()];
                let verified = self
                    .profile
                    .api
                    .verify()
                    .map_err(|error| format!("{case} LPAC verify: {error}"))?;
                self.launch(
                    &verified,
                    &args,
                    Inputs {
                        source: &source,
                        request: &child_request,
                        reply: &child_reply,
                    },
                )
                .map(Process::Lpac)
                .map_err(|error| error.to_string())
            } else {
                Command::new(&self.exe)
                    .args(["0", "0", "0", case])
                    .arg(target.as_ref())
                    .env_clear()
                    .env("SystemRoot", env_path("SystemRoot"))
                    .current_dir(self.image_dir())
                    .creation_flags(DETACHED_PROCESS)
                    .stdin(Stdio::from(child_request))
                    .stdout(Stdio::from(child_reply))
                    .stderr(Stdio::null())
                    .spawn()
                    .map(|child| Process::Open(Control(child)))
                    .map_err(|error| error.to_string())
            }
        };
        let step = if lpac {
            "LPAC launch/inspection"
        } else {
            "unconfined CreateProcessW"
        };
        let process = process.map_err(|error| format!("{case} {step}: {error}"))?;
        request
            .write_all(b"!")
            .map_err(|error| format!("{case} {step} succeeded, WriteFile(request): {error}"))?;
        Ok(Running {
            process,
            request,
            reply,
            context: format!("{case} lpac={lpac}"),
        })
    }

    fn single(&self, case: &str, target: impl AsRef<OsStr>, lpac: bool) -> Report {
        self.try_single(case, target, lpac)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn try_single(
        &self,
        case: &str,
        target: impl AsRef<OsStr>,
        lpac: bool,
    ) -> Result<Report, String> {
        let mut child = self.try_start(case, target, lpac)?;
        let report = child.try_reply()?;
        let exit = child.try_finish()?;
        if exit != 0 {
            return Err(format!(
                "GetExitCodeProcess ({case}, lpac={lpac}) exit {exit:#010x}, reply {report:?}"
            ));
        }
        Ok(report)
    }

    fn startup(&self, lpac: bool) -> Result<(), String> {
        let mode = if lpac { "LPAC" } else { "unconfined" };
        if !lpac {
            eprintln!(
                "unconfined startup CreateProcessW input image=[{}] cwd=[{}]",
                self.exe.display(),
                self.image_dir().display()
            );
        }
        let case = "ready";
        let result = self
            .try_single(case, self.image_dir(), lpac)
            .and_then(|report| {
                if report.api == "ready" && report.code == 0 {
                    Ok(())
                } else {
                    Err(format!(
                        "{} returned Win32 {} ({:#010x})",
                        report.api, report.code, report.code
                    ))
                }
            });
        match &result {
            Ok(()) => eprintln!(
                "{mode} startup: CreateProcessW and detached startup passed (Win32 0, exit 0)"
            ),
            Err(error) => eprintln!("{mode} startup: {error}"),
        }
        result
    }

    fn refused(&self, case: &str, target: impl AsRef<OsStr>, apis: &[&str], codes: &[u32]) {
        let control = self.try_single(case, target.as_ref(), false);
        let confined = self.try_single(case, target, true);
        eprintln!("{case} unconfined control: {control:?}");
        eprintln!("{case} LPAC confinement: {confined:?}");
        control
            .unwrap_or_else(|error| panic!("{error}"))
            .expect(apis, &[0], "unconfined control");
        confined
            .unwrap_or_else(|error| panic!("{error}"))
            .expect(apis, codes, "LPAC refusal");
    }

    fn refuse_registry_write(&self, key: &str) {
        let target = format!("{key}\t{}", unique());
        let control = self.try_single("profile-registry-direct", &target, false);
        let confined = self.try_single("profile-registry-direct", &target, true);
        eprintln!("direct registry [{key}] control: {control:?}; LPAC: {confined:?}");
        control.expect("unconfined registry control").expect(
            &["RegSetValueExW(profile direct)"],
            &[0],
            "direct unconfined registry control",
        );
        confined.expect("LPAC registry refusal").expect(
            &[
                "RegOpenKeyExW(KEY_SET_VALUE)",
                "RegSetValueExW(profile direct)",
            ],
            FILE_DENIALS,
            "direct LPAC registry refusal",
        );
    }

    fn refuse_writes(&self, parent: &Path) {
        let stale = parent.starts_with(self.profile_root()).then(|| {
            self.profile
                .api
                .verify()
                .expect("verify profile before creating retained canary")
        });
        let canary = Canary::new(parent);
        let control = self.single("create", &canary.0, false);
        eprintln!(
            "create unconfined control [{}]: {control:?}",
            canary.0.display()
        );
        control.expect(&["WriteFile(canary)"], &[0], "unconfined create");
        assert_eq!(fs::read(&canary.0).unwrap(), b"canary");
        if let Some(stale) = &stale {
            assert_refusal_then_prepare(self, stale, "unprotected DACL");
        }
        let reset = || {
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&canary.0)
                .and_then(|mut file| file.write_all(b"baseline"))
                .expect("reset the same write canary");
        };
        for (case, expected) in [
            ("grow", b"baselinecanary".as_slice()),
            ("mutate", b"canaryne".as_slice()),
        ] {
            reset();
            let control = self.single(case, &canary.0, false);
            eprintln!(
                "{case} unconfined control [{}]: {control:?}",
                canary.0.display()
            );
            control.expect(&["WriteFile(canary)"], &[0], "unconfined write");
            assert_eq!(fs::read(&canary.0).unwrap(), expected);
            reset();
            self.file_refusal(case, &canary.0);
            assert_eq!(fs::read(&canary.0).unwrap(), b"baseline");
        }
        fs::remove_file(&canary.0).expect("remove unconfined create canary");
        self.file_refusal("create", &canary.0);
        assert!(!canary.0.exists(), "LPAC created {}", canary.0.display());
    }

    fn file_refusal(&self, case: &str, target: &Path) {
        let report = self
            .storage_probe(case, target)
            .unwrap_or_else(|error| panic!("{error}"));
        eprintln!("{case} LPAC refusal [{}]: {report:?}", target.display());
        report.expect(
            &["CreateFileW(canary)", "WriteFile(canary)"],
            FILE_DENIALS,
            &format!("{case} LPAC file refusal at {}", target.display()),
        );
    }
    fn storage_probe(&self, case: &str, target: &Path) -> Result<Report, String> {
        self.try_single(case, target, true)
    }
}

fn fixture_limits() -> Limits {
    // SAFETY: querying all processor groups needs no pointer or handle.
    let cpus = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    assert_ne!(
        cpus,
        0,
        "GetActiveProcessorCount: {}",
        io::Error::last_os_error()
    );
    Limits {
        private_bytes: MEMORY_BYTES,
        cpu_rate: 10_000u32.div_ceil(cpus),
    }
}

fn copy_image(input: &Path, image: &Path) {
    let mut source = File::open(input).unwrap_or_else(|error| {
        panic!(
            "CreateFileW (archived fixture {}): {error}",
            input.display()
        )
    });
    let mut output = File::create_new(image).unwrap_or_else(|error| {
        panic!(
            "CreateFileW (private fixture image {}): {error}",
            image.display()
        )
    });
    let bytes = source
        .metadata()
        .expect("GetFileInformationByHandle (archived image)")
        .len();
    let copied = io::copy(&mut source, &mut output).expect("ReadFile/WriteFile (copy fixture)");
    assert!(
        bytes > 0 && bytes == copied,
        "ReadFile/WriteFile (copy fixture) Win32 0: source={bytes} copied={copied}"
    );
}

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let reported = std::env::temp_dir();
        let resolved = reported.canonicalize().unwrap_or_else(|error| {
            panic!(
                "GetFinalPathNameByHandleW (resolve fixture temp): {error}; temp=[{}]",
                reported.display()
            )
        });
        // These fixtures use short, drive-qualified DOS names for both CreateProcessW inputs.
        let local = match resolved.components().next() {
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_)) => {
                let units: Vec<u16> = resolved.as_os_str().encode_wide().skip(4).collect();
                PathBuf::from(OsString::from_wide(&units))
            }
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => resolved,
            _ => panic!(
                "GetFinalPathNameByHandleW (fixture temp) Win32 0: expected local drive, got [{}]",
                resolved.display()
            ),
        };
        let path = local.join(unique());
        let units = path
            .join("image directory")
            .join("fixture.exe")
            .as_os_str()
            .encode_wide()
            .count();
        assert!(
            units < MAX_PATH as usize,
            "GetFinalPathNameByHandleW (fixture path preflight) Win32 0: expected short DOS path, got {units} UTF-16 units"
        );
        eprintln!(
            "GetFinalPathNameByHandleW (fixture temp) Win32 0: reported=[{}] local=[{}]",
            reported.display(),
            local.display()
        );
        fs::create_dir(&path)
            .unwrap_or_else(|e| panic!("CreateDirectoryW (private fixture temp): {e}"));
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let start = Instant::now();
        loop {
            match fs::remove_dir_all(&self.0) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                // Job closure may precede release of the child's image section.
                Err(_) if start.elapsed() < Duration::from_secs(2) => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    eprintln!("RemoveDirectoryW (fixture {}): {e}", self.0.display());
                    assert!(thread::panicking(), "could not clean fixture directory");
                    break;
                }
            }
        }
    }
}

struct Canary(PathBuf);
impl Canary {
    fn new(parent: &Path) -> Self {
        Self(parent.join(unique()))
    }
}
impl Drop for Canary {
    fn drop(&mut self) {
        match fs::remove_file(&self.0) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("DeleteFileW (canary {}): {e}", self.0.display()),
        }
    }
}

fn unique() -> String {
    format!(
        "Kinewright-LpacSpike-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}

fn env_path(key: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(key).unwrap_or_else(|| panic!("{key} is missing")));
    assert!(path.is_absolute(), "{key} is not an absolute directory");
    path
}

enum Process {
    Lpac(media_lpac::Child),
    Open(Control),
}

struct Control(std::process::Child);
impl Drop for Control {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct Running {
    process: Process,
    request: PipeWriter,
    reply: PipeReader,
    context: String,
}

impl Running {
    fn reply(&mut self) -> Report {
        self.try_reply().unwrap_or_else(|error| panic!("{error}"))
    }

    fn try_reply(&mut self) -> Result<Report, String> {
        let start = Instant::now();
        let mut bytes = Vec::new();
        while start.elapsed() < TIMEOUT {
            let mut available = 0;
            // SAFETY: reply is the live pipe reader and available is writable.
            let ok = unsafe {
                PeekNamedPipe(
                    self.reply.as_raw_handle(),
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    &raw mut available,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                return Err(match &mut self.process {
                    Process::Lpac(child) => format!(
                        "{} PeekNamedPipe(reply): {error}; child wait/exit {:?}",
                        self.context,
                        child
                            .wait(Duration::from_millis(100))
                            .map(|code| format!("{code:#010x}"))
                    ),
                    Process::Open(child) => format!(
                        "{} PeekNamedPipe(reply): {error}; unconfined exit {:?}",
                        self.context,
                        child.0.try_wait()
                    ),
                });
            }
            if available == 0 {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            let length = bytes.len();
            let available = available as usize;
            if length + available > 16384 {
                return Err(format!(
                    "{} PeekNamedPipe (reply) Win32 0: unexpected reply length",
                    self.context
                ));
            }
            bytes.resize(length + available, 0);
            self.reply
                .read_exact(&mut bytes[length..])
                .map_err(|error| format!("{} ReadFile (fixture reply): {error}", self.context))?;
            if bytes.ends_with(b"\n") {
                return Report::parse(&self.context, &bytes);
            }
        }
        Err(format!(
            "{} PeekNamedPipe(reply): expected a report within {TIMEOUT:?} (timeout Win32 {WAIT_TIMEOUT})",
            self.context
        ))
    }

    fn release(&mut self) {
        self.request
            .write_all(b"!")
            .expect("WriteFile (release fixture)");
    }

    fn finish(self) -> u32 {
        self.try_finish().unwrap_or_else(|error| panic!("{error}"))
    }

    fn try_finish(mut self) -> Result<u32, String> {
        match &mut self.process {
            Process::Lpac(child) => child
                .wait(TIMEOUT)
                .map_err(|error| format!("{}: {error}", self.context)),
            Process::Open(child) => {
                let start = Instant::now();
                while start.elapsed() < TIMEOUT {
                    if let Some(status) = child.0.try_wait().map_err(|error| {
                        format!(
                            "{} WaitForSingleObject (unconfined child): {error}",
                            self.context
                        )
                    })? {
                        return status.code().map(i32::cast_unsigned).ok_or_else(|| {
                            format!(
                                "{} GetExitCodeProcess (unconfined child): exit code unavailable",
                                self.context
                            )
                        });
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(format!(
                    "{} WaitForSingleObject (unconfined child): Win32 {WAIT_TIMEOUT}",
                    self.context
                ))
            }
        }
    }
}

#[derive(Debug)]
struct Report {
    api: String,
    code: u32,
    detail: Option<String>,
}

impl Report {
    fn parse(context: &str, bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("{context} ReadFile (fixture report) Win32 0: {error}"))?;
        let mut fields = text.trim_end().splitn(3, '\t');
        let api = fields.next().unwrap_or_default();
        let code = fields.next().ok_or_else(|| {
            format!("{context} ReadFile (fixture report) Win32 0: malformed report {text:?}")
        })?;
        Ok(Self {
            api: api.into(),
            detail: fields.next().map(str::to_owned),
            code: code.parse().map_err(|error| {
                format!(
                    "{context} ReadFile (fixture reply) Win32 0: invalid reported code: {error}"
                )
            })?,
        })
    }

    fn expect(&self, apis: &[&str], codes: &[u32], step: &str) {
        assert!(
            apis.contains(&self.api.as_str()) && codes.contains(&self.code),
            "{step}: {} returned Win32 {} ({:#010x}), expected API {apis:?} and code {codes:?}; detail={:?}",
            self.api,
            self.code,
            self.code,
            self.detail,
        );
    }
}

struct Sid(PSID);

impl Sid {
    fn derive() -> Self {
        let name = wide(TEST_PROFILE);
        let mut sid = ptr::null_mut();
        // SAFETY: the name is terminated and the SID out pointer is writable.
        let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &raw mut sid) };
        assert!(
            hr >= 0 && !sid.is_null(),
            "DeriveAppContainerSidFromAppContainerName (fixture ACL): HRESULT {:#010x}",
            hr.cast_unsigned()
        );
        Self(sid)
    }

    fn text(&self) -> OsString {
        let mut buffer = ptr::null_mut();
        // SAFETY: the derived SID is valid and the pointer is writable.
        let ok = unsafe { ConvertSidToStringSidW(self.0, &raw mut buffer) };
        assert_ne!(
            ok,
            0,
            "ConvertSidToStringSidW: {}",
            io::Error::last_os_error()
        );
        // SAFETY: success returns a NUL-terminated LocalAlloc string.
        let text = unsafe { copy_wide(buffer) };
        // SAFETY: success transfers this allocation.
        unsafe { LocalFree(buffer.cast()) };
        text
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: the API allocated this SID and this owner frees it once.
        unsafe { FreeSid(self.0) };
    }
}

struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: the security API returned this LocalAlloc pointer, and null is accepted.
        unsafe { LocalFree(self.0) };
    }
}

fn grant_image_access(directory: &Path, sid: &Sid) {
    grant_file_access(
        directory,
        sid,
        FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    );
}

fn grant_file_access(target: &Path, sid: &Sid, rights: u32, inheritance: u32) {
    let path = wide(target);
    let mut old_acl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: path identifies only the just-created directory. The outputs are writable.
    let code = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut old_acl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    let _descriptor = Local(descriptor);
    assert_eq!(
        code, 0,
        "GetNamedSecurityInfoW (image directory DACL): Win32 {code}"
    );
    assert!(
        !old_acl.is_null(),
        "GetNamedSecurityInfoW returned a null DACL"
    );
    let mut ace = EXPLICIT_ACCESS_W {
        grfAccessPermissions: rights,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: inheritance,
        ..Default::default()
    };
    ace.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    ace.Trustee.TrusteeType = TRUSTEE_IS_USER;
    ace.Trustee.ptstrName = sid.0.cast();
    let mut new_acl = ptr::null_mut();
    // SAFETY: the original ACL and SID remain live and new_acl is writable.
    let code = unsafe { SetEntriesInAclW(1, &raw const ace, old_acl, &raw mut new_acl) };
    let _merged = Local(new_acl.cast());
    assert_eq!(
        code, 0,
        "SetEntriesInAclW (derived SID read/execute): Win32 {code}"
    );
    // SAFETY: this writes only the merged DACL on the isolated image directory.
    let code = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            new_acl,
            ptr::null_mut(),
        )
    };
    assert_eq!(
        code, 0,
        "SetNamedSecurityInfoW (fixture image directory only): Win32 {code}"
    );
}

fn container_storage(sid: &OsStr) -> Option<PathBuf> {
    match query_container_storage(sid) {
        Ok(Some(path)) => {
            assert!(
                path.is_absolute(),
                "GetAppContainerFolderPath HRESULT 0: expected an absolute path"
            );
            Some(path)
        }
        Ok(None) => panic!("GetAppContainerFolderPath HRESULT 0 returned a null path"),
        Err(status) => {
            let hr = status.cast_unsigned();
            let code = hr & 0xffff;
            assert!(
                hr >> 16 == 0x8007
                    && matches!(
                        code,
                        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_NOT_FOUND
                    ),
                "GetAppContainerFolderPath (unregistered LPAC SID): HRESULT {hr:#010x}, Win32 {code}"
            );
            eprintln!(
                "GetAppContainerFolderPath (unregistered LPAC SID): HRESULT {hr:#010x}, Win32 {code}"
            );
            None
        }
    }
}

fn query_container_storage(sid: &OsStr) -> Result<Option<PathBuf>, i32> {
    let sid = wide(sid);
    let mut buffer = ptr::null_mut();
    // SAFETY: sid is terminated and buffer receives the query's allocated path.
    let status = unsafe { GetAppContainerFolderPath(sid.as_ptr(), &raw mut buffer) };
    if status < 0 {
        return Err(status);
    }
    if buffer.is_null() {
        return Ok(None);
    }
    // SAFETY: success returns a terminated string allocated by the COM task allocator.
    let path = PathBuf::from(unsafe { copy_wide(buffer) });
    // SAFETY: the caller owns this allocation.
    unsafe { CoTaskMemFree(buffer.cast()) };
    Ok(Some(path))
}

fn check_storage_tree(fixture: &Fixture, path: &Path, checked: &mut BTreeSet<PathBuf>) {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => panic!(
            "GetFileInformationByHandle (storage {}): {error}",
            path.display()
        ),
    };
    assert_eq!(
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
        0,
        "unexpected reparse point in LPAC storage at {}",
        path.display()
    );
    assert!(
        metadata.is_dir(),
        "LPAC storage is not a directory at {}",
        path.display()
    );
    if !checked.insert(path.into()) {
        return;
    }
    fixture.refuse_writes(path);
    for entry in fs::read_dir(path)
        .unwrap_or_else(|e| panic!("FindFirstFileW (LPAC storage {}): {e}", path.display()))
    {
        let entry = entry.expect("FindNextFileW (LPAC storage)");
        let kind = entry
            .file_type()
            .expect("GetFileAttributesW (LPAC storage entry)");
        if kind.is_dir() || kind.is_symlink() {
            check_storage_tree(fixture, &entry.path(), checked);
        }
    }
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain([0]).collect()
}

unsafe fn copy_wide(value: *const u16) -> OsString {
    let mut length = 0;
    // SAFETY: callers pass a live NUL-terminated string from a Windows allocation.
    unsafe {
        while *value.add(length) != 0 {
            length += 1;
        }
        OsString::from_wide(std::slice::from_raw_parts(value, length))
    }
}

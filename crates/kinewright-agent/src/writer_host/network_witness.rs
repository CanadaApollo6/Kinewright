//! Credential-free control and contained runs of the exact native network policy.
use super::{ProbeOwner, guardian};
use crate::{
    job_guardian::protocol::{AttemptIdentity, Program, ProviderSandbox},
    orchestration::adapters::{
        CODEX_AUTH_CONTRACT, CredentialFile, CredentialStoreScope, ExecutableIdentity,
    },
};
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
};

struct ProbeRoot(PathBuf);
impl Drop for ProbeRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Compare a fixed unpaid probe outside containment and inside the native policy.
///
/// # Panics
/// Refuses any failed network stage or incomplete native cleanup.
#[doc(hidden)]
pub fn run_installed_codex_network_witness() {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/s4b-c5c");
    fs::create_dir_all(&out).unwrap();
    let root = ProbeRoot(std::env::temp_dir().join(format!("n5-{}", std::process::id())));
    fs::create_dir(&root.0).unwrap();
    let scratch = root.0.join("s");
    fs::create_dir(&scratch).unwrap();
    fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700)).unwrap();
    let home = crate::codex_credentials::prepare_home(&scratch).unwrap();
    let source = root.0.join("synthetic-auth.json");
    fs::write(&source, br#"{"auth_mode":"chatgpt"}"#).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let meta = fs::metadata(&source).unwrap();
    let scope = CredentialStoreScope {
        root: home.clone(),
        single_file: Some(CredentialFile {
            source,
            identity: [meta.dev(), meta.ino()],
        }),
        credential_files: vec!["auth.json".into()],
        lock_files: vec![],
        temporary_file_prefixes: vec![],
        refresh: CODEX_AUTH_CONTRACT.refresh,
    };
    let probe = guardian().with_file_name("kinewright-network-probe");
    let meta = fs::metadata(&probe).unwrap();
    let codex = crate::codex_qualification::executable().unwrap();
    let executable = ExecutableIdentity {
        path: probe.clone(),
        file_identity: [meta.dev(), meta.ino()],
    };
    let endpoint = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut sandbox = crate::native_launcher::sandbox(
        &codex,
        &scratch,
        &scope,
        endpoint.local_addr().unwrap().port(),
        true,
    )
    .unwrap();
    if let ProviderSandbox::LinuxAllowlist {
        executable_identity,
        ..
    } = &mut sandbox
    {
        *executable_identity = executable.file_identity;
    }
    let environment = crate::native_launcher::environment(&scratch, &home);
    let control = std::process::Command::new(&probe)
        .env_clear()
        .envs(&environment)
        .current_dir(&scratch)
        .output()
        .unwrap();
    assert!(control.status.success(), "C5C control probe process");
    let control: crate::network_probe::Report = serde_json::from_slice(&control.stdout).unwrap();
    fs::write(
        out.join("network-control.json"),
        serde_json::to_vec_pretty(&control).unwrap(),
    )
    .unwrap();
    let pipes = crate::provider_pipes::ProviderPipeListeners::bind(&scratch.join("io")).unwrap();
    let program = Program {
        executable: probe,
        arguments: vec![],
        environment,
        working_directory: scratch.clone(),
        pipe_directory: Some(pipes.directory().into()),
        sandbox,
    };
    fs::write(
        out.join("network-policy.json"),
        serde_json::to_vec_pretty(&program).unwrap(),
    )
    .unwrap();
    let fresh = || {
        kinewright_project::bundle_records::SessionId::fresh()
            .unwrap()
            .as_str()
            .to_owned()
    };
    let id = AttemptIdentity {
        project: fresh(),
        run: fresh(),
        task: fresh(),
        attempt: fresh(),
        nonce: fresh(),
        owner_instance: fresh(),
    };
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &root.0.join("claim"),
        &id.project,
        &id.run,
    )
    .unwrap();
    let mut owner = ProbeOwner(root.0.clone());
    let mut session = crate::guardian::ContainedSession::prepare(
        &mut claim,
        &mut owner,
        id,
        program,
        &guardian(),
        &crate::guardian::CertifiedProfile::linux_allowlist(),
    )
    .unwrap();
    let mut pipes = pipes.accept().unwrap();
    pipes.credentials(b"").unwrap();
    pipes
        .output
        .set_read_timeout(Some(std::time::Duration::from_secs(120)))
        .unwrap();
    session.commit(&mut owner).unwrap();
    let diagnostics =
        crate::protocol::limits::stderr_capture(pipes.diagnostics, "c5c-probe").unwrap();
    let mut output = Vec::new();
    let read = pipes.output.take(65537).read_to_end(&mut output);
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    claim.retire(&proof).unwrap();
    claim.finish().unwrap();
    let _ = diagnostics.join().unwrap();
    read.unwrap();
    let contained: crate::network_probe::Report = serde_json::from_slice(&output).unwrap();
    fs::write(
        out.join("network-contained.json"),
        serde_json::to_vec_pretty(&contained).unwrap(),
    )
    .unwrap();
    eprintln!(
        "C5C_UNPAID control={} contained={} roots={}/{} cleanup=true",
        control.passed(),
        contained.passed(),
        control.roots,
        contained.roots
    );
    assert!(control.passed(), "C5C_NETWORK_CONTROL");
    assert!(contained.passed(), "C5C_NETWORK_CONTAINED");
}

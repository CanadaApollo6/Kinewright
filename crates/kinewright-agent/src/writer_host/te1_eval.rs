//! The arm-specific Studio bridge. Native execution requires a consumed lead grant.
#![allow(clippy::missing_errors_doc)]
use super::{
    Arc, AttachmentRole, Core, Document, Duration, HostAuth, HostClient, HostCommand,
    HostCommandOwner, HostConfig, HostError, HostOrigin, HostRefusal, HostReply, Mutex,
    SessionRoots, WriterHost, orchestration, studio_runtime,
};
use crate::orchestration::{
    AdmittedAttempt,
    adapters::{
        AttemptBinding, AuthenticationMode, AuthorizedInvocation, CertificationRefusal,
        CertifiedConnectorProfile, ConnectorRuntimeChecks, ConnectorSpecification,
        ContainedSession, CredentialFile, CredentialStoreScope, EffectiveAuthEvidence,
        EffectiveLaunchContext, ExecutableIdentity, GuardianLauncher, LaunchError,
        RuntimeQualification, SandboxPolicy, available_connectors,
    },
    orchestration_records::{
        AttemptOutcome, AttemptState, DependencyReservation, EffectState, ExecutionIdentity,
        RunCleanup, RunId, RunOutcome, RunPhase, RunProjection, RunWait, TaskState,
    },
};
use crate::te1_eval::{Script, SendContext, Trace};
use kinewright_core::{AgentError, AgentEvent, AgentSession};
use kinewright_project::{
    bundle::{BundleWriteAdmission, ProjectBundle},
    receipts::ReceiptResult,
    studio_records::{
        CommentId, FeedbackAnchor, FeedbackBatch, FeedbackBatchId, FeedbackRevision, FrameRate,
        MetadataChange, MetadataResult, MetadataUpdate, Orientation, PublishSnapshot,
        ReviseRequest, ReviseResult, VersionId,
    },
};
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{OnceLock, Weak},
    thread::JoinHandle,
};

fn io_error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

pub(crate) struct ScriptedLauncher {
    host: OnceLock<Weak<Mutex<HostCommandOwner>>>,
    script: Script,
    trace: Arc<Mutex<Trace>>,
    fixtures: Mutex<Value>,
    observer: Arc<crate::te1_eval::observe::Observations>,
}
struct ScriptedSession {
    events: crossbeam_channel::Receiver<AgentEvent>,
    worker: Option<JoinHandle<()>>,
}
impl AgentSession for ScriptedSession {
    fn send_user_message(&mut self, _: String) -> Result<(), AgentError> {
        Err(AgentError::Protocol(
            "scripted sends require durable Studio dispatch".into(),
        ))
    }
    fn events(&self) -> crossbeam_channel::Receiver<AgentEvent> {
        self.events.clone()
    }
    fn interrupt(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for ScriptedSession {
    fn drop(&mut self) {
        self.interrupt();
    }
}
impl GuardianLauncher for ScriptedLauncher {
    fn launch(
        &self,
        attempt: AdmittedAttempt,
        invocation: AuthorizedInvocation,
    ) -> Result<ContainedSession, LaunchError> {
        self.start(&attempt, &invocation).map_err(LaunchError)
    }
}
impl ScriptedLauncher {
    fn start(
        &self,
        attempt: &AdmittedAttempt,
        invocation: &AuthorizedInvocation,
    ) -> io::Result<ContainedSession> {
        invocation
            .validate(invocation.profile().session_config(), attempt)
            .map_err(io_error)?;
        let identity = attempt.identity().clone();
        let host = self
            .host
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| io_error("eval host missing"))?;
        let profile = invocation.profile();
        // This is the actual hardened recipe, with a disposable synthetic store.
        // It is prepared again but never passed to an OS process constructor.
        let program = crate::native_launcher::program(
            profile,
            std::str::from_utf8(invocation.invocation().bytes()).map_err(io_error)?,
        )?;
        let binding = program
            .input
            .as_ref()
            .ok_or_else(|| io_error("eval input binding missing"))?;
        let mut input = crate::job_guardian::input::seal(invocation.invocation().bytes(), binding)?;
        let mut received = String::new();
        std::io::Read::read_to_string(&mut input, &mut received)?;
        if received.as_bytes() != invocation.invocation().bytes() {
            return Err(io_error("eval stdin bytes changed"));
        }
        let input_digest = kinewright_media::sha256_bytes(received.as_bytes());
        drop(input);
        let mut locked = host.lock().map_err(io_error)?;
        let producer = locked.run_producer(&identity.run).map_err(io_error)?;
        let mut state = locked
            .spending_scope(&producer, &identity.run)
            .map_err(io_error)?;
        let brief = state
            .tasks
            .iter()
            .find(|t| t.id == identity.task)
            .ok_or_else(|| io_error("eval task missing"))?
            .brief
            .clone();
        let candidate_base = locked
            .owner()
            .map_err(io_error)?
            .candidate_binding(&state.tasks[0].brief.scope.base.branch)
            .map_err(io_error)?;
        let child_index = state
            .tasks
            .iter()
            .position(|t| t.id == identity.task)
            .ok_or_else(|| io_error("eval task order missing"))?;
        // The only simulated boundary: no native process exists. A real reserved,
        // charged attempt gets the same task-turn state as a contained child.
        state
            .attempts
            .iter_mut()
            .find(|a| a.identity == identity)
            .ok_or_else(|| io_error("eval attempt missing"))?
            .state = AttemptState::Contained;
        locked
            .publish_native(&producer, state, &identity, vec![])
            .map_err(io_error)?;
        locked
            .native_started(&producer, &identity)
            .map_err(io_error)?;
        let context = SendContext {
            endpoint: profile.binding().endpoint().into(),
            identity: serde_json::to_value(&identity).map_err(io_error)?,
            brief: serde_json::to_value(&brief).map_err(io_error)?,
            prompt: received,
            fixtures: self.fixtures.lock().map_err(io_error)?.clone(),
            child_index,
        };
        self.observer.bind(context.clone())?;
        drop(locked);
        {
            let mut trace = self.trace.lock().map_err(io_error)?;
            trace.scripted_sends += 1;
            trace.recipes += 1;
            trace.input_hashes.push(serde_json::json!({"before":binding.sha256,"after":input_digest,"bytes":binding.byte_length,"role":brief.role}));
            trace.prompts.push(context.prompt.clone());
            trace.sends.push(json!({"identity":context.identity,"brief":context.brief,"accepted_candidate":candidate_base}));
            trace.recipe_argv.push(program.arguments.clone());
            trace.recipe_argument_counts.push(program.arguments.len());
            trace.recipe_bytes.push(serde_json::to_vec(&program)?.len());
            let size = crate::te1_eval::recipe::measure(
                &serde_json::to_value(&program)?,
                &json!({"identity":context.identity,"brief":context.brief,"fixtures":context.fixtures,"endpoint":context.endpoint}),
            );
            trace.recipe_sizes.push(size);
            trace
                .capacity_sizes
                .extend(capacity_recipes(profile, &context.prompt)?);
        }
        let trace = self.trace.clone();
        let script = self.script.clone();
        let provider = profile.selection().connector;
        let (sender, events) = crossbeam_channel::bounded(2);
        let worker = std::thread::Builder::new().name("te1-scripted-transport".into()).spawn(move || {
            let result = (|| -> io::Result<()> {
                script(context)?;
                let mut locked = host.lock().map_err(io_error)?;
                let raw = if provider == "claude" {
                    json!({"type":"result","subtype":"success","uuid":identity.nonce.as_str(),"modelUsage":{"scripted-model":{"inputTokens":100,"outputTokens":20,"cacheReadInputTokens":0,"cacheCreationInputTokens":0}}})
                } else { json!({"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":0}}) };
                locked.ingest_native_raw_as(crate::native_launcher::StreamDialect::of(&provider), &producer, &identity, &raw, 1, false).map_err(io_error)?;
                locked.eval_scripted_completed(&producer, &identity).map_err(io_error)
            })();
            if let Err(error) = result { trace.lock().unwrap().errors.push(error.to_string()); }
            let _ = sender.send(AgentEvent::Done);
        })?;
        Ok(ContainedSession::from_guardian(Box::new(ScriptedSession {
            events,
            worker: Some(worker),
        })))
    }
}
impl HostCommandOwner {
    /// Finite scripted transport only: there is no process or cleanup proof to
    /// manufacture. Publication and checkpoint code remain production code.
    fn eval_scripted_completed(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
    ) -> Result<(), HostError> {
        let mut state = self.spending_scope(producer, &identity.run)?;
        self.persist_runtime_task(&mut state, identity, true)?;
        state
            .attempts
            .iter_mut()
            .find(|a| a.identity == *identity)
            .ok_or_else(|| HostRefusal::new("eval", "attempt missing"))?
            .state = AttemptState::Retired(AttemptOutcome::Completed);
        let task = state
            .tasks
            .iter_mut()
            .find(|t| t.id == identity.task)
            .ok_or_else(|| HostRefusal::new("eval", "task missing"))?;
        task.progress.capacity = DependencyReservation::Free;
        task.progress.turn_ended = true;
        if matches!(task.state, TaskState::Preparing | TaskState::Running(_)) {
            task.state = TaskState::Queued;
        }
        for effect in state
            .effects
            .iter_mut()
            .filter(|e| e.state == EffectState::Claimed(identity.attempt.clone()))
        {
            effect.state = EffectState::Settled;
        }
        crate::orchestration::tasks::refresh_cleanup(&mut state);
        if state.cleanup == RunCleanup::Settled
            && matches!(state.tasks[0].state, TaskState::Terminal(_))
        {
            state.outcome = RunOutcome::Ready;
            state.phase = RunPhase::Finished;
            state.wait = RunWait::None;
        }
        self.publish_native(producer, state, identity, vec![])
    }
}

// Conservative metadata capacity only: build each served role's real recipe
// with the longest offered model/options, then also expand every host-home
// prefix to a 255-byte home. The expanded paths are not executed or opened.
fn capacity_recipes(profile: &CertifiedConnectorProfile, prompt: &str) -> io::Result<Vec<Value>> {
    let mut rows = Vec::new();
    let selection = crate::orchestration::specialist_setting::specialist_choices()
        .into_iter()
        .filter(|s| s.connector == profile.selection().connector)
        .max_by_key(|s| serde_json::to_vec(s).unwrap().len())
        .ok_or_else(|| io_error("capacity model/options unavailable"))?;
    for role in [
        crate::orchestration::orchestration_records::Role::Director,
        crate::orchestration::orchestration_records::Role::Editor,
        crate::orchestration::orchestration_records::Role::Reviewer,
        crate::orchestration::orchestration_records::Role::Motion,
        crate::orchestration::orchestration_records::Role::Audio,
        crate::orchestration::orchestration_records::Role::Colour,
    ] {
        let mut config = profile.session_config().clone();
        config.model = Some(selection.model.clone());
        for option in &selection.options {
            match option.name.as_str() {
                "model_reasoning_effort" | "effort" => config.effort = Some(option.value.clone()),
                "service_tier" => config.service_tier = Some(option.value.clone()),
                _ => return Err(io_error("capacity option unsupported")),
            }
        }
        config.tool_names = Some(
            crate::orchestration::task_manifest::names(role)
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        );
        let recipe = crate::native_launcher::recipe(
            profile.specification(),
            profile.binding(),
            profile.runtime(),
            &config,
            prompt,
        )?;
        let text = serde_json::to_string(&recipe)?;
        let path = profile.binding().working_directory().to_string_lossy();
        let expanded = path
            .strip_prefix("/home/")
            .and_then(|p| p.split('/').next())
            .map_or(text.len(), |user| {
                text.replace(
                    &format!("/home/{user}/"),
                    &format!("/home/{}/", "h".repeat(249)),
                )
                .len()
            });
        rows.push(json!({"role":role,"selection":selection,"home_bytes":std::env::var("HOME").map_err(io_error)?.len(),"program_bytes":text.len(),"all_home_paths_255_bytes":expanded}));
    }
    if profile.specification().provider() == "codex" {
        crate::native_runtime::write_model_catalog(
            profile.binding().working_directory(),
            profile.session_config().model.as_deref(),
        )?;
    }
    Ok(rows)
}

/// Synthetic qualification reads only files created beneath this run's temp
/// directory. Never calls `InstalledNativeChecks` or resolves a person's store.
#[derive(Default)]
struct SyntheticChecks {
    cache: Mutex<std::collections::BTreeMap<String, RuntimeQualification>>,
    paths: Mutex<Vec<PathBuf>>,
}
impl Drop for SyntheticChecks {
    fn drop(&mut self) {
        for path in self.paths.get_mut().unwrap().drain(..).rev() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}
impl ConnectorRuntimeChecks for SyntheticChecks {
    fn preflight(&self, _: &ConnectorSpecification) -> Result<(), CertificationRefusal> {
        Ok(())
    }
    fn probe(
        &self,
        spec: &ConnectorSpecification,
        binding: &AttemptBinding,
    ) -> Result<RuntimeQualification, CertificationRefusal> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| CertificationRefusal::UnknownAuthentication)?;
        let key = binding.identity().nonce.as_str().to_owned();
        if let Some(value) = cache.get(&key) {
            return Ok(value.clone());
        }
        self.paths
            .lock()
            .map_err(|_| CertificationRefusal::UnknownAuthentication)?
            .extend([
                binding.working_directory().to_path_buf(),
                binding
                    .working_directory()
                    .with_extension("te1-synthetic-auth"),
            ]);
        let result = synthetic_runtime(spec, binding).map_err(|e| {
            CertificationRefusal::ExecutableResolution(format!("synthetic recipe: {e}"))
        })?;
        cache.insert(key, result.clone());
        Ok(result)
    }
}
fn synthetic_runtime(
    spec: &ConnectorSpecification,
    binding: &AttemptBinding,
) -> io::Result<RuntimeQualification> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let scratch = binding.working_directory();
    let auth = scratch.with_extension("te1-synthetic-auth");
    std::fs::create_dir(&auth)?;
    std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o700))?;
    let executable = PathBuf::from("/usr/bin/true");
    let metadata = std::fs::metadata(&executable)?;
    let executable = ExecutableIdentity {
        path: executable,
        file_identity: [metadata.dev(), metadata.ino()],
    };
    let (root, single_file, credential_files) = if spec.provider() == "codex" {
        let path = auth.join("auth.json");
        std::fs::write(&path, br#"{"auth_mode":"chatgpt"}"#)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let m = std::fs::metadata(&path)?;
        (
            crate::codex_credentials::prepare_home(scratch)?,
            Some(CredentialFile {
                source: path,
                identity: [m.dev(), m.ino()],
            }),
            vec![PathBuf::from("auth.json")],
        )
    } else {
        let path = auth.join(".credentials.json");
        std::fs::write(&path,br#"{"claudeAiOauth":{"accessToken":"synthetic-never-sent","expiresAt":9000000000000,"scopes":["user:inference"],"subscriptionType":"max"}}"#)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        (auth, None, vec![PathBuf::from(".credentials.json")])
    };
    let store = CredentialStoreScope {
        root,
        single_file,
        credential_files,
        lock_files: vec![],
        temporary_file_prefixes: vec![],
        refresh: spec.auth_contract().refresh,
    };
    let context = EffectiveLaunchContext {
        environment_digest: [1; 32],
        settings_digest: [2; 32],
        environment_cleared: true,
        settings_isolated: true,
        credential_store: store.clone(),
        executable: executable.clone(),
    };
    let authentication = EffectiveAuthEvidence {
        contract: spec.auth_contract(),
        context: context.clone(),
        at_launch: AuthenticationMode::Subscription,
        at_recheck: Some(AuthenticationMode::Subscription),
        observed_store: store,
    };
    let mut result = RuntimeQualification {
        version: spec.minimum_version(),
        authentication,
        launch_context: context,
        executable,
        sandbox: Some(SandboxPolicy::LinuxFilesystemAndIpc),
        model_available: true,
    };
    let program = crate::native_launcher::recipe(
        spec,
        binding,
        &result,
        &spec.session_config(binding),
        "synthetic prompt",
    )?;
    result.launch_context.environment_digest = digest(&program.environment)?;
    let args = program.arguments;
    result.launch_context.settings_digest = digest(&args)?;
    result.authentication.context = result.launch_context.clone();
    Ok(result)
}
fn digest(value: &impl serde::Serialize) -> io::Result<[u8; 32]> {
    let hash = kinewright_media::sha256_bytes(&serde_json::to_vec(value)?);
    Ok(std::array::from_fn(|i| {
        u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).unwrap()
    }))
}

pub(crate) struct Studio {
    pub host: WriterHost,
    pub run: RunId,
    path: PathBuf,
    manual: Vec<u8>,
    trace: Arc<Mutex<Trace>>,
}
impl Studio {
    #[allow(
        clippy::too_many_arguments,
        reason = "Arm bridge receives the frozen fixture objects and scripted transport"
    )]
    pub fn start(
        root: &Path,
        mut document: Document,
        provider: &str,
        assignment: &[u8],
        report: &[u8],
        transcript: &[u8],
        script: Option<Script>,
        grant: Option<Arc<crate::te1_eval::grant::ConsumedGrant>>,
        trace: Arc<Mutex<Trace>>,
    ) -> io::Result<Self> {
        let case = serde_json::from_slice::<Value>(assignment)?["case"]
            .as_u64()
            .ok_or_else(|| io_error("case missing"))?;
        if case == 6
            && !document
                .tracks
                .iter()
                .any(|t| t.id.0 == 3 && t.clips.iter().any(|c| c.id.0 == 7))
        {
            return Err(io_error(
                "TE15_B2A_CAPTIONS_PRESTART: captions track and clip must exist before Start run",
            ));
        }
        trace.lock().map_err(io_error)?.initial_tracks =
            document.tracks.iter().map(|t| t.id.0).collect();
        let path = root.join("project.kinewright");
        let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document())
            .map_err(io_error)?;
        let empty = bundle.empty_inputs().map_err(io_error)?.reference.clone();
        let mut manifest = (*bundle
            .store()
            .read_input_manifest(&empty)
            .map_err(io_error)?)
        .clone();
        let mut pins = Vec::new();
        for asset in &mut document.media_pool {
            let source = crate::te1_eval::media_source(asset.id.0);
            let relative = source
                .file_name()
                .ok_or_else(|| io_error("fixture file name missing"))?;
            let locator = root.join(relative);
            std::fs::copy(&source, &locator)?;
            let mut file = std::fs::File::open(&locator)?;
            let length = file.metadata()?.len();
            let (mut imported, entry, retained) = bundle
                .store_mut()
                .map_err(io_error)?
                .import_media(
                    &mut file,
                    length,
                    asset.id,
                    &locator,
                    Some(if asset.id.0 == 2 {
                        kinewright_core::ColorDescription {
                            primaries: kinewright_core::ColorPrimaries::Bt709,
                            transfer: kinewright_core::ColorTransfer::Bt709,
                            matrix: kinewright_core::ColorMatrix::Bt709,
                            range: kinewright_core::ColorRange::Limited,
                            white_point: kinewright_core::ColorWhitePoint::D65,
                            bit_depth: kinewright_core::ColorBitDepth::Eight,
                            confidence_basis_points: 10000,
                            provenance: kinewright_core::ColorProvenance::UserOverride,
                        }
                    } else {
                        asset.color_description.clone()
                    }),
                )
                .map_err(io_error)?;
            if (
                imported.fps,
                imported.duration,
                imported.kind,
                imported.resolution,
            ) != (
                asset.fps,
                asset.duration,
                if asset.id.0 == 1 {
                    kinewright_core::MediaKind::Video
                } else {
                    kinewright_core::MediaKind::AudioVideo
                },
                if asset.id.0 == 2 {
                    Some((16, 16))
                } else {
                    asset.resolution
                },
            ) {
                return Err(io_error(format!(
                    "fixture admission changes media timing: {imported:?} vs {asset:?}"
                )));
            }
            imported.name.clone_from(&asset.name);
            imported.path = relative.into();
            *asset = imported;
            manifest.entries.push(entry);
            pins.extend(retained);
        }
        let font = kinewright_core::title_font_bytes();
        let reference = kinewright_project::bundle_records::ObjectRef {
            sha256: kinewright_media::sha256_bytes(font),
            byte_length: font.len() as u64,
            record_kind: kinewright_project::bundle_records::RecordKind::OwnedFont,
        };
        pins.push(
            bundle
                .store_mut()
                .map_err(io_error)?
                .admit_blob(&mut &font[..], &reference, &[])
                .map_err(io_error)?,
        );
        manifest
            .entries
            .push(kinewright_project::owned_inputs::OwnedInputEntry {
                entry_id: "font:Inter".into(),
                object: reference,
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            });
        let sealed = bundle.admit_inputs(&manifest).map_err(io_error)?;
        let inputs = sealed.reference.clone();
        pins.push(sealed);
        let core = Core::spawn(document).map_err(io_error)?;
        core.request(kinewright_core::Command::Do(
            kinewright_core::Operation::MoveMarker {
                marker: kinewright_core::MarkerId(2),
                to: kinewright_core::TimeCode(94),
            },
        ))
        .map_err(io_error)?;
        core.request(kinewright_core::Command::Undo)
            .map_err(io_error)?;
        let manual = serde_json::to_vec(&core.export_history().map_err(io_error)?)?;
        bundle.save(&core, &inputs).map_err(io_error)?;
        drop(pins);
        drop(bundle);
        let spec = available_connectors()
            .into_iter()
            .find(|s| s.provider() == provider)
            .ok_or_else(|| io_error("unsupported eval provider"))?;
        let settings = root.join("scheduler");
        crate::orchestration::director_setting::write_director_setting(
            Some(&settings),
            &grant.as_ref().map_or_else(
                || Ok(spec.selection()),
                |g| g.selection("director").map_err(io_error),
            )?,
        )
        .map_err(io_error)?;
        let case = serde_json::from_slice::<Value>(assignment)?["case"]
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| io_error("case missing"))?;
        let observer = Arc::new(crate::te1_eval::observe::Observations::new(
            case,
            trace.clone(),
        ));
        let launcher = script.map(|script| {
            Arc::new(ScriptedLauncher {
                host: OnceLock::new(),
                script,
                trace: trace.clone(),
                fixtures: Mutex::new(Value::Null),
                observer: observer.clone(),
            })
        });
        let native = grant.map(|permit| {
            Arc::new(NativeEvaluation {
                host: OnceLock::new(),
                permit,
                run_binding: Mutex::new(None),
                root: root.to_path_buf(),
                trace: trace.clone(),
                fixtures: Mutex::new(Value::Null),
                observer: observer.clone(),
            })
        });
        let ports = if let Some(native) = &native {
            let selected = native.permit.selection("editor")?;
            crate::orchestration::specialist_setting::write_specialist_setting(
                Some(&settings),
                &crate::orchestration::specialist_setting::SpecialistChoice::Specific(selected),
            )
            .map_err(io_error)?;
            let guardian = native.permit.guardian()?.to_path_buf();
            studio_runtime::RuntimePorts {
                guardian: guardian.clone(),
                checks: Arc::new(crate::codex_qualification::InstalledNativeChecks { guardian }),
                evaluation_native: Some(native.clone()),
                ceiling: std::num::NonZeroU64::new(
                    native.permit.grant().limits["tokens_per_task"]
                        .as_u64()
                        .ok_or_else(|| io_error("token limit missing"))?,
                ),
                fixture_autostart: true,
                ..Default::default()
            }
        } else {
            studio_runtime::RuntimePorts {
                checks: Arc::new(SyntheticChecks::default()),
                evaluation: launcher.clone(),
                evaluation_counts: Some(trace.clone()),
                fixture_claim: Some(root.join("machine")),
                fixture_autostart: true,
                ..Default::default()
            }
        };
        let host = WriterHost::start_inner(
            HostConfig {
                project: "TE1 unpaid measurement".into(),
                path: Some(path.clone()),
                roots: SessionRoots {
                    read: vec![root.into()],
                    write: vec![root.into()],
                },
                origin: HostOrigin::Headless,
                scheduler_settings: Some(settings),
            },
            HostAuth::generate().map_err(io_error)?,
            None,
            ports,
        )
        .map_err(io_error)?;
        if let Some(launcher) = &launcher {
            launcher
                .host
                .set(host.owner.clone())
                .map_err(|_| io_error("eval owner already set"))?;
        }
        if let Some(native) = &native {
            native
                .host
                .set(host.owner.clone())
                .map_err(|_| io_error("eval owner already set"))?;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            host.runtime.update();
            if !host
                .owner
                .upgrade()
                .ok_or_else(|| io_error("eval owner gone"))?
                .lock()
                .map_err(io_error)?
                .native_recovery_pending
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(io_error("eval recovery deadline"));
            }
            std::thread::park_timeout(Duration::from_millis(10));
        }
        let owner = host
            .owner
            .upgrade()
            .ok_or_else(|| io_error("eval owner gone"))?;
        let mut locked = owner.lock().map_err(io_error)?;
        locked.fixture_director = false;
        locked
            .evaluation_observer
            .set(observer.clone())
            .map_err(|_| io_error("eval observer already installed"))?;
        let explanation = locked
            .owner_mut()
            .map_err(io_error)?
            .admit_host_grant(report)
            .map_err(io_error)?;
        let transcript = locked
            .owner_mut()
            .map_err(io_error)?
            .admit_host_grant(transcript)
            .map_err(io_error)?;
        let fixture_refs = json!({"child_report":explanation,"transcript":transcript.clone()});
        if let Some(launcher) = &launcher {
            *launcher.fixtures.lock().map_err(io_error)? = fixture_refs.clone();
        }
        if let Some(native) = &native {
            *native.fixtures.lock().map_err(io_error)? = fixture_refs;
            observer.set_native_report(explanation.clone())?;
        }
        // Studio's ordinary submitted-feedback / ReviseCut path, not a task
        // fixture projection. These are durable objects in this run's bundle.
        let (checkpoint_ref, checkpoint) = locked
            .owner()
            .map_err(io_error)?
            .manual_checkpoint()
            .map_err(io_error)?;
        drop(locked);
        let client = host.client();
        let publisher_auth = HostAuth::generate().map_err(io_error)?;
        owner
            .lock()
            .map_err(io_error)?
            .register_attachment(
                publisher_auth.clone(),
                "TE1 snapshot publisher".into(),
                AttachmentRole::Producer,
                host.roots.clone(),
                false,
            )
            .map_err(io_error)?;
        let publisher =
            HostClient::connect(host.endpoint.clone(), publisher_auth, host.roots.clone())
                .map_err(io_error)?;
        let snapshot_receipt = metadata(
            &publisher,
            MetadataChange::PublishSnapshot(PublishSnapshot {
                version: VersionId::fresh().map_err(io_error)?,
                expected_head: checkpoint.head_id.clone(),
                checkpoint_ref,
                parent_version: None,
                run_provenance: None,
                revision_refs: vec![],
            }),
        )?;
        let ReceiptResult::Metadata {
            result: MetadataResult::Snapshot(snapshot),
        } = snapshot_receipt.result
        else {
            return Err(io_error("eval snapshot refused"));
        };
        let mut revisions = Vec::new();
        for note in 0..if case == 5 { 2 } else { 1 } {
            let note_bytes = if case == 5 {
                ["marker-one-frame: Move marker 1 from frame 5 to 6. Preserve all other markers and edits.", "correct-homophone: Correct the caption to we chose their design. Preserve neighbouring words and timing."][note].as_bytes().to_vec()
            } else {
                assignment.to_vec()
            };
            let mut locked = owner.lock().map_err(io_error)?;
            let text = locked
                .owner_mut()
                .map_err(io_error)?
                .admit_host_grant(&note_bytes)
                .map_err(io_error)?;
            let empty = locked
                .owner_mut()
                .map_err(io_error)?
                .admit_host_grant(b"[]")
                .map_err(io_error)?;
            drop(locked);
            let saved = metadata(
                &client,
                MetadataChange::SaveFeedback(FeedbackRevision {
                    comment_id: CommentId::fresh().map_err(io_error)?,
                    revision: 1,
                    predecessor: None,
                    anchor: FeedbackAnchor {
                        snapshot: snapshot.clone(),
                        frame: if note == 0 { 5 } else { 20 },
                        fps: FrameRate {
                            numerator: 25,
                            denominator: 1,
                        },
                        raster_size: [160, 90],
                        orientation: Orientation::Upright,
                        target: None,
                        source_ref: None,
                        script_beat_id: None,
                    },
                    original_text_ref: text.clone(),
                    strokes_ref: empty,
                    frame_proof_ref: text,
                    voice_ref: None,
                    transcript_revision: Some(transcript.clone()),
                }),
            )?;
            let ReceiptResult::Metadata {
                result: MetadataResult::Feedback(revision),
            } = saved.result
            else {
                return Err(io_error("eval note refused"));
            };
            trace
                .lock()
                .map_err(io_error)?
                .feedback_revisions
                .push(serde_json::to_value(&revision)?);
            revisions.push(revision);
        }
        let issued = issue(&client)?;
        let batch = FeedbackBatchId::fresh().map_err(io_error)?;
        let request = ReviseRequest {
            command_id: issued.envelope.id.clone(),
            batch_id: batch.clone(),
            expected_candidate_head: checkpoint.head_id,
        };
        checked(
            client
                .request(HostCommand::Metadata {
                    envelope: issued.envelope,
                    update: MetadataUpdate {
                        expected_root_generation: issued.root_generation,
                        change: MetadataChange::SubmitBatch {
                            batch: FeedbackBatch {
                                batch_id: batch,
                                revision_refs: revisions,
                            },
                            request: request.clone(),
                        },
                    },
                })
                .map_err(io_error)?,
        )?;
        let result = checked(
            client
                .request(HostCommand::ReviseCut { request })
                .map_err(io_error)?,
        )?
        .revise_result;
        let Some(ReviseResult::Accepted { run_id, .. }) = result else {
            return Err(io_error(format!("eval ReviseCut refused: {result:?}")));
        };
        let run = serde_json::from_value(serde_json::to_value(run_id)?)?;
        Ok(Self {
            host,
            run,
            path,
            manual,
            trace,
        })
    }
    pub fn projection(&self) -> io::Result<RunProjection> {
        // Headless observation must advance the same worker-completion and
        // queued-wake lifecycle that ordinary host requests drive.
        self.host.runtime.update();
        let owner = self
            .host
            .owner
            .upgrade()
            .ok_or_else(|| io_error("eval host gone"))?;
        let locked = owner.lock().map_err(io_error)?;
        let state = locked
            .owner()
            .map_err(io_error)?
            .orchestration_projection(&self.run)
            .map_err(io_error)?
            .ok_or_else(|| io_error("eval run gone"))?;
        if let RunWait::ProviderSettings(detail) = &state.wait {
            let bytes = locked
                .owner()
                .map_err(io_error)?
                .store()
                .map_err(io_error)?
                .read_evidence(&detail.reason)
                .map_err(io_error)?;
            return Err(io_error(format!(
                "scripted runtime refused: {}",
                String::from_utf8_lossy(&bytes)
            )));
        }
        Ok(state)
    }
    pub fn finish(self) -> io::Result<(Document, Value, Value)> {
        let owner = self
            .host
            .owner
            .upgrade()
            .ok_or_else(|| io_error("eval host gone"))?;
        let locked = owner.lock().map_err(io_error)?;
        let state = locked
            .owner()
            .map_err(io_error)?
            .orchestration_projection(&self.run)
            .map_err(io_error)?
            .ok_or_else(|| io_error("eval run gone"))?;
        let candidate = locked
            .owner()
            .map_err(io_error)?
            .candidate_binding(&state.tasks[0].brief.scope.base.branch)
            .map_err(io_error)?;
        let store = locked
            .owner()
            .map_err(io_error)?
            .store()
            .map_err(io_error)?;
        let checkpoint = store
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                &candidate.checkpoint,
            )
            .map_err(io_error)?;
        let history = (*store
            .resolve_history(&checkpoint.history_ref)
            .map_err(io_error)?)
        .clone();
        let durable = store
            .read_record::<Document>(&checkpoint.document_ref)
            .map_err(io_error)?;
        if *history.document != *durable || checkpoint.document_hash != candidate.document_hash {
            return Err(io_error(
                "candidate durable checkpoint differs from history",
            ));
        }
        let manual = locked
            .owner()
            .map_err(io_error)?
            .core()
            .export_history()
            .map_err(io_error)?;
        if serde_json::to_vec(&manual)? != self.manual {
            return Err(io_error("person's manual history changed"));
        }
        let accounting = locked
            .native_accounting(&state)
            .map_err(io_error)?
            .ok_or_else(|| io_error("eval accounting missing"))?;
        let document = history.document.clone();
        let value = serde_json::to_value(history)?;
        let accounting = serde_json::to_value(accounting)?;
        drop(locked);
        // Read back authoritative data from the actual bundle, never the trace.
        if !self.path.exists() || !self.trace.lock().map_err(io_error)?.errors.is_empty() {
            return Err(io_error("eval failed before durable result"));
        }
        drop(owner);
        self.host.shutdown().map_err(io_error)?;
        let loaded =
            kinewright_project::bundle::LoadedBundle::read(&self.path).map_err(io_error)?;
        let reopened_manual =
            Core::spawn(std::sync::Arc::clone(&manual.baseline)).map_err(io_error)?;
        loaded.restore_manual(&reopened_manual).map_err(io_error)?;
        if reopened_manual.export_history().map_err(io_error)? != manual
            || manual.cursor.redo.len() != 1
        {
            return Err(io_error("reopened manual redo differs"));
        }
        reopened_manual
            .request(kinewright_core::Command::Redo)
            .map_err(io_error)?;
        let redone = reopened_manual.export_history().map_err(io_error)?;
        if !redone
            .document
            .markers
            .iter()
            .any(|m| m.id.0 == 2 && m.position.0 == 94)
        {
            return Err(io_error("person's redo no longer moves their marker"));
        }
        reopened_manual
            .request(kinewright_core::Command::Undo)
            .map_err(io_error)?;
        let undone = reopened_manual.export_history().map_err(io_error)?;
        if undone.document != manual.document || undone.cursor != manual.cursor {
            return Err(io_error("person's redo/undo changed"));
        }
        let reopened = loaded.into_writer().map_err(io_error)?;
        let durable = reopened
            .branch_history(&candidate.branch)
            .map_err(io_error)?;
        if serde_json::to_value(&*durable)? != value {
            return Err(io_error("reopened candidate history differs"));
        }
        Ok(((*document).clone(), value, accounting))
    }
}
fn checked(reply: HostReply) -> io::Result<HostReply> {
    if let Some(error) = &reply.error {
        Err(io_error(format!("host: {error:?}")))
    } else {
        Ok(reply)
    }
}
fn issue(client: &HostClient) -> io::Result<orchestration::MetadataIssuer> {
    checked(
        client
            .request(HostCommand::IssueMetadataIssuer { issuer: None })
            .map_err(io_error)?,
    )?
    .metadata_issuer
    .ok_or_else(|| io_error("metadata issuer missing"))
}
fn metadata(
    client: &HostClient,
    change: MetadataChange,
) -> io::Result<kinewright_project::receipts::CommandReceipt> {
    let issued = issue(client)?;
    checked(
        client
            .request(HostCommand::Metadata {
                envelope: issued.envelope,
                update: MetadataUpdate {
                    expected_root_generation: issued.root_generation,
                    change,
                },
            })
            .map_err(io_error)?,
    )?
    .metadata
    .ok_or_else(|| io_error("metadata receipt missing"))
}

/// Exists only after a durable lead-grant receipt; normal Studio cannot create it.
pub(super) struct NativeEvaluation {
    host: OnceLock<Weak<Mutex<HostCommandOwner>>>,
    permit: Arc<crate::te1_eval::grant::ConsumedGrant>,
    run_binding: Mutex<Option<RunId>>,
    root: PathBuf,
    trace: Arc<Mutex<Trace>>,
    fixtures: Mutex<Value>,
    observer: Arc<crate::te1_eval::observe::Observations>,
}
impl NativeEvaluation {
    pub(super) fn guardian_before_spawn(&self) -> io::Result<()> {
        self.permit.verify_guardian(self.permit.guardian()?)
    }
    pub(super) fn before_launch(&self, invocation: &AuthorizedInvocation) -> io::Result<()> {
        let identity = invocation.profile().binding().identity();
        {
            use std::io::Write;
            let mut bound = self.run_binding.lock().map_err(io_error)?;
            match bound.as_ref() {
                Some(run) if run != &identity.run => {
                    return Err(io_error("grant already bound to another Studio run"));
                }
                Some(_) => {}
                None => {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(self.root.join("run-binding.json"))?;
                    file.write_all(&serde_json::to_vec(
                        &json!({"grant":self.permit.grant(),"studio_run":identity.run}),
                    )?)?;
                    file.sync_all()?;
                    std::fs::File::open(&self.root)?.sync_all()?;
                    *bound = Some(identity.run.clone());
                }
            }
        }
        let host = self
            .host
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| io_error("eval host missing"))?;
        let locked = host.lock().map_err(io_error)?;
        let state = locked
            .owner()
            .map_err(io_error)?
            .orchestration_projection(&identity.run)
            .map_err(io_error)?
            .ok_or_else(|| io_error("eval run missing"))?;
        let task = state
            .tasks
            .iter()
            .find(|t| t.id == identity.task)
            .ok_or_else(|| io_error("eval task missing"))?;
        let role = serde_json::to_value(task.brief.role)?;
        if self.permit.grant().dry_pass["cli_sha256"]
            != kinewright_media::sha256_file(&invocation.profile().runtime().executable.path)
                .map_err(io_error)?
        {
            return Err(io_error("provider image changed since the dry pass"));
        }
        if task.brief.provider
            != self
                .permit
                .selection(role.as_str().ok_or_else(|| io_error("role missing"))?)?
        {
            return Err(io_error("paid role differs from grant"));
        }
        let prompt = std::str::from_utf8(invocation.invocation().bytes())
            .map_err(io_error)?
            .to_owned();
        let program = crate::native_launcher::program(invocation.profile(), &prompt)?;
        let context = SendContext {
            endpoint: invocation.profile().binding().endpoint().into(),
            identity: serde_json::to_value(identity)?,
            brief: serde_json::to_value(&task.brief)?,
            prompt: prompt.clone(),
            fixtures: self.fixtures.lock().map_err(io_error)?.clone(),
            child_index: state
                .tasks
                .iter()
                .position(|t| t.id == identity.task)
                .unwrap(),
        };
        self.observer.bind(context)?;
        let mut trace = self.trace.lock().map_err(io_error)?;
        trace.launch_requests += 1;
        trace.provider_launches += 1;
        trace.recipes += 1;
        trace.prompts.push(prompt.clone());
        trace.recipe_argv.push(program.arguments.clone());
        trace.recipe_argument_counts.push(program.arguments.len());
        trace.recipe_bytes.push(serde_json::to_vec(&program)?.len());
        trace.recipe_sizes.push(crate::te1_eval::recipe::measure(&serde_json::to_value(&program)?,&json!({"identity":identity,"brief":task.brief,"fixtures":self.fixtures.lock().map_err(io_error)?.clone(),"endpoint":invocation.profile().binding().endpoint()})));
        let binding = program
            .input
            .as_ref()
            .ok_or_else(|| io_error("input missing"))?;
        trace.input_hashes.push(json!({"before":binding.sha256,"after":kinewright_media::sha256_bytes(prompt.as_bytes()),"bytes":binding.byte_length,"role":task.brief.role}));
        trace.sends.push(json!({"identity":identity,"brief":task.brief,"accepted_candidate":locked.owner().map_err(io_error)?.candidate_binding(&task.brief.scope.base.branch).map_err(io_error)?}));
        Ok(())
    }
}

//! Adapters to current Core/branch/project APIs, without a toy Studio host.
#![allow(clippy::missing_errors_doc)]
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use kinewright_core::{
    Command, Core, Document, Event, JournalCommand, Query, QueryResult, TimelineRevision,
};
use kinewright_project::{
    ProjectFile, io_observer, load_document, serialize_project_document, write_project_document,
};
use serde_json::{Value, json};

use super::protocol::{Call, CurrentQuery, LoadedFixture, unique_json};
use crate::TimelineBranch;

pub struct Baseline {
    pub core: Option<Core>,
    branches: BTreeMap<String, TimelineBranch>,
    pub root: PathBuf,
    lock: Option<kinewright_project::LockfileHandle>,
    server: Option<crate::McpServer>,
    media: Option<Arc<kinewright_media::FfmpegMediaEngine>>,
}

impl Baseline {
    pub fn new(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        Ok(Self {
            core: None,
            branches: BTreeMap::new(),
            root: root.to_path_buf(),
            lock: None,
            server: None,
            media: None,
        })
    }

    pub fn core(&self) -> Result<&Core, String> {
        self.core.as_ref().ok_or_else(|| "core not started".into())
    }

    pub fn media(&mut self) -> Result<Arc<kinewright_media::FfmpegMediaEngine>, String> {
        if self.media.is_none() {
            self.media = Some(Arc::new(
                kinewright_media::FfmpegMediaEngine::new_with_data_dir(
                    self.root.join("media-cache"),
                )
                .map_err(|e| format!("media prerequisite NOT RUN: {e}"))?,
            ));
        }
        self.media.clone().ok_or_else(|| "media missing".into())
    }

    pub fn require_generated_media_control(&self) -> Result<(), String> {
        let (_, document) = self.snapshot()?;
        if !document.media_pool.is_empty() || !document.lut_assets.is_empty() {
            return Err("S0 media control needs self-contained generated clips; supplied-media/snapshot fixture adapter pending".into());
        }
        if document.resolution.0 > 2048 || document.resolution.1 > 2048 || document.duration.0 > 300
        {
            return Err(
                "S0 generated control exceeds raster/frame admission bound; NOT RUN".into(),
            );
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<(TimelineRevision, Arc<Document>), String> {
        let Event::QueryResult(QueryResult::Snapshot { revision, document }) = self
            .core()?
            .request(Command::Query(Query::Snapshot))
            .map_err(|e| e.to_string())?
        else {
            return Err("unexpected snapshot response".into());
        };
        Ok((revision, document))
    }

    #[allow(clippy::too_many_lines)] // Exhaustive current-API dispatch, with no second host.
    pub fn execute(&mut self, call: &Call, fixture: &LoadedFixture) -> Result<Value, String> {
        match call {
            Call::LockAcquire(args) => {
                if self.lock.is_some() {
                    return Err("participant already holds a lock".into());
                }
                let path = self.output_path(&args.path)?;
                match kinewright_project::acquire_project_lock_with_policy(
                    &path,
                    kinewright_project::LockMode::Headless,
                    "http://127.0.0.1:9/mcp",
                    &self.root.join("recovery"),
                    1,
                    std::time::Duration::ZERO,
                ) {
                    Ok(acquired) => {
                        self.lock = Some(acquired.handle);
                        Ok(json!({"acquired":true}))
                    }
                    Err(error) => Ok(json!({"acquired":false,"error":error.to_string()})),
                }
            }
            Call::LockVerify(_) => {
                Ok(json!({"verified":self.lock.as_ref().ok_or("no held lock")?.verify().is_ok()}))
            }
            Call::LockRelease(_) => {
                self.lock
                    .take()
                    .ok_or("no held lock")?
                    .release()
                    .map_err(|e| e.to_string())?;
                Ok(json!({"released":true}))
            }
            Call::HeadlessSave(args) => self.headless_save(&args.path),
            Call::ServerStart(_) => {
                self.require_generated_media_control()?;
                if self.server.is_some() {
                    return Err("server already started".into());
                }
                let media = self.media()?;
                self.server = Some(
                    crate::McpServer::start(self.core()?.clone(), media.clone(), media.clone())
                        .map_err(|e| e.to_string())?,
                );
                self.media = Some(media);
                Ok(
                    json!({"endpoint":self.server.as_ref().ok_or("no server")?.endpoint(),"provider":null}),
                )
            }
            Call::ScriptedSession(args) => super::recording::scripted_session(
                self.server.as_ref().ok_or("server not started")?,
                input(fixture, &args.input)?,
            ),
            Call::PreparedCommit(args) => {
                self.require_generated_media_control()?;
                super::recording::prepared_commit(
                    self.server.as_ref().ok_or("server not started")?,
                    args,
                    input(fixture, &args.input)?,
                )
            }
            Call::CoreStart(_) => {
                if self.server.is_some() {
                    return Err("stop the current server before replacing its Core; adapter lifecycle prerequisite".into());
                }
                let document = match &fixture.fixture.initial.document {
                    None => Document::default(),
                    Some(role) => parse_project(fixture, role)?.document,
                };
                self.core = Some(
                    Core::spawn_at(document, TimelineRevision(fixture.fixture.initial.revision))
                        .map_err(|e| e.to_string())?,
                );
                self.history()
            }
            Call::CoreSend(args) => {
                let target = self.target(args.branch.as_deref())?;
                let command: JournalCommand =
                    serde_json::from_value(unique_json(input(fixture, &args.input)?)?)
                        .map_err(|e| e.to_string())?;
                let command = match (args.expected_revision, command) {
                    (None, command) => Command::from(command),
                    (Some(revision), JournalCommand::Do(operation)) => Command::DoIfRevision {
                        expected: TimelineRevision(revision),
                        operation,
                        token: None,
                    },
                    (Some(revision), JournalCommand::DoBatch(operations)) => {
                        Command::DoBatchIfRevision {
                            expected: TimelineRevision(revision),
                            operations,
                        }
                    }
                    (Some(_), _) => {
                        return Err(
                            "guarded Undo/Redo/coalesced command is an invalid fixture action"
                                .into(),
                        );
                    }
                };
                let event = target.request(command).map_err(|e| e.to_string())?;
                let result = event_value(event);
                Ok(json!({"event":result,"history":history_value(&target)?}))
            }
            Call::CoreQuery(args) => match args.query {
                CurrentQuery::History => history_value(&self.target(args.branch.as_deref())?),
                CurrentQuery::Snapshot => {
                    let Event::QueryResult(QueryResult::Snapshot { revision, document }) = self
                        .target(args.branch.as_deref())?
                        .request(Command::Query(Query::Snapshot))
                        .map_err(|e| e.to_string())?
                    else {
                        return Err("unexpected snapshot response".into());
                    };
                    Ok(json!({"revision":revision,"document":document}))
                }
                CurrentQuery::AppliedOperations | CurrentQuery::OpLog => {
                    let query = if matches!(args.query, CurrentQuery::AppliedOperations) {
                        Query::AppliedOperations
                    } else {
                        Query::OpLog
                    };
                    Ok(event_value(
                        self.target(args.branch.as_deref())?
                            .request(Command::Query(query))
                            .map_err(|e| e.to_string())?,
                    ))
                }
            },
            Call::BranchCreate(args) => {
                if self.branches.len() >= 64 || self.branches.contains_key(&args.name) {
                    return Err("duplicate/too many branches".into());
                }
                let (revision, document) = self.snapshot()?;
                self.branches.insert(
                    args.name.clone(),
                    TimelineBranch::new(args.name.as_str(), revision, document)
                        .map_err(|e| e.to_string())?,
                );
                self.branch(&args.name)
            }
            Call::BranchCompare(args) => self.branch(&args.name),
            Call::BranchMerge(args) => {
                let result = self
                    .branches
                    .get(&args.name)
                    .ok_or("unknown branch")?
                    .merge_into(self.core()?)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"outcome":format!("{result:?}"),"history":self.history()?}))
            }
            Call::BranchCherryPick(args) => {
                let result = self
                    .branches
                    .get(&args.name)
                    .ok_or("unknown branch")?
                    .cherry_pick_into(
                        self.core()?,
                        TimelineRevision(args.expected_revision),
                        &args.indices,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(json!({"outcome":format!("{result:?}"),"history":self.history()?}))
            }
            Call::ProjectLoad(args) => {
                let file = self.root.join("loaded.kinewright");
                fs::write(&file, input(fixture, &args.input)?).map_err(|e| e.to_string())?;
                self.reopen(&file)
            }
            Call::ProjectSerialize(_) => {
                let (_, document) = self.snapshot()?;
                let bytes = serialize_project_document(&document).map_err(|e| e.to_string())?;
                Ok(
                    json!({"serialized":bytes,"sha256":kinewright_media::sha256_bytes(bytes.as_bytes()),"byte_length":bytes.len()}),
                )
            }
            Call::ProjectSave(args) => {
                let (_, document) = self.snapshot()?;
                let path = self.output_path(&args.path)?;
                let (result, trace) =
                    io_observer::observe(|| write_project_document(&document, &path, None));
                result.map_err(|e| e.to_string())?;
                if trace.overflowed {
                    return Err("IO trace overflow".into());
                }
                let bytes = fs::read(&path).map_err(|e| e.to_string())?;
                let expected = serialize_project_document(&document).map_err(|e| e.to_string())?;
                // R2 follows IO observer vocabulary. S1 must emit directory_sync
                // beside the exact parent-directory sync call, never as a synthetic note.
                Ok(
                    json!({"writer_bytes_equal":bytes == expected.as_bytes(),"file":file_value(&path)?,
                    "directory_sync_observed":trace.events.iter().any(|e| e.operation == "directory_sync"),"io":trace}),
                )
            }
            Call::ProjectReopen(args) => {
                let path = self.output_path(&args.path)?;
                self.reopen(&path)
            }
            Call::InspectFiles(args) => {
                let path = self.output_path(&args.path)?;
                file_value(&path)
            }
            Call::LegacyRenameRefusal(args) => {
                let (_, document) = self.snapshot()?;
                let bytes = serialize_project_document(&document).map_err(|e| e.to_string())?;
                let path = self.output_path(&args.path)?;
                let (result, trace) = io_observer::observe(|| {
                    kinewright_project::project::observe_refused_rename(&path, bytes.as_bytes())
                });
                result.map_err(|e| e.to_string())?;
                Ok(
                    json!({"writer_bytes_equal":fs::read(&path).map_err(|e| e.to_string())? == bytes.as_bytes(),
                    "in_place_fallback_observed":trace.events.iter().any(|e| e.operation == "in_place_fallback"),"io":trace}),
                )
            }
            _ => Err("action requires its owning controller/adapter".into()),
        }
    }

    fn headless_save(&self, relative: &Path) -> Result<Value, String> {
        use kinewright_project::{SidecarMode, SidecarSession, digest_bytes};
        use std::sync::RwLock;
        let path = self.output_path(relative)?;
        let (revision, document) = self.snapshot()?;
        let digest = fs::read(&path).map_or_else(|_| String::new(), |b| digest_bytes(&b));
        let admission = if path.exists() {
            kinewright_project::bundle::BundleWriteAdmission::read(&path)
                .map_err(|e| e.to_string())?
        } else {
            kinewright_project::bundle::BundleWriteAdmission::new_document()
        };
        admission
            .authorize_legacy_write(&path)
            .map_err(|e| e.to_string())?;
        let mut loaded = SidecarSession::load(
            &SidecarMode::Load {
                project_digest: digest.clone(),
            },
            Some(&path),
            Arc::new(RwLock::new(kinewright_core::IncidentLog::default())),
            revision,
            None,
            None,
        )
        .session;
        let recovery = self.root.join("recovery");
        fs::create_dir_all(&recovery).map_err(|e| e.to_string())?;
        let (result, trace) = io_observer::observe(|| {
            kinewright_project::save_headless(
                &document,
                &admission,
                &path,
                None,
                &mut loaded,
                &digest,
                revision,
                &recovery,
                self.lock.as_ref(),
            )
        });
        match result {
            Ok(result) => Ok(
                json!({"saved":true,"sidecar":format!("{:?}",result.sidecar),"file":file_value(&path)?,"io":trace}),
            ),
            Err(error) => Ok(json!({"saved":false,"error":error.to_string(),"io":trace})),
        }
    }

    pub fn output_path(&self, relative: &Path) -> Result<PathBuf, String> {
        super::protocol::relative_path(relative)?;
        let path = self.root.join(relative);
        // Refuse symlinks on both existing files and every parent. The private
        // fixture tree is not the future production pinned-root implementation.
        let mut component = self.root.clone();
        for name in relative.components() {
            component.push(name);
            if fs::symlink_metadata(&component).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("output path contains symlink".into());
            }
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        Ok(path)
    }

    pub fn history(&self) -> Result<Value, String> {
        history_value(self.core()?)
    }

    pub fn finish(&mut self) -> Result<Value, String> {
        if let Some(server) = self.server.take() {
            server.shutdown();
        }
        let media = if let Some(media) = self.media.take() {
            let finished = media.teardown_observer();
            drop(media);
            if finished.recv_timeout(std::time::Duration::from_millis(super::MEDIA_TEARDOWN_MS))
                != Err(crossbeam_channel::RecvTimeoutError::Disconnected)
            {
                return Err("media worker teardown not observed; NOT RUN".into());
            }
            Some(json!({"worker_finished":true}))
        } else {
            None
        };
        if let Some(lock) = self.lock.take() {
            lock.release().map_err(|e| e.to_string())?;
        }
        Ok(json!({"media":media}))
    }

    fn target(&self, branch: Option<&str>) -> Result<Core, String> {
        branch.map_or_else(
            || self.core().cloned(),
            |name| {
                self.branches
                    .get(name)
                    .map(TimelineBranch::core)
                    .ok_or_else(|| "unknown branch".into())
            },
        )
    }

    fn branch(&self, name: &str) -> Result<Value, String> {
        let comparison = self
            .branches
            .get(name)
            .ok_or("unknown branch")?
            .compare()
            .map_err(|e| e.to_string())?;
        Ok(
            json!({"name":name,"base_revision":comparison.base_revision,"revision":comparison.branch_revision,
            "base_document":comparison.base_document,"document":comparison.document,"applied_operations":comparison.operations}),
        )
    }

    fn reopen(&mut self, path: &Path) -> Result<Value, String> {
        if self.server.is_some() {
            return Err(
                "stop the current server before reopening its Core; adapter lifecycle prerequisite"
                    .into(),
            );
        }
        let before = self.core.as_ref().map(|_| self.snapshot()).transpose()?;
        let raw = fs::read(path).map_err(|e| e.to_string())?;
        match load_document(path) {
            Ok((document, version, _)) => {
                let serialized =
                    serialize_project_document(&document).map_err(|e| e.to_string())?;
                let same_document = before
                    .as_ref()
                    .is_some_and(|(_, before)| **before == document);
                self.core = Some(Core::spawn(document).map_err(|e| e.to_string())?);
                Ok(
                    json!({"loaded":true,"read_version":version,"can_overwrite":kinewright_project::can_overwrite_save(version),
                    "same_document":same_document,"writer_round_trip_equal":serialized.as_bytes() == raw,
                    "original_bytes_unchanged":fs::read(path).map_err(|e| e.to_string())? == raw,"file":file_value(path)?,"history":self.history()?}),
                )
            }
            Err(error) => Ok(
                json!({"loaded":false,"error":error,"original_bytes_unchanged":fs::read(path).map_err(|e| e.to_string())? == raw}),
            ),
        }
    }
}

impl Drop for Baseline {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn history_value(core: &Core) -> Result<Value, String> {
    let history = core.observe_history().map_err(|e| e.to_string())?;
    Ok(json!({"undo_count":history.undo.len(),"redo_count":history.redo.len(),"trace":history}))
}

fn parse_project(fixture: &LoadedFixture, role: &str) -> Result<ProjectFile, String> {
    serde_json::from_value(unique_json(input(fixture, role)?)?).map_err(|e| e.to_string())
}

pub fn input<'a>(fixture: &'a LoadedFixture, role: &str) -> Result<&'a [u8], String> {
    fixture
        .files
        .get(role)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing captured input {role}"))
}

pub fn file_value(path: &Path) -> Result<Value, String> {
    let raw = super::protocol::read_bounded(path, super::protocol::INDEX_BYTES)?;
    Ok(
        json!({"path":path,"sha256":kinewright_media::sha256_bytes(&raw),"byte_length":raw.len(),
        "utf8":String::from_utf8(raw).ok(),"graph":null}),
    )
}

fn event_value(event: Event) -> Value {
    match event {
        Event::DocumentChanged {
            doc,
            revision,
            journal_command,
            ..
        } => {
            json!({"accepted":true,"revision":revision,"document":doc,"journal_command":journal_command})
        }
        Event::QueryResult(QueryResult::AppliedOperations(ops) | QueryResult::OpLog(ops)) => {
            json!({"operations":ops})
        }
        other => json!({"accepted":false,"event":format!("{other:?}")}),
    }
}

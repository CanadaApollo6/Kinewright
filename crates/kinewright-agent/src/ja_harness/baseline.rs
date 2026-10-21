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
}

impl Baseline {
    pub fn new(root: &Path) -> Result<Self, String> {
        fs::create_dir(root).map_err(|e| e.to_string())?;
        Ok(Self {
            core: None,
            branches: BTreeMap::new(),
            root: root.to_path_buf(),
        })
    }

    pub fn core(&self) -> Result<&Core, String> {
        self.core.as_ref().ok_or_else(|| "core not started".into())
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
            Call::CoreStart(_) => {
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

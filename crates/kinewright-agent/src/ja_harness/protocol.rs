//! Version 1 fixture protocol. These records are eval data, never project types.
#![allow(clippy::missing_errors_doc)]
use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;

pub const PROTOCOL: &str = "kinewright.ja-harness";
pub const CONTROL_BYTES: usize = 256 * 1024;
pub const INDEX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Lane {
    Portable,
    Media,
    AppPaint,
    LinuxNative,
    WindowsNative,
    Provider,
    Practice,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Baseline,
    Revised,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    pub relative_file: PathBuf,
    pub sha256: String,
    pub byte_length: u64,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Initial {
    pub document: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub participants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub action_timeout_ms: u64,
    pub setup_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub protocol: String,
    pub protocol_version: u32,
    pub fixture_id: String,
    pub gate_ids: Vec<String>,
    pub lane: Lane,
    pub requirements: Vec<String>,
    pub inputs: BTreeMap<String, FileRef>,
    pub initial: Initial,
    pub actions: Vec<Action>,
    pub barriers: Vec<Barrier>,
    pub oracles: BTreeMap<String, FileRef>,
    pub limits: Limits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Barrier {
    pub name: String,
    pub after: u32,
    pub positive_control: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: u32,
    pub participant: String,
    pub after: Option<u32>,
    pub retry_of: Option<u32>,
    #[serde(flatten)]
    pub call: Call,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "action",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Call {
    AdapterStatus(Empty),
    S2SavePhaseProbe(Empty),
    S2CoreSealProbe(Empty),
    S2RescueSourceProbe(Empty),
    S2RescueSlotsProbe(Empty),
    S2RescueRecoveryProbe(Empty),
    OwnedInputProbe(InputArgs),
    BundleCopyProbe(InputArgs),
    BundleBudgetProbe(InputArgs),
    BundleFixtureProbe(InputArgs),
    CommentSubmit(CommentArgs),
    CommentInspect(CommentArgs),
    CommentRetire(CommentMaintenanceArgs),
    CommentRotate(CommentMaintenanceArgs),
    StrictSave(IoProbeArgs),
    StrictRenameRefusal(IoProbeArgs),
    CoreStart(Empty),
    CoreSend(CommandArgs),
    CoreQuery(QueryArgs),
    BranchCreate(BranchArgs),
    BranchCompare(BranchArgs),
    BranchMerge(BranchArgs),
    BranchCherryPick(CherryArgs),
    ProjectLoad(InputArgs),
    ProjectSerialize(Empty),
    ProjectSave(PathArgs),
    ProjectReopen(PathArgs),
    InspectFiles(PathArgs),
    LegacyRenameRefusal(PathArgs),
    AssertObserved(Assertion),
    SurfaceAbsent(SurfaceArgs),
    LockAcquire(PathArgs),
    LockVerify(Empty),
    LockRelease(Empty),
    HeadlessSave(PathArgs),
    BarrierWait(BarrierArgs),
    BarrierRelease(BarrierArgs),
    DropReply(ActionRef),
    ParticipantExit(Empty),
    ParticipantRestart(Empty),
    ParticipantKill(Empty),
    ParticipantInventory(Empty),
    SpawnChild(Empty),
    ChildExit(Empty),
    ServerStart(Empty),
    ScriptedSession(InputArgs),
    PreparedCommit(PreparedArgs),
    MediaSeek(FrameArgs),
    MediaProof(ProofArgs),
    MediaExport(PathArgs),
    MediaDecode(DecodeArgs),
    MediaStats(Empty),
    AppFixture(InputArgs),
}

impl Call {
    pub(crate) fn main_adapter_only(&self) -> bool {
        matches!(
            self,
            Self::ServerStart(_)
                | Self::ScriptedSession(_)
                | Self::PreparedCommit(_)
                | Self::AppFixture(_)
                | Self::MediaSeek(_)
                | Self::MediaProof(_)
                | Self::MediaExport(_)
                | Self::MediaDecode(_)
                | Self::MediaStats(_)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IoProbeArgs {
    pub path: PathBuf,
    pub input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionRef {
    pub action_id: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarrierArgs {
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedArgs {
    pub input: String,
    pub expected_revision: u64,
    pub confirmation: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameArgs {
    pub at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofArgs {
    pub at: i64,
    pub max_width: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecodeArgs {
    pub path: PathBuf,
    pub at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandArgs {
    pub input: String,
    pub expected_revision: Option<u64>,
    pub branch: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentArgs {
    pub path: PathBuf,
    pub request: String,
    pub text: String,
    pub strokes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentMaintenanceArgs {
    pub path: PathBuf,
    pub request: String,
    #[serde(default)]
    pub rotations: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputArgs {
    pub input: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathArgs {
    pub path: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchArgs {
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CherryArgs {
    pub name: String,
    pub expected_revision: u64,
    pub indices: Vec<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryArgs {
    pub query: CurrentQuery,
    pub branch: Option<String>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentQuery {
    Snapshot,
    AppliedOperations,
    OpLog,
    History,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceArgs {
    pub surface: String,
    pub owner: String,
    pub positive_control: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    pub id: String,
    pub observation: u32,
    pub pointer: String,
    pub oracle: String,
    #[serde(default)]
    pub oracle_pointer: String,
    pub evidence_class: EvidenceClass,
    pub positive_control: Option<String>,
    #[serde(default)]
    pub comparison: Comparison,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    #[default]
    Equal,
    AtMost,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    Control,
    BaselineProperty,
    /// A new surface with no predecessor endpoint; baseline retains a pending witness.
    RevisedProperty,
}

/// Already captured immutable fixture bytes and verified input/oracle bytes.
pub struct LoadedFixture {
    pub fixture: Fixture,
    pub bytes: Vec<u8>,
    pub files: BTreeMap<String, Vec<u8>>,
}

pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    if !std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("fixture input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(format!("oversize input: {}", path.display()));
    }
    Ok(bytes)
}

pub fn relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("path escape or non-normal relative path".into());
    }
    Ok(())
}

pub fn load(path: &Path) -> Result<LoadedFixture, String> {
    let bytes = read_bounded(path, INDEX_BYTES)?;
    let value = unique_json(&bytes)?;
    let fixture: Fixture = serde_json::from_value(value).map_err(|e| e.to_string())?;
    validate(&fixture)?;
    let root = path
        .parent()
        .ok_or("fixture has no parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let mut files = BTreeMap::new();
    let mut admitted = 0_u64;
    for (prefix, refs) in [("", &fixture.inputs), ("oracle:", &fixture.oracles)] {
        for (role, reference) in refs {
            relative_path(&reference.relative_file)?;
            let resolved = root
                .join(&reference.relative_file)
                .canonicalize()
                .map_err(|e| e.to_string())?;
            if !resolved.starts_with(&root) {
                return Err("input symlink escapes fixture root".into());
            }
            admitted = admitted
                .checked_add(reference.byte_length)
                .ok_or("input quota overflow")?;
            if admitted > fixture.limits.input_bytes {
                return Err("input quota exceeded".into());
            }
            let data = read_bounded(&resolved, reference.byte_length)?;
            if u64::try_from(data.len()).unwrap_or(u64::MAX) != reference.byte_length
                || kinewright_media::sha256_bytes(&data) != reference.sha256
            {
                return Err(format!("input hash/length mismatch: {role}"));
            }
            files.insert(format!("{prefix}{role}"), data);
        }
    }
    for action in &fixture.actions {
        match &action.call {
            Call::ScriptedSession(args) => super::recording::validate_script_fixture(
                files.get(&args.input).ok_or("missing script input")?,
            )?,
            Call::PreparedCommit(args) => super::recording::validate_plan_fixture(
                files
                    .get(&args.input)
                    .ok_or("missing prepared-plan input")?,
            )?,
            _ => {}
        }
        if let Call::CoreSend(args) = &action.call {
            let command: kinewright_core::JournalCommand = serde_json::from_value(unique_json(
                files.get(&args.input).ok_or("missing command input")?,
            )?)
            .map_err(|e| e.to_string())?;
            if args.expected_revision.is_some()
                && !matches!(
                    command,
                    kinewright_core::JournalCommand::Do(_)
                        | kinewright_core::JournalCommand::DoBatch(_)
                )
            {
                return Err(
                    "guarded Undo/Redo/coalesced command is an invalid fixture action".into(),
                );
            }
        }
    }
    Ok(LoadedFixture {
        fixture,
        bytes,
        files,
    })
}

/// Fixture admission only: S0's generated-media controls cannot introduce
/// external media or LUT locators. This is not production task-scope authority.
pub fn validate_local_control_operations(
    operations: &[kinewright_core::Operation],
) -> Result<(), String> {
    if operations.iter().any(|op| {
        matches!(
            op,
            kinewright_core::Operation::AddAsset { .. }
                | kinewright_core::Operation::RelinkAsset { .. }
                | kinewright_core::Operation::AddLutAsset { .. }
        )
    }) {
        return Err("external media/LUT fixture adapter pending; NOT RUN".into());
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "validate every fixture action and prerequisite together"
)]
fn validate(f: &Fixture) -> Result<(), String> {
    if f.protocol != PROTOCOL || f.protocol_version != 1 {
        return Err("wrong protocol/version".into());
    }
    if f.inputs.keys().any(|role| role.starts_with("oracle:")) {
        return Err("input role uses reserved oracle namespace".into());
    }
    if f.fixture_id.is_empty()
        || f.fixture_id.len() > 128
        || f.gate_ids.is_empty()
        || f.gate_ids.len() > 64
    {
        return Err("invalid fixture/gate labels".into());
    }
    if f.inputs.len() > 64
        || f.oracles.len() > 64
        || f.requirements.len() > 64
        || f.actions.len() > 64
        || f.initial.participants.len() > 2
        || f.barriers.len() > 64
        || f.limits.input_bytes > INDEX_BYTES
        || f.limits.output_bytes > INDEX_BYTES
        || f.limits.output_bytes < u64::try_from(2 * CONTROL_BYTES).map_err(|e| e.to_string())?
        || !(1..=600_000).contains(&f.limits.action_timeout_ms)
        || !(1..=600_000).contains(&f.limits.setup_timeout_ms)
    {
        return Err("fixture exceeds harness ceilings".into());
    }
    let mut previous = 0;
    for (role, reference) in f.inputs.iter().chain(&f.oracles) {
        if role.is_empty()
            || role.len() > 128
            || reference.kind.len() > 128
            || reference.sha256.len() != 64
            || !reference
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid bounded input/oracle identity".into());
        }
    }
    if f.gate_ids.iter().any(|g| g.is_empty() || g.len() > 128) {
        return Err("invalid gate label".into());
    }
    for action in &f.actions {
        if action.call.main_adapter_only() && action.participant != "main" {
            return Err(
                "adapter requires main participant; participant adapter unavailable".into(),
            );
        }
        if action.id <= previous
            || action.after.is_some_and(|id| id >= action.id)
            || action.participant.len() > 128
            || action.participant.is_empty()
            || serde_json::to_vec(action).map_err(|e| e.to_string())?.len() > CONTROL_BYTES
        {
            return Err("invalid action order/participant/size".into());
        }
        match &action.call {
            Call::ProjectSave(a)
            | Call::ProjectReopen(a)
            | Call::InspectFiles(a)
            | Call::LegacyRenameRefusal(a)
            | Call::LockAcquire(a)
            | Call::HeadlessSave(a)
            | Call::MediaExport(a) => relative_path(&a.path)?,
            Call::MediaDecode(a) => relative_path(&a.path)?,
            Call::StrictSave(a) | Call::StrictRenameRefusal(a) => {
                relative_path(&a.path)?;
                if !f.inputs.contains_key(&a.input) {
                    return Err("missing IO probe input".into());
                }
            }
            Call::OwnedInputProbe(a)
            | Call::BundleCopyProbe(a)
            | Call::BundleBudgetProbe(a)
            | Call::BundleFixtureProbe(a) => {
                if !f.inputs.contains_key(&a.input) {
                    return Err("missing owned-input probe input".into());
                }
            }
            Call::CommentSubmit(a) | Call::CommentInspect(a) => {
                relative_path(&a.path)?;
                if !f.inputs.contains_key(&a.text)
                    || !f.inputs.contains_key(&a.strokes)
                    || a.request.is_empty()
                    || a.request.len() > 128
                    || a.request.contains(['/', '\\'])
                    || a.original
                        .as_ref()
                        .is_some_and(|o| o.is_empty() || o.len() > 128 || o.contains(['/', '\\']))
                {
                    return Err("invalid bounded comment request roles".into());
                }
            }
            Call::CommentRetire(a) | Call::CommentRotate(a) => {
                relative_path(&a.path)?;
                if a.request.is_empty()
                    || a.request.len() > 128
                    || a.request.contains(['/', '\\'])
                    || a.rotations > 8
                {
                    return Err("invalid comment maintenance request".into());
                }
            }
            Call::CoreSend(a) => {
                if !f.inputs.contains_key(&a.input) {
                    return Err("missing command input".into());
                }
            }
            Call::AssertObserved(a)
                if a.observation >= action.id
                    || !f.oracles.contains_key(&a.oracle)
                    || a.id.is_empty()
                    || a.id.len() > 128 =>
            {
                return Err("invalid assertion observation/oracle".into());
            }
            _ => {}
        }
        previous = action.id;
    }
    Ok(())
}

/// Reject duplicate keys at every nesting depth, before deserializing actions.
pub fn unique_json(bytes: &[u8]) -> Result<Value, String> {
    struct Unique(Value);
    impl<'de> Deserialize<'de> for Unique {
        fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct JsonVisitor;
            impl<'de> Visitor<'de> for JsonVisitor {
                type Value = Unique;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("unique-key JSON")
                }
                fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Unique, M::Error> {
                    let mut values = serde_json::Map::new();
                    while let Some((key, value)) = map.next_entry::<String, Unique>()? {
                        if values.insert(key, value.0).is_some() {
                            return Err(de::Error::custom("duplicate JSON key"));
                        }
                    }
                    Ok(Unique(Value::Object(values)))
                }
                fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Unique, S::Error> {
                    let mut values = Vec::new();
                    while let Some(value) = seq.next_element::<Unique>()? {
                        values.push(value.0);
                    }
                    Ok(Unique(Value::Array(values)))
                }
                fn visit_bool<E: de::Error>(self, v: bool) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_i64<E: de::Error>(self, v: i64) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_u64<E: de::Error>(self, v: u64) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_f64<E: de::Error>(self, v: f64) -> Result<Unique, E> {
                    serde_json::Number::from_f64(v)
                        .map(|n| Unique(Value::Number(n)))
                        .ok_or_else(|| de::Error::custom("nonfinite number"))
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                    Ok(Unique(Value::Null))
                }
            }
            d.deserialize_any(JsonVisitor)
        }
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let result = Unique::deserialize(&mut decoder).map_err(|e| e.to_string())?;
    decoder.end().map_err(|e| e.to_string())?;
    Ok(result.0)
}

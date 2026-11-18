//! Prepare and inspect interchange timelines through one local I/O seam.

mod files;
mod resolve;
mod workers;

#[cfg(test)]
mod tests;

use std::{
    collections::BTreeSet,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use kinewright_core::{Analysis, Document, ImportRecord, LossCode, MediaAsset, Rational};
use kinewright_interchange::{
    MAX_INPUT_BYTES, fcpxml,
    ir::{Loss, ParseOptions, Timeline},
    locator::{LocatorFs, Platform, with_local},
    lower_with_sites, otio,
};
use serde::Serialize;

use self::{
    files::{DirectoryEntry, EntryKind, NativeLocatorFs},
    workers::{Ticket, Workers},
};
use crate::bundle::BundleWriteAdmission;

pub use kinewright_interchange::{Refusal, RefusalCode, SitePage, SiteQuery, locator::ShareRoot};

const MAX_MEDIA_ROOTS: usize = 8;
const MAX_ROOT_ENTRIES: usize = 100_000;
const MAX_ROOT_DEPTH: usize = 4;
const PREPARE_WORKERS: usize = 4;

#[derive(Debug, Clone, Copy)]
pub struct MediaDeadlines {
    pub stat: Duration,
    pub hash_stall: Duration,
    pub metadata: Duration,
}

impl Default for MediaDeadlines {
    fn default() -> Self {
        Self {
            stat: Duration::from_secs(2),
            hash_stall: Duration::from_secs(10),
            metadata: Duration::from_secs(10),
        }
    }
}

/// A handle for one preparation, shared by the UI and its import task.
#[derive(Default, Debug, Clone)]
pub struct PrepareControl(Arc<Progress>);

#[derive(Default, Debug)]
struct Progress {
    cancelled: AtomicBool,
    hashed: AtomicU64,
}

impl PrepareControl {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn hashed_bytes(&self) -> u64 {
        self.0.hashed.load(Ordering::Relaxed)
    }

    fn add_hashed(&self, bytes: u64) {
        self.0.hashed.fetch_add(bytes, Ordering::Relaxed);
    }
}

#[derive(Debug, Default, Clone)]
pub struct ImportOptions {
    pub timeline: Option<String>,
    pub project_fps: Option<Rational>,
    pub resolution: Option<(u32, u32)>,
    pub media_roots: Vec<String>,
    pub deadlines: MediaDeadlines,
    pub control: PrepareControl,
}

pub struct PrepareOptions {
    pub import: ImportOptions,
    /// Windows shares the person explicitly approved for this preparation.
    pub allowed_shares: BTreeSet<ShareRoot>,
    /// The same authorized media analysis used for ordinary single-file imports.
    pub analysis: Arc<dyn Analysis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStatus {
    Online,
    Unprobed,
    Missing,
    Ambiguous,
    InvalidLocator,
    Unreadable,
    Remote,
    TimedOut,
}

impl MediaStatus {
    #[must_use]
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Online | Self::Unprobed)
    }
}

#[derive(Debug)]
pub struct MediaEntry {
    pub name: String,
    pub locator: String,
    pub status: MediaStatus,
    pub resolved_path: Option<PathBuf>,
    pub relinked_by_name: bool,
    /// The search result is shared by locators with the same missing name.
    pub matches: Option<Arc<[PathBuf]>>,
}

impl MediaEntry {
    /// Identify the share associated with this row on systems that can open it.
    #[must_use]
    pub fn share_root(&self) -> Option<ShareRoot> {
        kinewright_interchange::locator::share_path(&self.locator, Platform::NATIVE)
            .map(|share| share.root)
    }
}

#[derive(Debug)]
pub struct PreparedImport {
    name: String,
    document: Document,
    record: ImportRecord,
    media: Vec<MediaEntry>,
    control: PrepareControl,
}

impl PreparedImport {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn document(&self) -> &Document {
        &self.document
    }

    #[must_use]
    pub const fn record(&self) -> &ImportRecord {
        &self.record
    }

    #[must_use]
    pub fn media(&self) -> &[MediaEntry] {
        &self.media
    }
}

#[derive(Debug, Serialize)]
pub struct ImportSummary {
    pub timeline: String,
    pub fps: Rational,
    pub resolution: (u32, u32),
}

#[derive(Debug)]
pub struct InterchangeInspection {
    pub summary: ImportSummary,
    pub record: ImportRecord,
    pub media: Vec<MediaEntry>,
    pub sites: SitePage,
}

/// Read, probe and lower one timeline in memory.
///
/// # Errors
/// Returns a typed refusal for an invalid source, options or cancelled import.
pub fn prepare(path: &str, options: PrepareOptions) -> Result<PreparedImport, Refusal> {
    let fs = Arc::new(NativeLocatorFs {
        analysis: Some(options.analysis),
    });
    prepare_with(
        fs,
        path,
        options.import,
        Preparation::Probe {
            allowed: &options.allowed_shares,
            platform: Platform::NATIVE,
        },
    )
    .map(|(prepared, _)| prepared)
}

/// Read, stat and lower one timeline for the agent. The main timeline bytes
/// supply the record's source digest. Media is never opened or hashed.
///
/// # Errors
/// Returns a typed refusal for an invalid source, options or cancelled inspection.
pub fn inspect(
    path: &str,
    options: ImportOptions,
    query: Option<SiteQuery>,
) -> Result<InterchangeInspection, Refusal> {
    inspect_with(
        Arc::new(NativeLocatorFs { analysis: None }),
        path,
        options,
        query,
    )
}

fn inspect_with(
    fs: Arc<impl ImportFs>,
    path: &str,
    options: ImportOptions,
    query: Option<SiteQuery>,
) -> Result<InterchangeInspection, Refusal> {
    let (prepared, sites) = prepare_with(fs, path, options, Preparation::Inspect(query))?;
    Ok(InterchangeInspection {
        summary: ImportSummary {
            timeline: prepared.name,
            fps: prepared.document.fps,
            resolution: prepared.document.resolution,
        },
        record: prepared.record,
        media: prepared.media,
        sites,
    })
}

#[derive(Debug)]
pub enum CommitImportError<E> {
    Cancelled,
    Session(E),
}

/// Pass the validated document to the app's untitled new-project constructor.
/// Callers supply no project path and publish the tab only on success.
///
/// # Errors
/// Returns cancellation or the application's session creation error.
pub fn commit<T, E>(
    prepared: PreparedImport,
    new_session: impl FnOnce(String, Document, BundleWriteAdmission) -> Result<T, E>,
) -> Result<T, CommitImportError<E>> {
    if prepared.control.is_cancelled() {
        return Err(CommitImportError::Cancelled);
    }
    new_session(
        prepared.name,
        prepared.document,
        BundleWriteAdmission::new_document(),
    )
    .map_err(CommitImportError::Session)
}

#[derive(Clone, Copy)]
enum Preparation<'a> {
    Probe {
        allowed: &'a BTreeSet<ShareRoot>,
        platform: Platform,
    },
    Inspect(Option<SiteQuery>),
}

impl Preparation<'_> {
    #[cfg(test)]
    const LOCAL: Self = Self::Probe {
        allowed: &BTreeSet::new(),
        platform: Platform::NATIVE,
    };

    const fn query(self) -> Option<SiteQuery> {
        match self {
            Self::Probe { .. } => None,
            Self::Inspect(query) => query,
        }
    }
}

trait ImportFs:
    LocatorFs<Metadata = EntryKind, Probe = MediaAsset, Entry = DirectoryEntry> + Send + Sync + 'static
{
}
impl<T> ImportFs for T where
    T: LocatorFs<Metadata = EntryKind, Probe = MediaAsset, Entry = DirectoryEntry>
        + Send
        + Sync
        + 'static
{
}

fn prepare_with(
    fs: Arc<impl ImportFs>,
    path: &str,
    options: ImportOptions,
    mode: Preparation<'_>,
) -> Result<(PreparedImport, SitePage), Refusal> {
    let path = local_path(path)?;
    if options.media_roots.len() > MAX_MEDIA_ROOTS {
        return Err(Refusal::new(
            RefusalCode::TooManyItems,
            "Supply at most eight media roots.",
        ));
    }
    let roots = options
        .media_roots
        .iter()
        .map(|root| local_path(root))
        .collect::<Result<_, _>>()?;
    if options.control.is_cancelled() {
        return Err(cancelled());
    }
    let source = SourcePath::new(&path)?;
    let mut workers = Workers::new(&options.control, options.deadlines);
    let mut timeline = read_timeline(&fs, &source, &options, &mut workers)?;
    let (media, table) = resolve::media(fs, &timeline, source.root(), roots, &mut workers, mode)?;
    let lowered = lower_with_sites(&timeline, &table, mode.query())?;
    if options.control.is_cancelled() {
        return Err(cancelled());
    }
    if timeline.name.is_empty() {
        timeline.name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Imported timeline")
            .into();
    }
    Ok((
        PreparedImport {
            name: timeline.name,
            document: lowered.document,
            record: lowered.record,
            media,
            control: options.control,
        },
        lowered.sites,
    ))
}

fn local_path(path: &str) -> Result<PathBuf, Refusal> {
    with_local(path, Platform::NATIVE, |path| PathBuf::from(path.as_str()))
}

fn cancelled() -> Refusal {
    Refusal::new(
        RefusalCode::Cancelled,
        "The timeline preparation was cancelled.",
    )
}

struct SourcePath {
    main: PathBuf,
    bundle: Option<Bundle>,
}

#[derive(Clone, Copy)]
enum Bundle {
    Otio,
    Fcpxml,
}

impl SourcePath {
    fn new(path: &Path) -> Result<Self, Refusal> {
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default();
        let (main, bundle) = if ext.eq_ignore_ascii_case("otioz") {
            return Err(Refusal::new(
                RefusalCode::UnsupportedContainer,
                "OTIO archives are outside the supported containers.",
            ));
        } else if ext.eq_ignore_ascii_case("otiod") {
            (path.join("content.otio"), Some(Bundle::Otio))
        } else if ext.eq_ignore_ascii_case("fcpxmld") {
            (path.join("Info.fcpxml"), Some(Bundle::Fcpxml))
        } else {
            let parent_ext = path
                .parent()
                .and_then(Path::extension)
                .and_then(|ext| ext.to_str());
            let bundle = parent_ext
                .filter(|ext| ext.eq_ignore_ascii_case("fcpxmld"))
                .map(|_| Bundle::Fcpxml);
            (path.to_owned(), bundle)
        };
        Ok(Self { main, bundle })
    }

    fn root(&self) -> &Path {
        self.main
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }
}

fn read_timeline(
    fs: &Arc<impl ImportFs>,
    source: &SourcePath,
    options: &ImportOptions,
    workers: &mut Workers<'_>,
) -> Result<Timeline, Refusal> {
    let input_fs = Arc::clone(fs);
    let bytes = workers.io(
        source.main.clone(),
        "The timeline file did not respond.",
        move |path, ticket| read_main(&*input_fs, &path, ticket),
    )?;
    let name = source
        .main
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let parse = ParseOptions {
        timeline: options.timeline.as_deref(),
        project_fps: options.project_fps,
        resolution: options.resolution,
    };
    let first = bytes
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(&bytes)
        .iter()
        .find(|byte| !byte.is_ascii_whitespace());
    let mut timeline = match (source.bundle, first) {
        (Some(Bundle::Otio), _) | (None, Some(b'{' | b'[')) => otio::parse(&bytes, name, parse)?,
        (Some(Bundle::Fcpxml), _) | (None, Some(b'<')) => fcpxml::parse(&bytes, name, parse)?,
        (None, _) if name.to_ascii_lowercase().ends_with(".otio") => {
            otio::parse(&bytes, name, parse)?
        }
        (None, _) if name.to_ascii_lowercase().ends_with(".fcpxml") => {
            fcpxml::parse(&bytes, name, parse)?
        }
        (None, _) => {
            return Err(Refusal::new(
                RefusalCode::UnsupportedContainer,
                "The source is neither OTIO nor FCPXML.",
            ));
        }
    };
    if matches!(source.bundle, Some(Bundle::Fcpxml)) {
        note_sidecars(fs, source, &mut timeline, workers)?;
    }
    Ok(timeline)
}

fn note_sidecars(
    fs: &Arc<impl ImportFs>,
    source: &SourcePath,
    timeline: &mut Timeline,
    workers: &mut Workers<'_>,
) -> Result<(), Refusal> {
    let media: BTreeSet<_> = timeline
        .media
        .iter()
        .filter_map(|media| {
            let path = local_path(&media.locator).ok()?;
            let relative = if path.is_absolute() {
                path.strip_prefix(source.root()).ok()?
            } else {
                &path
            };
            let mut components = relative.components();
            components.find_map(|part| match part {
                std::path::Component::Normal(name) => Some(name.to_owned()),
                _ => None,
            })
        })
        .collect();
    let directory_fs = Arc::clone(fs);
    let main = source.main.file_name().map(ToOwned::to_owned);
    let sidecars = workers.io(
        source.root().to_owned(),
        "The bundle directory did not respond.",
        move |path, ticket| scan_sidecars(&*directory_fs, &path, main.as_deref(), &media, ticket),
    )?;
    timeline.notes.extend(
        sidecars
            .iter()
            .map(|name| Loss::dropped(LossCode::MetadataDropped).named(name)),
    );
    Ok(())
}

fn scan_sidecars(
    fs: &impl ImportFs,
    root: &Path,
    main: Option<&std::ffi::OsStr>,
    media: &BTreeSet<std::ffi::OsString>,
    ticket: &Ticket,
) -> Result<Vec<String>, Refusal> {
    let entries = fs.read_dir(root);
    io_progress(ticket)?;
    let Ok(entries) = entries else {
        return Ok(Vec::new());
    };
    let mut sidecars = Vec::new();
    for entry in entries.take(MAX_ROOT_ENTRIES) {
        io_progress(ticket)?;
        if let Ok(entry) = entry
            && let Some(name) = entry.path.file_name()
            && Some(name) != main
            && !media.contains(name)
        {
            sidecars.push(name.to_string_lossy().into_owned());
        }
    }
    sidecars.sort_unstable();
    Ok(sidecars)
}

fn io_progress(ticket: &Ticket) -> Result<(), Refusal> {
    if ticket.note_io_progress() {
        Ok(())
    } else {
        Err(cancelled())
    }
}

fn read_main(fs: &impl ImportFs, path: &Path, ticket: &Ticket) -> Result<Vec<u8>, Refusal> {
    let opened = fs.open(path);
    io_progress(ticket)?;
    let mut reader = opened
        .map_err(|error| source_refusal(&error))?
        .take(MAX_INPUT_BYTES as u64 + 1);
    let mut input = Vec::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer);
        io_progress(ticket)?;
        let count = read.map_err(|error| source_refusal(&error))?;
        if count == 0 {
            break;
        }
        if input.len() + count > MAX_INPUT_BYTES {
            return Err(Refusal::new(
                RefusalCode::TooLarge,
                "The timeline file exceeds 128 MiB.",
            ));
        }
        input.extend_from_slice(&buffer[..count]);
    }
    Ok(input)
}

fn source_refusal(error: &io::Error) -> Refusal {
    match error.kind() {
        io::ErrorKind::NotFound => {
            Refusal::new(RefusalCode::NotFound, "The timeline file was not found.")
        }
        _ => Refusal::new(
            RefusalCode::Malformed,
            "The timeline file could not be read.",
        ),
    }
}

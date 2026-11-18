use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use kinewright_core::MediaAsset;
use kinewright_interchange::{
    Availability, MediaTable, ProbedMedia,
    ir::{Media, Timeline},
    locator::{Locator, Platform, classify, share_path},
};

use super::{
    ImportFs, MAX_ROOT_DEPTH, MAX_ROOT_ENTRIES, MediaEntry, MediaStatus, Preparation, Refusal,
    files::EntryKind,
    workers::{Phase, Ticket, Workers},
};

pub(super) fn media(
    fs: Arc<impl ImportFs>,
    timeline: &Timeline,
    base: &Path,
    roots: Vec<PathBuf>,
    workers: &mut Workers<'_>,
    mode: Preparation<'_>,
) -> Result<(Vec<MediaEntry>, MediaTable), Refusal> {
    let (mut entries, paths) = initial_entries(&timeline.media, base, mode);
    let fs_for_stat = Arc::clone(&fs);
    let stats = workers.run(Phase::Stat, paths.paths, move |path, _| {
        fs_for_stat.stat(&path)
    })?;
    for (index, stat) in paths.indices.into_iter().zip(stats) {
        entries[index].status = match stat {
            Some(Ok(EntryKind::File)) => MediaStatus::Unprobed,
            Some(Ok(_)) => MediaStatus::Missing,
            Some(Err(error)) if error.kind() == io::ErrorKind::NotFound => MediaStatus::Missing,
            Some(Err(_)) => MediaStatus::Unreadable,
            None => MediaStatus::TimedOut,
        };
    }
    search_roots(&fs, workers, &mut entries, roots)?;
    let mut table = MediaTable {
        entries: entries.iter().map(availability).collect(),
    };
    if matches!(mode, Preparation::Probe { .. }) {
        probe(fs, workers, &mut entries, &mut table)?;
    }
    for entry in &mut entries {
        if !entry.status.is_available() {
            entry.resolved_path = None;
        }
    }
    Ok((entries, table))
}

#[derive(Default)]
struct Paths {
    paths: Vec<PathBuf>,
    indices: Vec<usize>,
}

impl Paths {
    fn add(&mut self, index: usize, path: &Path) {
        self.paths.push(path.to_owned());
        self.indices.push(index);
    }
}

fn initial_entries(
    media: &[Media],
    base: &Path,
    mode: Preparation<'_>,
) -> (Vec<MediaEntry>, Paths) {
    let mut paths = Paths::default();
    let entries = media
        .iter()
        .enumerate()
        .map(|(index, media)| {
            let (status, resolved_path) = match classify(&media.locator, Platform::NATIVE) {
                Locator::Local(path) => {
                    let path = if path.is_absolute() {
                        PathBuf::from(path.as_str())
                    } else {
                        base.join(path.as_str())
                    };
                    paths.add(index, &path);
                    (MediaStatus::Missing, Some(path))
                }
                Locator::Remote => approved_share_path(&media.locator, mode).map_or(
                    (MediaStatus::Remote, None),
                    |path| {
                        paths.add(index, &path);
                        (MediaStatus::Missing, Some(path))
                    },
                ),
                Locator::Invalid if media.locator.is_empty() => (MediaStatus::Missing, None),
                Locator::Invalid => (MediaStatus::InvalidLocator, None),
            };
            let name = if media.name.is_empty() {
                "Unidentified media".to_owned()
            } else {
                media.name.clone()
            };
            MediaEntry {
                name,
                locator: media.locator.clone(),
                status,
                resolved_path,
                relinked_by_name: false,
                matches: None,
            }
        })
        .collect();
    (entries, paths)
}

fn approved_share_path(locator: &str, mode: Preparation<'_>) -> Option<PathBuf> {
    let Preparation::Probe { allowed, platform } = mode else {
        return None;
    };
    let share = share_path(locator, platform)?;
    allowed
        .contains(&share.root)
        .then(|| PathBuf::from(share.path))
}

fn availability(entry: &MediaEntry) -> Availability {
    match (entry.status, &entry.resolved_path) {
        (MediaStatus::Unprobed, Some(path)) => Availability::Unprobed {
            path: path.to_owned(),
            relinked_by_name: entry.relinked_by_name,
        },
        (MediaStatus::Remote, _) => Availability::Remote,
        (MediaStatus::TimedOut, _) => Availability::TimedOut,
        _ => Availability::Unavailable,
    }
}

fn probe(
    fs: Arc<impl ImportFs>,
    workers: &mut Workers<'_>,
    entries: &mut [MediaEntry],
    table: &mut MediaTable,
) -> Result<(), Refusal> {
    let mut paths = Paths::default();
    for (index, entry) in entries.iter().enumerate() {
        if let Some(path) = &entry.resolved_path
            && entry.status == MediaStatus::Unprobed
        {
            paths.add(index, path);
        }
    }
    let probes = workers.run(Phase::Probe, paths.paths, move |path, ticket| {
        fs.probe(&path, ticket)
    })?;
    for (index, result) in paths.indices.into_iter().zip(probes) {
        let entry = &mut entries[index];
        let availability = match result {
            Some(Ok(asset)) if asset.source_fingerprint.is_verified() => {
                entry.status = MediaStatus::Online;
                Availability::Probed(Box::new(probed(asset, entry.relinked_by_name)))
            }
            Some(_) => {
                entry.status = MediaStatus::Unreadable;
                Availability::Unavailable
            }
            None => {
                entry.status = MediaStatus::TimedOut;
                Availability::TimedOut
            }
        };
        table.entries[index] = availability;
    }
    Ok(())
}

fn probed(asset: MediaAsset, relinked_by_name: bool) -> ProbedMedia {
    ProbedMedia {
        path: asset.path,
        kind: asset.kind,
        fps: asset.fps,
        duration: asset.duration,
        resolution: asset.resolution,
        source_fingerprint: asset.source_fingerprint,
        color_description: asset.color_description,
        relinked_by_name,
    }
}

fn missing_name(entry: &MediaEntry) -> Option<&std::ffi::OsStr> {
    (entry.status == MediaStatus::Missing)
        .then_some(entry.resolved_path.as_deref())??
        .file_name()
}

type Matches = BTreeMap<OsString, BTreeSet<PathBuf>>;

fn search_roots(
    fs: &Arc<impl ImportFs>,
    workers: &mut Workers<'_>,
    entries: &mut [MediaEntry],
    roots: Vec<PathBuf>,
) -> Result<(), Refusal> {
    if roots.is_empty() {
        return Ok(());
    }
    let wanted: BTreeSet<_> = entries
        .iter()
        .filter_map(missing_name)
        .map(ToOwned::to_owned)
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let fs_for_search = Arc::clone(fs);
    let roots: BTreeSet<_> = roots.into_iter().collect();
    let results = workers.run(
        Phase::Stat,
        roots.into_iter().collect(),
        move |root, ticket| walk(&*fs_for_search, root, &wanted, ticket),
    )?;
    let timed_out = results.iter().any(Option::is_none);
    let complete = results
        .iter()
        .all(|result| result.as_ref().is_some_and(|(_, complete)| *complete));
    let mut index = Matches::new();
    for (matches, _) in results.into_iter().flatten() {
        for (name, paths) in matches {
            index.entry(name).or_default().extend(paths);
        }
    }
    let index: BTreeMap<_, Arc<[PathBuf]>> = index
        .into_iter()
        .map(|(name, paths)| (name, paths.into_iter().collect()))
        .collect();
    for entry in entries {
        let Some(name) = missing_name(entry) else {
            continue;
        };
        if let Some(matches) = index.get(name) {
            entry.matches = Some(Arc::clone(matches));
            match &**matches {
                [unique] if complete => {
                    entry.status = MediaStatus::Unprobed;
                    entry.relinked_by_name = true;
                    entry.resolved_path = Some(unique.clone());
                    continue;
                }
                [_, _, ..] => {
                    entry.status = MediaStatus::Ambiguous;
                    continue;
                }
                _ => {}
            }
        }
        if timed_out {
            entry.status = MediaStatus::TimedOut;
        }
    }
    Ok(())
}

fn walk(
    fs: &impl ImportFs,
    root: PathBuf,
    wanted: &BTreeSet<OsString>,
    ticket: &Ticket,
) -> (Matches, bool) {
    let mut names = Matches::new();
    let mut directories = vec![(root, 0)];
    let mut count = 0;
    let mut complete = true;
    while let Some((path, depth)) = directories.pop() {
        if !ticket.note_io_progress() {
            return (names, false);
        }
        let entries = fs.read_dir(&path);
        if !ticket.note_io_progress() {
            return (names, false);
        }
        let Ok(entries) = entries else {
            complete = false;
            continue;
        };
        for entry in entries {
            if count == MAX_ROOT_ENTRIES || !ticket.note_io_progress() {
                return (names, false);
            }
            count += 1;
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            match entry.kind {
                EntryKind::Directory if depth + 1 < MAX_ROOT_DEPTH => {
                    directories.push((entry.path, depth + 1));
                }
                EntryKind::File => {
                    if let Some(name) = entry.path.file_name()
                        && wanted.contains(name)
                    {
                        names.entry(name.to_owned()).or_default().insert(entry.path);
                    }
                }
                _ => {}
            }
        }
    }
    (names, complete)
}

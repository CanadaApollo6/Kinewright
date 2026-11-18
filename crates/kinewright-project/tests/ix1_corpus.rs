use kinewright_core::InterchangeFormat;
use kinewright_project::interchange::{ImportOptions, RefusalCode, inspect};

#[test]
fn imported_timeline_corpus_prints_item_and_media_counts() {
    let Some(root) = std::env::var_os("KINEWRIGHT_IX_CORPUS") else {
        println!("KINEWRIGHT_IX_CORPUS unset, external corpus skipped");
        return;
    };
    let mut files = Vec::new();
    let mut folders = vec![std::path::PathBuf::from(root)];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            match (extension.as_str(), kind.is_dir(), kind.is_file()) {
                ("otiod" | "fcpxmld", true, _) | ("otio" | "fcpxml", _, true) => files.push(path),
                (_, true, _) => folders.push(path),
                _ => {}
            }
        }
    }
    files.sort_unstable();
    println!(
        "file\ttimeline\tformat\titems\texact\tapproximated\tdropped\tassets\tremote\ttimed_out"
    );
    let mut counts = [0; 2];
    for path in files {
        let name = path.to_str().expect("corpus path is UTF-8");
        let first = inspect(name, ImportOptions::default(), None);
        let reports = match first {
            Err(refusal) if refusal.code == RefusalCode::AmbiguousTimeline => refusal
                .timelines
                .into_iter()
                .map(|timeline| {
                    inspect(
                        name,
                        ImportOptions {
                            timeline: Some(timeline),
                            ..ImportOptions::default()
                        },
                        None,
                    )
                })
                .collect(),
            report => vec![report],
        };
        for report in reports {
            match report {
                Ok(report) => {
                    let record = report.record;
                    record.validate().unwrap();
                    let index = usize::from(record.format == InterchangeFormat::Fcpxml);
                    counts[index] += 1;
                    println!(
                        "{}\t{}\t{:?}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        record.source_name,
                        report.summary.timeline,
                        record.format,
                        record.items,
                        record.exact,
                        record.approximated,
                        record.dropped,
                        record.media.assets,
                        record.media.remote,
                        record.media.timed_out,
                    );
                }
                Err(refusal) => println!("{name:?}\t{:?}\t{}", refusal.code, refusal.message),
            }
        }
    }
    println!("counts\t{counts:?}");
    assert!(
        counts[0] > 0 && counts[1] > 0 && counts.iter().sum::<u32>() >= 20,
        "{counts:?}"
    );
}

//! Release-only IX1 timing and allocation witness. Run one case per process.
use std::{fmt::Write as _, path::Path, sync::Arc, time::Instant};

use kinewright_core::RefusedMedia;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::interchange::{ImportOptions, PrepareOptions, prepare};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let format = arguments.next().expect("specify otio, fcpxml, or validate");
    let clips = arguments
        .next()
        .map_or(10_000, |value| value.parse().expect("clip count"));
    assert!([1_000, 10_000, 50_000].contains(&clips));
    assert!(arguments.next().is_none());
    let peak_pin = match (format.as_str(), clips) {
        ("otio", 1_000) => 7_622_283,
        ("otio", 10_000) => 73_710_528,
        ("otio", 50_000) => 305_313_143,
        ("fcpxml", 1_000) => 6_882_275,
        ("fcpxml", 10_000) => 62_050_915,
        ("fcpxml", 50_000) => 257_348_545,
        ("validate", 10_000) => 322_685,
        _ => panic!("choose a supported format and clip count"),
    };
    let validate = format == "validate";
    let assets = if validate { 2_000 } else { 32 };
    let padding = if clips == 50_000 { 2_200 } else { 2_600 };
    let (extension, fixture) = match format.as_str() {
        "otio" => ("otio", otio(clips, assets, padding)),
        "fcpxml" | "validate" => ("fcpxml", fcpxml(clips, assets, padding)),
        _ => panic!("specify otio, fcpxml, or validate"),
    };
    let directory = TempDirectory::new("ix1-limits");
    let path = directory.path(&format!("timeline.{extension}"));
    let size = fixture.len();
    std::fs::write(&path, fixture).unwrap();
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|text| text.split_ascii_whitespace().next().map(str::to_owned));
    let (elapsed, heap) = if validate {
        let prepared = prepare_fixture(&path);
        assert_eq!(prepared.document().media_pool.len(), assets);
        assert_eq!(
            prepared
                .document()
                .tracks
                .iter()
                .map(|track| track.clips.len())
                .sum::<usize>(),
            clips
        );
        let _profiler = dhat::Profiler::builder().testing().build();
        let started = Instant::now();
        prepared.document().validate().unwrap();
        (started.elapsed(), dhat::HeapStats::get().max_bytes)
    } else {
        let _profiler = dhat::Profiler::builder().testing().build();
        let started = Instant::now();
        let prepared = prepare_fixture(&path);
        let elapsed = started.elapsed();
        let heap = dhat::HeapStats::get().max_bytes;
        assert_eq!(
            prepared
                .document()
                .tracks
                .iter()
                .map(|track| track.clips.len())
                .sum::<usize>(),
            clips
        );
        assert_eq!(prepared.record().media.online, 0);
        (elapsed, heap)
    };
    assert!(
        heap <= peak_pin,
        "{format} {clips}: peak heap {heap} B exceeds {peak_pin} B"
    );
    println!(
        "{format}\t{clips}\t{assets}\t{size}\t{:.3}\t{heap}\t{}",
        elapsed.as_secs_f64() * 1000.0,
        load.as_deref().unwrap_or("unavailable"),
    );
}

fn prepare_fixture(path: &Path) -> kinewright_project::interchange::PreparedImport {
    prepare(
        path.to_str().unwrap(),
        PrepareOptions {
            import: ImportOptions::default(),
            analysis: Arc::new(RefusedMedia),
        },
    )
    .unwrap()
}

fn otio(clips: usize, assets: usize, padding: usize) -> String {
    let metadata = "x".repeat(padding);
    let mut xml = String::new();
    xml.push_str(r#"{"OTIO_SCHEMA":"Timeline.1","name":"Limits","global_start_time":{"value":0,"rate":24},"tracks":{"OTIO_SCHEMA":"Stack.1","children":[{"OTIO_SCHEMA":"Track.1","kind":"Video","children":["#);
    for clip in 0..clips {
        if clip > 0 {
            xml.push(',');
        }
        let asset = clip % assets;
        write!(xml, r#"{{"OTIO_SCHEMA":"Clip.1","name":"Clip {clip}","metadata":{{"notes":"{metadata}"}},"source_range":{{"start_time":{{"value":0,"rate":24}},"duration":{{"value":24,"rate":24}}}},"media_reference":{{"OTIO_SCHEMA":"ExternalReference.1","target_url":"https://media.invalid/{asset}.mov","available_range":{{"start_time":{{"value":0,"rate":24}},"duration":{{"value":24,"rate":24}}}}}}}}"#).unwrap();
    }
    xml.push_str("]}]}}");
    xml
}

fn fcpxml(clips: usize, assets: usize, padding: usize) -> String {
    let metadata = "x".repeat(padding);
    let mut xml = String::new();
    xml.push_str(r#"<fcpxml version="1.13"><resources><format id="f" frameDuration="1/24s" width="1920" height="1080"/>"#);
    for asset in 0..assets {
        write!(xml, r#"<asset id="a{asset}" start="0s" duration="1s" hasVideo="1" format="f"><media-rep kind="original-media" src="https://media.invalid/{asset}.mov"/></asset>"#).unwrap();
    }
    xml.push_str(
        r#"</resources><library><event><project name="Limits"><sequence format="f"><spine>"#,
    );
    for clip in 0..clips {
        let asset = clip % assets;
        write!(xml, r#"<asset-clip ref="a{asset}" name="Clip {clip}" offset="{clip}s" duration="1s"><note>{metadata}</note></asset-clip>"#).unwrap();
    }
    xml.push_str("</spine></sequence></project></event></library></fcpxml>");
    xml
}

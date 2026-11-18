use std::{fs, path::Path};

use kinewright_interchange::{MediaTable, fcpxml, ir::ParseOptions, lower, otio};
use serde::Serialize;

#[path = "support/interchange_opaque.rs"]
mod opaque;

#[test]
fn authored_fixtures_match_document_and_report_goldens() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut inputs: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| matches!(ext, "otio" | "fcpxml"))
        })
        .collect();
    inputs.sort_unstable();
    assert_eq!(inputs.len(), 46);
    let mut opaque_frames = 0;
    for input in inputs {
        let name = input.file_name().unwrap().to_str().unwrap();
        let bytes = fs::read(&input).unwrap();
        let parser = if input
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("otio"))
        {
            otio::parse
        } else {
            fcpxml::parse
        };
        let timeline = parser(&bytes, name, ParseOptions::default())
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let (document, record) = lower(&timeline, &MediaTable::default())
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(document.validate(), Ok(()), "{name}");
        assert_eq!(record.validate(), Ok(()), "{name}");
        assert_eq!(
            u64::from(record.exact) + u64::from(record.approximated) + u64::from(record.dropped),
            u64::from(record.items),
            "{name}"
        );
        let (again_document, again_record) = lower(&timeline, &MediaTable::default()).unwrap();
        assert_eq!(
            (&document, &record),
            (&again_document, &again_record),
            "{name}"
        );
        opaque_frames += opaque::check(&timeline, &document, &record, name);
        assert_golden(&input.with_extension("document.json"), &document);
        assert_golden(&input.with_extension("record.json"), &record);
    }
    assert!(
        opaque_frames > 500,
        "only {opaque_frames} opaque frames verified"
    );
}

fn assert_golden(path: &Path, actual: &impl Serialize) {
    let mut actual = serde_json::to_vec_pretty(actual).unwrap();
    actual.push(b'\n');
    let expected = fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert_eq!(expected, actual, "{}", path.display());
}

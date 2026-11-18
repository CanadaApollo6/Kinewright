#![no_main]

use kinewright_interchange::{MediaTable, RefusalCode, ir::ParseOptions, lower, otio};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(timeline) = otio::parse(bytes, "fuzz.otio", ParseOptions::default()) {
        match lower(&timeline, &MediaTable::default()) {
            Ok((document, record)) => {
                assert_eq!(document.validate(), Ok(()));
                assert_eq!(record.validate(), Ok(()));
            }
            Err(error) => assert_ne!(error.code, RefusalCode::LoweringFailed, "{error:?}"),
        }
    }
});

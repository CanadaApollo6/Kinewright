#![no_main]

use kinewright_interchange::locator::{Locator, Platform, classify, with_local};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let Ok(raw) = std::str::from_utf8(bytes) else {
        return;
    };
    for platform in [Platform::Unix, Platform::Windows] {
        let result = classify(raw, platform);
        let checked = with_local(raw, platform, |local| local);
        if let Locator::Local(path) = result {
            assert_eq!(checked.as_ref().ok(), Some(&path));
            assert!(!path.as_str().contains('\0'));
            assert!(!path.as_str().split(['/', '\\']).any(|part| part == ".."));
        } else {
            assert!(checked.is_err());
        }
    }
});

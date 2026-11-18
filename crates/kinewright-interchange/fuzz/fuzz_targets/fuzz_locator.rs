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
            if platform == Platform::Windows {
                assert!(!matches!(path.as_str().as_bytes(), [letter, b':', ..]
                    if letter.is_ascii_alphabetic() && !path.is_absolute()));
                for part in path.as_str().split(['/', '\\']) {
                    assert!(!part.ends_with(' ') && !part.ends_with('.'));
                    let stem = part.split('.').next().unwrap().trim_end_matches(' ');
                    assert!(!matches!(
                        stem.to_ascii_uppercase().as_str(),
                        "CON"
                            | "PRN"
                            | "AUX"
                            | "NUL"
                            | "CONIN$"
                            | "CONOUT$"
                            | "COM1"
                            | "COM2"
                            | "COM3"
                            | "COM4"
                            | "COM5"
                            | "COM6"
                            | "COM7"
                            | "COM8"
                            | "COM9"
                            | "LPT1"
                            | "LPT2"
                            | "LPT3"
                            | "LPT4"
                            | "LPT5"
                            | "LPT6"
                            | "LPT7"
                            | "LPT8"
                            | "LPT9"
                            | "COM¹"
                            | "COM²"
                            | "COM³"
                            | "LPT¹"
                            | "LPT²"
                            | "LPT³"
                    ));
                }
            }
        } else {
            assert!(checked.is_err());
        }
    }
});

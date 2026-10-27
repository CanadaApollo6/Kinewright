#[cfg(target_arch = "x86_64")]
use super::INITIAL_STATE;
use super::{Sha256, sha256_bytes, sha256_reader};

fn digest(input: &[u8], software: bool) -> [u8; 32] {
    let mut hasher = if software {
        Sha256::new_software()
    } else {
        Sha256::new()
    };
    hasher.update(input);
    hasher.finalize()
}

fn hex(digest: [u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(encoded, "{byte:02x}").unwrap();
    }
    encoded
}

fn random_bytes(length: usize, seed: &mut u64) -> Vec<u8> {
    (0..length)
        .map(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            seed.to_le_bytes()[0]
        })
        .collect()
}

#[test]
fn s2h_sha256_nist_vectors_dispatch_and_forced_software() {
    // NIST SHA-256 examples and FIPS 180 test messages. These include both
    // padding-block cases and the million-byte streaming example.
    let vectors: &[(&[u8], &str)] = &[
        (
            b"",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        (
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmn\
              hijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
        ),
        (
            &vec![b'a'; 1_000_000],
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
        ),
    ];
    for &(input, expected) in vectors {
        for software in [false, true] {
            assert_eq!(hex(digest(input, software)), expected);
            let mut hasher = if software {
                Sha256::new_software()
            } else {
                Sha256::new()
            };
            for chunk in input.chunks(63) {
                hasher.update(chunk);
            }
            assert_eq!(hex(hasher.finalize()), expected);
        }
        assert_eq!(sha256_bytes(input), expected);
        let mut reader = input;
        assert_eq!(sha256_reader(&mut reader).unwrap(), expected);
    }
}

#[test]
fn s2h_sha256_dispatch_uses_hardware_when_available_and_force_is_local() {
    let input = [0xab; 128];
    let mut automatic = Sha256::new();
    let mut software = Sha256::new_software();
    automatic.update(&input);
    software.update(&input);
    assert_eq!(software.hardware_blocks, 0);
    #[cfg(target_arch = "x86_64")]
    assert_eq!(
        automatic.hardware_blocks,
        if std::is_x86_feature_detected!("sha") {
            2
        } else {
            0
        }
    );
    #[cfg(not(target_arch = "x86_64"))]
    assert_eq!(automatic.hardware_blocks, 0);
    assert_eq!(automatic.finalize(), software.finalize());
}

#[test]
fn s2h_sha256_random_lengths_through_10kib() {
    let mut seed = 0x21c8_936e_e12f_27a5;
    for length in 0..=10 * 1024 {
        let input = random_bytes(length, &mut seed);
        assert_eq!(
            digest(&input, false),
            digest(&input, true),
            "length {length}"
        );
    }
}

#[test]
fn s2h_sha256_multi_mib_matches_software() {
    let mut seed = 0x8a71_a950_5c01_d6bd;
    for length in [2 * 1024 * 1024, 3 * 1024 * 1024 + 55, 5 * 1024 * 1024 + 65] {
        let input = random_bytes(length, &mut seed);
        let expected = digest(&input, true);
        assert_eq!(digest(&input, false), expected, "length {length}");
        for software in [false, true] {
            let mut hasher = if software {
                Sha256::new_software()
            } else {
                Sha256::new()
            };
            for chunk in input.chunks(64 * 1024 - 1) {
                hasher.update(chunk);
                hasher.update(&[]);
            }
            assert_eq!(hasher.finalize(), expected, "length {length}");
        }
    }
}

#[test]
fn s2h_sha256_chunking_and_update_boundaries() {
    let mut seed = 0xa975_9ef7_03cf_591b;
    let storage = random_bytes(10 * 1024 + 1, &mut seed);
    let input = &storage[1..];
    for length in [
        0,
        1,
        2,
        55,
        56,
        57,
        63,
        64,
        65,
        119,
        120,
        121,
        127,
        128,
        129,
        191,
        192,
        193,
        255,
        256,
        257,
        1023,
        1024,
        1025,
        10 * 1024,
    ] {
        let input = &input[..length];
        let expected = digest(input, true);
        for software in [false, true] {
            // Every split near the first four blocks, plus both endpoints for
            // longer messages, covers partial buffers and padding boundaries.
            for split in (0..=length.min(257)).chain(std::iter::once(length)) {
                let mut hasher = if software {
                    Sha256::new_software()
                } else {
                    Sha256::new()
                };
                hasher.update(&[]);
                hasher.update(&input[..split]);
                hasher.update(&[]);
                hasher.update(&input[split..]);
                hasher.update(&[]);
                assert_eq!(
                    hasher.finalize(),
                    expected,
                    "length {length}, split {split}"
                );
            }
            for size in [1, 2, 3, 7, 31, 55, 56, 57, 63, 64, 65, 127, 128, 129, 1024] {
                let mut hasher = if software {
                    Sha256::new_software()
                } else {
                    Sha256::new()
                };
                for chunk in input.chunks(size) {
                    hasher.update(chunk);
                    hasher.update(&[]);
                }
                assert_eq!(
                    hasher.finalize(),
                    expected,
                    "length {length}, chunks {size}"
                );
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[test]
fn s2h_sha_ni_rounds_match_software() {
    if !std::is_x86_feature_detected!("sha") {
        eprintln!("NOT RUN: SHA-NI compression comparison requires CPU SHA support");
        return;
    }
    let mut seed = 0x719a_83c5_ef1d_4b06;
    for count in 1..=33 {
        let input = random_bytes(count * 64, &mut seed);
        let blocks = input.as_chunks::<64>().0;
        // Compare arbitrary chaining values as well as the standard IV.
        for initial in [
            INITIAL_STATE,
            std::array::from_fn(|_| {
                let bytes = random_bytes(4, &mut seed);
                u32::from_le_bytes(bytes.try_into().unwrap())
            }),
        ] {
            let mut hardware = initial;
            assert!(super::x86::try_compress(&mut hardware, blocks));
            let mut software = Sha256::new_software();
            software.state = initial;
            software.update(&input);
            assert_eq!(hardware, software.state, "{count} blocks");
            for block in blocks {
                let mut chunked = initial;
                assert!(super::x86::try_compress(&mut chunked, &[*block]));
                let mut software = Sha256::new_software();
                software.state = initial;
                software.update(block);
                assert_eq!(chunked, software.state);
            }
        }
    }
}

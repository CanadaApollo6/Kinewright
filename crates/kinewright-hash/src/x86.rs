//! SHA-NI compression for `x86_64` on both Windows MSVC and Unix targets.
//!
//! Only SHA and baseline `x86_64` SSE2 instructions are used. No global target
//! features, SSSE3, SSE4.1, raw pointers, or aligned memory accesses are required.
use std::arch::x86_64::{
    __m128i, _mm_add_epi32, _mm_cvtsi128_si32, _mm_or_si128, _mm_setr_epi32, _mm_sha256msg1_epu32,
    _mm_sha256msg2_epu32, _mm_sha256rnds2_epu32, _mm_shuffle_epi32, _mm_slli_si128, _mm_srli_si128,
};

use super::ROUND_CONSTANTS;

#[allow(unsafe_code)]
pub(super) fn try_compress(state: &mut [u32; 8], blocks: &[[u8; 64]]) -> bool {
    if !std::is_x86_feature_detected!("sha") {
        return false;
    }
    // SAFETY: the runtime check above gates the sole call to the SHA-enabled
    // intrinsic kernel. SSE2 is baseline on x86_64. All memory access inside
    // the kernel uses safe references, slices and integer conversions.
    unsafe { compress(state, blocks) };
    true
}

#[allow(unsafe_code)] // A target-feature function may only be called on supported CPUs.
#[target_feature(enable = "sha")]
fn compress(state: &mut [u32; 8], blocks: &[[u8; 64]]) {
    let signed = |word: u32| i32::from_ne_bytes(word.to_ne_bytes());
    // Intrinsic lanes, from low to high, are F,E,B,A and H,G,D,C.
    let mut abef = _mm_setr_epi32(
        signed(state[5]),
        signed(state[4]),
        signed(state[1]),
        signed(state[0]),
    );
    let mut cdgh = _mm_setr_epi32(
        signed(state[7]),
        signed(state[6]),
        signed(state[3]),
        signed(state[2]),
    );

    for block in blocks {
        let saved_abef = abef;
        let saved_cdgh = cdgh;
        let mut messages = [_mm_setr_epi32(0, 0, 0, 0); 4];
        for (message, bytes) in messages.iter_mut().zip(block.as_chunks::<16>().0) {
            let words = bytes.as_chunks::<4>().0;
            *message = _mm_setr_epi32(
                i32::from_be_bytes(words[0]),
                i32::from_be_bytes(words[1]),
                i32::from_be_bytes(words[2]),
                i32::from_be_bytes(words[3]),
            );
        }

        for group in 0..16 {
            let slot = group % 4;
            if group >= 4 {
                // Four-word ring: W[t-16] + sigma0(W[t-15]), then W[t-7],
                // then sigma1(W[t-2]). MSG2 also resolves the two new words
                // needed by the last two lanes of this same group.
                let partial = _mm_sha256msg1_epu32(messages[slot], messages[(slot + 1) % 4]);
                let middle = _mm_or_si128(
                    _mm_srli_si128::<4>(messages[(slot + 2) % 4]),
                    _mm_slli_si128::<12>(messages[(slot + 3) % 4]),
                );
                messages[slot] =
                    _mm_sha256msg2_epu32(_mm_add_epi32(partial, middle), messages[(slot + 3) % 4]);
            }
            let constants = &ROUND_CONSTANTS[group * 4..group * 4 + 4];
            let rounds = _mm_add_epi32(
                messages[slot],
                _mm_setr_epi32(
                    signed(constants[0]),
                    signed(constants[1]),
                    signed(constants[2]),
                    signed(constants[3]),
                ),
            );
            cdgh = _mm_sha256rnds2_epu32(cdgh, abef, rounds);
            abef = _mm_sha256rnds2_epu32(abef, cdgh, _mm_shuffle_epi32::<0x0e>(rounds));
        }
        abef = _mm_add_epi32(abef, saved_abef);
        cdgh = _mm_add_epi32(cdgh, saved_cdgh);
    }

    let unsigned = |word: i32| u32::from_ne_bytes(word.to_ne_bytes());
    let low = |vector: __m128i| unsigned(_mm_cvtsi128_si32(vector));
    *state = [
        low(_mm_shuffle_epi32::<0xff>(abef)),
        low(_mm_shuffle_epi32::<0xaa>(abef)),
        low(_mm_shuffle_epi32::<0xff>(cdgh)),
        low(_mm_shuffle_epi32::<0xaa>(cdgh)),
        low(_mm_shuffle_epi32::<0x55>(abef)),
        low(abef),
        low(_mm_shuffle_epi32::<0x55>(cdgh)),
        low(cdgh),
    ];
}

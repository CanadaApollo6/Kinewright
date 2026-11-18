#![forbid(unsafe_code)]

pub mod fcpxml;
mod fcpxml_syntax;
mod fcpxml_time;
pub mod ir;
pub mod locator;
pub mod otio;
mod otio_syntax;
mod parse;
mod refusal;
mod time;

pub use refusal::{Refusal, RefusalCode};
pub use time::Time;

pub const MAX_INPUT_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_TREE_DEPTH: usize = 128;
pub const MAX_NEST_DEPTH: usize = 8;
pub const MAX_ITEMS: usize = 250_000;
pub const MAX_CLIPS: usize = 50_000;
pub const MAX_MEDIA: usize = 20_000;
pub const MAX_XML_NODES: u32 = 4_000_000;
pub const MAX_TRACKS_PER_KIND: usize = 128;
pub const MAX_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_RATE: u32 = 1_000;

fn shorten(text: &str, max: usize) -> &str {
    let end = text.floor_char_boundary(max);
    &text[..end]
}

fn check_rate(rate: kinewright_core::Rational) -> Result<kinewright_core::Rational, Refusal> {
    if rate.is_valid()
        && u64::from(rate.numerator()) <= u64::from(MAX_RATE) * u64::from(rate.denominator())
    {
        Ok(rate)
    } else {
        Err(Refusal::new(
            RefusalCode::UnsupportedRate,
            "The frame rate must be positive and at most 1,000 fps.",
        ))
    }
}

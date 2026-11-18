//! One descriptor-only `FFmpeg` input path for the app's remaining decoders and
//! the confined helper. Policy changes belong here and reach both callers.
pub mod codec_policy;
#[allow(unsafe_code)]
pub mod color;
#[allow(unsafe_code)]
pub mod owned_io;

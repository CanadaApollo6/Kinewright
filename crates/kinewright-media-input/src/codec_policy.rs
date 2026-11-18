//! The format and decoder policy from the pinned FFmpeg build profile.
use std::{ffi::CString, sync::LazyLock};

#[derive(serde::Deserialize)]
struct Profile {
    decoders: Vec<String>,
    demuxers: Vec<String>,
}
static PROFILE: LazyLock<Profile> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../scripts/ffmpeg-profile.json"))
        .expect("compiled FFmpeg profile")
});
pub static CODECS: LazyLock<CString> =
    LazyLock::new(|| CString::new(PROFILE.decoders.join(",")).expect("codec names"));
pub static FORMATS: LazyLock<CString> = LazyLock::new(|| {
    // libavformat's registered name differs from its configure component name.
    CString::new(
        PROFILE
            .demuxers
            .iter()
            .map(|name| match name.as_str() {
                "image_png_pipe" => "png_pipe",
                "image_jpeg_pipe" => "jpeg_pipe",
                "matroska" => "matroska,webm",
                name => name,
            })
            .collect::<Vec<_>>()
            .join(","),
    )
    .expect("format names")
});

#[must_use]
pub fn allows(id: ffmpeg_next::codec::Id) -> bool {
    let name = id.name();
    PROFILE
        .decoders
        .iter()
        .any(|decoder| decoder == name || (decoder == "libdav1d" && name == "av1"))
}

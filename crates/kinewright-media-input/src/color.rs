//! Numeric access to `FFmpeg` decoder colour metadata.
use ffmpeg_next as ffmpeg;

/// Read primaries, transfer, matrix and range as native numbers. `FFmpeg`
/// accepts metadata values outside bindgen's closed Rust enum variants.
#[must_use]
pub fn decoder_colour_numbers(decoder: &ffmpeg::codec::decoder::Video) -> (u32, u32, u32, u32) {
    const {
        assert!(size_of::<ffmpeg::ffi::AVColorPrimaries>() == size_of::<u32>());
        assert!(size_of::<ffmpeg::ffi::AVColorTransferCharacteristic>() == size_of::<u32>());
        assert!(size_of::<ffmpeg::ffi::AVColorSpace>() == size_of::<u32>());
        assert!(size_of::<ffmpeg::ffi::AVColorRange>() == size_of::<u32>());
    }
    // SAFETY: the borrowed decoder owns a live, initialized AVCodecContext.
    // These repr(u32) fields have the checked width above. Raw address reads
    // avoid loading a closed enum; every bit pattern is valid as u32. No other
    // decoder operation runs through this immutable borrow during the reads.
    unsafe {
        let context = decoder.as_ptr();
        (
            std::ptr::addr_of!((*context).color_primaries)
                .cast::<u32>()
                .read_unaligned(),
            std::ptr::addr_of!((*context).color_trc)
                .cast::<u32>()
                .read_unaligned(),
            std::ptr::addr_of!((*context).colorspace)
                .cast::<u32>()
                .read_unaligned(),
            std::ptr::addr_of!((*context).color_range)
                .cast::<u32>()
                .read_unaligned(),
        )
    }
}

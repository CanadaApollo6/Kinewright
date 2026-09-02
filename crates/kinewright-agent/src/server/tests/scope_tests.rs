//! Video scope tests.

use crate::server::color_qc::scope_data;

#[test]
fn scopes_are_exact_and_ignore_fully_transparent_pixels() {
    let scopes = scope_data(
        &kinewright_core::RgbaImage {
            width: 3,
            height: 1,
            pixels: vec![0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0],
        },
        16,
    );
    assert_eq!(scopes["visible_pixel_count"], 2);
    assert_eq!(scopes["clipping_basis_points"]["black"], 5_000);
    assert_eq!(scopes["clipping_basis_points"]["white"], 5_000);
    assert_eq!(scopes["mean_milli"]["luma"], 127_500);
    assert_eq!(scopes["histograms"]["luma"][0], 1);
    assert_eq!(scopes["histograms"]["luma"][15], 1);
}

//! Reframe focus geometry tests.

use super::*;
use crate::server::tracking::tracked_subject_focus_constraint;

fn crop_axis(focus_basis_points: i64, visible_basis_points: i64) -> (i64, i64) {
    let visible = visible_basis_points.clamp(1, 10_000);
    let maximum_left = 10_000 - visible;
    let left = focus_basis_points
        .saturating_sub(visible / 2)
        .clamp(0, maximum_left);
    (left, left + visible)
}

fn contains(
    focus_basis_points: i64,
    visible_basis_points: i64,
    subject_minimum: i64,
    subject_maximum: i64,
) -> bool {
    let (left, right) = crop_axis(focus_basis_points, visible_basis_points);
    left <= subject_minimum && right >= subject_maximum
}

#[test]
fn tracked_subject_constraint_inverts_clamped_cover_crop_at_both_edges() {
    let subject = TrackedSubjectBounds {
        at: TimeCode(69),
        left_basis_points: 500,
        right_basis_points: 700,
        top_basis_points: 1_000,
        bottom_basis_points: 2_000,
    };
    let constraint = tracked_subject_focus_constraint(subject, 1_920, 1_080, 5_625)
        .expect("subject fits the vertical short crop");

    // 1920x1080 into a 9:16 delivery leaves 3165 basis points of source
    // width visible. The left crop edge is clamped for focus 0..=1582,
    // so the valid focus interval includes that entire plateau.
    assert_eq!(
        (constraint.min_x_basis_points, constraint.max_x_basis_points),
        (0, 2_082)
    );
    assert_eq!(
        (constraint.min_y_basis_points, constraint.max_y_basis_points),
        (0, 10_000)
    );
    for focus in constraint.min_x_basis_points..=constraint.max_x_basis_points {
        assert!(contains(
            focus,
            3_165,
            i64::from(subject.left_basis_points),
            i64::from(subject.right_basis_points)
        ));
    }
    assert!(!contains(2_083, 3_165, 500, 700));

    let right_edge_subject = TrackedSubjectBounds {
        left_basis_points: 9_000,
        right_basis_points: 9_500,
        ..subject
    };
    let right_edge = tracked_subject_focus_constraint(right_edge_subject, 1_920, 1_080, 5_625)
        .expect("right-edge subject fits the crop");
    assert_eq!(
        (right_edge.min_x_basis_points, right_edge.max_x_basis_points),
        (7_917, 10_000)
    );
    for focus in right_edge.min_x_basis_points..=right_edge.max_x_basis_points {
        assert!(contains(focus, 3_165, 9_000, 9_500));
    }
    assert!(!contains(7_916, 3_165, 9_000, 9_500));
}

#[test]
fn tracked_subject_constraint_uses_the_same_aspect_rounding_as_evaluator() {
    let subject = TrackedSubjectBounds {
        at: TimeCode(235),
        left_basis_points: 1_938,
        right_basis_points: 6_541,
        top_basis_points: 3_000,
        bottom_basis_points: 6_000,
    };
    let constraint = tracked_subject_focus_constraint(subject, 1_080, 1_920, 16_000)
        .expect("subject fits the tall crop");

    // ceil(1080 * 100000000 / (1920 * 16000)) = 3516. The helper must
    // preserve that conservative evaluator rounding when inverting the
    // vertical crop axis.
    assert_eq!(
        (constraint.min_y_basis_points, constraint.max_y_basis_points),
        (4_242, 4_758)
    );
    for focus in constraint.min_y_basis_points..=constraint.max_y_basis_points {
        assert!(contains(focus, 3_516, 3_000, 6_000));
    }
}

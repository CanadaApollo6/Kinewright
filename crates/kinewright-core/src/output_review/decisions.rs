use super::{OverlayKind, OverlayText, OverlayUnknown, ReviewExpectations, ReviewOverlay};
use crate::ClipId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use unicode_general_category::{GeneralCategory, get_general_category};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoverageProfileId {
    #[serde(rename = "output_review/1")]
    OutputReviewV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredReviewRow {
    StructureAndAvIntegrity,
    LoudnessAndPeak,
    TechnicalDefectScan,
    OverlayVerification,
    RetainedSpeechChecks,
    IndependentVisualReview,
    EditorialAuditPresent,
}

impl CoverageProfileId {
    #[must_use]
    pub const fn required_rows(self) -> &'static [RequiredReviewRow] {
        use RequiredReviewRow::{
            EditorialAuditPresent, IndependentVisualReview, LoudnessAndPeak, OverlayVerification,
            RetainedSpeechChecks, StructureAndAvIntegrity, TechnicalDefectScan,
        };
        match self {
            Self::OutputReviewV1 => &[
                StructureAndAvIntegrity,
                LoudnessAndPeak,
                TechnicalDefectScan,
                OverlayVerification,
                RetainedSpeechChecks,
                IndependentVisualReview,
                EditorialAuditPresent,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReviewRowId {
    Required(RequiredReviewRow),
    Other(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewRowResult {
    Pass,
    Unknown,
    Failed,
    Unrun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageBlocker {
    Missing,
    Unknown,
    Failed,
    Unrun,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageCheck {
    pub blockers: Vec<(RequiredReviewRow, CoverageBlocker)>,
    pub extra_rows: Vec<ReviewRowId>,
}

impl CoverageCheck {
    #[must_use]
    pub const fn all_required_passed(&self) -> bool {
        self.blockers.is_empty()
    }
}

#[must_use]
pub fn check_review_coverage(
    profile: CoverageProfileId,
    results: &BTreeMap<ReviewRowId, ReviewRowResult>,
) -> CoverageCheck {
    let required = profile.required_rows();
    let blockers = required
        .iter()
        .filter_map(|&row| {
            let blocker = match results.get(&ReviewRowId::Required(row)) {
                Some(ReviewRowResult::Pass) => return None,
                Some(ReviewRowResult::Unknown) => CoverageBlocker::Unknown,
                Some(ReviewRowResult::Failed) => CoverageBlocker::Failed,
                Some(ReviewRowResult::Unrun) => CoverageBlocker::Unrun,
                None => CoverageBlocker::Missing,
            };
            Some((row, blocker))
        })
        .collect();
    let extra_rows = results
        .keys()
        .copied()
        .filter(|row| match row {
            ReviewRowId::Required(row) => !required.contains(row),
            ReviewRowId::Other(_) => true,
        })
        .collect();
    CoverageCheck {
        blockers,
        extra_rows,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EligibilityReason {
    UnknownText(OverlayUnknown),
    EmptyText,
    CaptionSpeechNotDetermined,
    CaptionWordsDisagree,
    CaptionWordClipped,
    SourcedText,
    DecimalDigit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryException {
    Element {
        element: ClipId,
        reason: EligibilityReason,
    },
    OverlayInventoryNotDetermined(OverlayKind),
}

/// The seal constructs an outcome only after its required coverage passes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SealedReviewOutcome {
    Delivered,
    ReviewedFactsNotChecked { exceptions: Vec<DeliveryException> },
}

/// An empty exception list makes the cut eligible. Coverage is checked separately.
#[must_use]
pub fn delivery_eligibility(expectations: &ReviewExpectations) -> Vec<DeliveryException> {
    let mut exceptions = Vec::new();
    for (kind, inventory) in [
        (OverlayKind::Caption, &expectations.overlays.captions),
        (OverlayKind::Title, &expectations.overlays.titles),
        (OverlayKind::Card, &expectations.overlays.cards),
    ] {
        if !inventory.is_determined() {
            exceptions.push(DeliveryException::OverlayInventoryNotDetermined(kind));
        }
        for overlay in inventory.known() {
            if let Some(reason) = ineligible_overlay(expectations, overlay) {
                exceptions.push(DeliveryException::Element {
                    element: overlay.element,
                    reason,
                });
            }
        }
    }
    exceptions
}

fn ineligible_overlay(
    expectations: &ReviewExpectations,
    overlay: &ReviewOverlay,
) -> Option<EligibilityReason> {
    let text = match &overlay.text {
        OverlayText::Expected(text) => text,
        OverlayText::Unknown(reason) => return Some(EligibilityReason::UnknownText(*reason)),
    };
    if text.trim().is_empty() {
        return Some(EligibilityReason::EmptyText);
    }
    match overlay.kind {
        OverlayKind::Title | OverlayKind::Card => {
            if overlay.sourced {
                return Some(EligibilityReason::SourcedText);
            }
            text.chars()
                .any(decimal_digit)
                .then_some(EligibilityReason::DecimalDigit)
        }
        OverlayKind::Caption => {
            if !expectations.kept_words.is_determined() {
                return Some(EligibilityReason::CaptionSpeechNotDetermined);
            }
            let words = || {
                expectations.kept_words.known().filter(|word| {
                    word.output.start < overlay.cue.end && overlay.cue.start < word.output.end
                })
            };
            if words().any(|word| word.clipped) {
                return Some(EligibilityReason::CaptionWordClipped);
            }
            let expected = normalized_tokens(text);
            let matches = !expected.is_empty()
                && words()
                    .flat_map(|word| normalized_tokens(&word.text))
                    .eq(expected);
            (!matches).then_some(EligibilityReason::CaptionWordsDisagree)
        }
    }
}

fn decimal_digit(c: char) -> bool {
    get_general_category(c) == GeneralCategory::DecimalNumber
}

fn normalized_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        let next_is_digit = next.is_some_and(decimal_digit);
        let number_after_sign = || {
            let mut remaining = chars.clone().skip_while(|c| c.is_whitespace());
            match remaining.next() {
                Some('.') => remaining.next().is_some_and(decimal_digit),
                Some(digit) => decimal_digit(digit),
                None => false,
            }
        };
        if matches!(c, '+' | '-' | '−') && number_after_sign() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            token.push(if c == '−' { '-' } else { c });
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
        } else if c.is_alphanumeric()
            || matches!(
                get_general_category(c),
                GeneralCategory::NonspacingMark | GeneralCategory::SpacingMark
            )
            || c == '.'
                && next_is_digit
                && (token.is_empty()
                    || token.ends_with(decimal_digit)
                    || token.ends_with(['+', '-']))
        {
            token.extend(c.to_lowercase());
        } else if matches!(c, '\'' | '’')
            && token.ends_with(char::is_alphabetic)
            && next.is_some_and(char::is_alphabetic)
        {
            // Apostrophes inside a word are punctuation, including curly forms.
        } else if !token.is_empty() {
            tokens.push(std::mem::take(&mut token));
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output_review::*;
    use crate::{ExactMicros, Rational, TimeCode};

    #[test]
    fn output_review_v1_requires_seven_explicit_passes() {
        use RequiredReviewRow::*;
        let profile = CoverageProfileId::OutputReviewV1;
        let required = [
            StructureAndAvIntegrity,
            LoudnessAndPeak,
            TechnicalDefectScan,
            OverlayVerification,
            RetainedSpeechChecks,
            IndependentVisualReview,
            EditorialAuditPresent,
        ];
        assert_eq!(profile.required_rows(), required);
        let extra = ReviewRowId::Other(8);
        let mut results = BTreeMap::from([(extra, ReviewRowResult::Pass)]);
        assert_eq!(
            check_review_coverage(profile, &results).blockers,
            required.map(|row| (row, CoverageBlocker::Missing))
        );
        results.extend(required.map(|row| (ReviewRowId::Required(row), ReviewRowResult::Pass)));
        for row in required {
            for (status, reason) in [
                (None, CoverageBlocker::Missing),
                (Some(ReviewRowResult::Unknown), CoverageBlocker::Unknown),
                (Some(ReviewRowResult::Failed), CoverageBlocker::Failed),
                (Some(ReviewRowResult::Unrun), CoverageBlocker::Unrun),
            ] {
                let key = ReviewRowId::Required(row);
                if let Some(status) = status {
                    results.insert(key, status);
                } else {
                    results.remove(&key);
                }
                let check = check_review_coverage(profile, &results);
                assert!(!check.all_required_passed());
                assert_eq!(check.blockers, [(row, reason)]);
                assert_eq!(check.extra_rows, [extra]);
                results.insert(key, ReviewRowResult::Pass);
            }
        }
        let check = check_review_coverage(profile, &results);
        assert!(check.all_required_passed());
        assert_eq!(check.extra_rows, [extra]);
    }

    fn with_typed_card(sourced: bool) -> ReviewExpectations {
        ReviewExpectations {
            identity: ReviewCutIdentity {
                snapshot: ReviewSnapshotId([1; 32]),
                input_manifest_sha256: [2; 32],
                export_settings_sha256: [3; 32],
            },
            project_fps: Rational::default(),
            output_fps: Rational::default(),
            raster: (640, 480),
            output_frames: TimeCode(30),
            transcripts: Vec::new(),
            segments: Vec::new(),
            kept_words: ReviewInventory::NoneProven(AbsenceProof::NoAudibleSourceSegments),
            speech_windows: ReviewInventory::NoneProven(AbsenceProof::NoAudibleSourceSegments),
            intended_pauses: ReviewInventory::NoneProven(AbsenceProof::NoAuthorPauseDeclarations),
            av_landmarks: ReviewInventory::NoneProven(AbsenceProof::NoPairedAudioVideo),
            overlays: OverlayExpectations {
                captions: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
                titles: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
                cards: ReviewInventory::Present {
                    first: ReviewOverlay {
                        element: ClipId(15),
                        kind: OverlayKind::Card,
                        cue: ExactMicros::ZERO..ExactMicros::integer(1_000_000),
                        visible: Vec::new(),
                        text: OverlayText::Expected("Opening scene".to_owned()),
                        pixels: OverlayPixels::RenderedText,
                        sourced,
                    },
                    rest: Vec::new(),
                },
            },
        }
    }

    #[test]
    fn output_review_eligibility_reports_a_sourced_typed_card() {
        assert!(delivery_eligibility(&with_typed_card(false)).is_empty());
        assert_eq!(
            delivery_eligibility(&with_typed_card(true)),
            [DeliveryException::Element {
                element: ClipId(15),
                reason: EligibilityReason::SourcedText
            },]
        );
    }
}

// Derived from Scarlet packages/mcp/src/dialogue-join.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_HOP_MICROS: u32 = 10_000;
/// Signed hundredths of dBFS. Measurements below -120 dBFS saturate here.
pub const LEVEL_FLOOR: i16 = -12_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Hop {
    pub peak_dbfs_hundredths: i16,
    pub rms_dbfs_hundredths: i16,
    pub voice: bool,
}

/// Compact parallel arrays; validation is shared by builders and deserialization.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawProfile")]
pub struct SoundProfile {
    hop_micros: u32,
    duration_micros: u64,
    peaks: Box<[i16]>,
    rms: Box<[i16]>,
    voice: Box<[bool]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    hop_micros: u32,
    duration_micros: u64,
    peaks: Vec<i16>,
    rms: Vec<i16>,
    voice: Vec<bool>,
}

impl TryFrom<RawProfile> for SoundProfile {
    type Error = ProfileError;
    fn try_from(raw: RawProfile) -> Result<Self, Self::Error> {
        Self::from_raw(
            raw.hop_micros,
            raw.duration_micros,
            raw.peaks,
            raw.rms,
            raw.voice,
        )
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum ProfileError {
    #[error("profile hop must be between 1 and 31000 microseconds")]
    Hop,
    #[error("profile arrays must agree and cover the nonempty recording exactly")]
    Length,
    #[error("profile hop {index} has a positive level or RMS greater than peak")]
    Level { index: usize },
}

fn expected_hops(hop_micros: u32, duration_micros: u64) -> Result<u64, ProfileError> {
    if hop_micros == 0 || hop_micros > 31_000 {
        return Err(ProfileError::Hop);
    }
    if duration_micros == 0 {
        return Err(ProfileError::Length);
    }
    Ok(duration_micros.div_ceil(u64::from(hop_micros)))
}

impl SoundProfile {
    /// Consume measurements. The last hop may be shorter; no padding beyond the
    /// recording is treated as evidence. Levels below the stated floor saturate.
    ///
    /// # Errors
    /// Refuses coarse/zero hops, inconsistent lengths/duration, or invalid levels.
    pub fn from_raw(
        hop_micros: u32,
        duration_micros: u64,
        mut peaks: Vec<i16>,
        mut rms: Vec<i16>,
        voice: Vec<bool>,
    ) -> Result<Self, ProfileError> {
        let expected = expected_hops(hop_micros, duration_micros)?;
        if peaks.len() != rms.len()
            || peaks.len() != voice.len()
            || u64::try_from(peaks.len()).ok() != Some(expected)
        {
            return Err(ProfileError::Length);
        }
        for (index, (peak, mean)) in peaks.iter_mut().zip(&mut rms).enumerate() {
            *peak = (*peak).max(LEVEL_FLOOR);
            *mean = (*mean).max(LEVEL_FLOOR);
            if *peak > 0 || *mean > *peak {
                return Err(ProfileError::Level { index });
            }
        }
        Ok(Self {
            hop_micros,
            duration_micros,
            peaks: peaks.into_boxed_slice(),
            rms: rms.into_boxed_slice(),
            voice: voice.into_boxed_slice(),
        })
    }

    /// Build through the same validation door as cached profiles.
    ///
    /// # Errors
    /// Returns the same validation refusals as [`Self::from_raw`].
    pub fn from_hops(
        hop_micros: u32,
        duration_micros: u64,
        hops: impl IntoIterator<Item = Hop>,
    ) -> Result<Self, ProfileError> {
        let expected = expected_hops(hop_micros, duration_micros)?;
        let mut peaks = Vec::new();
        let mut rms = Vec::new();
        let mut voice = Vec::new();
        for (index, hop) in hops.into_iter().enumerate() {
            if u64::try_from(index).map_or(true, |index| index >= expected) {
                return Err(ProfileError::Length);
            }
            peaks.push(hop.peak_dbfs_hundredths);
            rms.push(hop.rms_dbfs_hundredths);
            voice.push(hop.voice);
        }
        Self::from_raw(hop_micros, duration_micros, peaks, rms, voice)
    }

    #[must_use]
    pub const fn hop_micros(&self) -> u32 {
        self.hop_micros
    }
    #[must_use]
    pub const fn duration_micros(&self) -> u64 {
        self.duration_micros
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.peaks.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peaks.is_empty()
    }
    /// Borrow all validated hop measurements without copying the profile.
    pub fn hops(&self) -> impl ExactSizeIterator<Item = Hop> {
        self.peaks
            .iter()
            .zip(&self.rms)
            .zip(&self.voice)
            .map(|((&peak, &rms), &voice)| Hop {
                peak_dbfs_hundredths: peak,
                rms_dbfs_hundredths: rms,
                voice,
            })
    }
    #[must_use]
    pub fn hop(&self, index: usize) -> Option<Hop> {
        Some(Hop {
            peak_dbfs_hundredths: *self.peaks.get(index)?,
            rms_dbfs_hundredths: self.rms[index],
            voice: self.voice[index],
        })
    }
    pub(super) fn span(&self, index: usize) -> super::Span {
        let start =
            u64::try_from(index).expect("validated profile index") * u64::from(self.hop_micros);
        super::Span {
            start_micros: start,
            end_micros: start
                .saturating_add(u64::from(self.hop_micros))
                .min(self.duration_micros),
        }
    }
    pub(super) fn index_at(&self, numerator: u128, denominator: u128) -> Option<usize> {
        if numerator >= u128::from(self.duration_micros) * denominator {
            return None;
        }
        usize::try_from(numerator / (denominator * u128::from(self.hop_micros))).ok()
    }
}

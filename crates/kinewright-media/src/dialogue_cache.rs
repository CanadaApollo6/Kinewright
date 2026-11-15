//! Exact cached recognition and source acoustic evidence, read under a session.
use kinewright_core::{
    TranscriptStatus,
    dialogue::{SoundProfile, Word},
};

pub(crate) struct WordEvidence {
    pub recogniser_key: String,
    pub words: Vec<Word>,
}

/// Cache reads never perform recognition or acoustic measurement inline.
pub enum DialogueCacheRead {
    Ready {
        recogniser_key: String,
        profile: SoundProfile,
        words: Vec<Word>,
    },
    Pending(TranscriptStatus),
}

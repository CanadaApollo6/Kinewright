//! Bind shared backends to the project that originated each request.
#[cfg(test)]
use kinewright_core::RefusedMedia;
use kinewright_core::{Analysis, Document, Export, LegacyMediaGrant, Playback};
use kinewright_media::{FfmpegMediaEngine, owned_io::PlaybackInputs};
use std::sync::Arc;

pub(crate) struct MediaPorts {
    pub(crate) playback: Arc<dyn Playback>,
    pub(crate) analysis: Arc<dyn Analysis>,
    pub(crate) exporter: Arc<dyn Export>,
    pub(crate) unavailable: bool,
}

pub(crate) enum MediaBackend {
    Engine(Arc<FfmpegMediaEngine>),
    #[cfg(test)]
    Stub {
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        exporter: Arc<dyn Export>,
    },
}

impl MediaBackend {
    pub(crate) fn scoped(
        &self,
        document: Arc<Document>,
        legacy: Option<LegacyMediaGrant>,
        inputs: Option<Arc<PlaybackInputs>>,
    ) -> MediaPorts {
        let unavailable = legacy.is_none() && inputs.is_none();
        match self {
            Self::Engine(engine) => {
                let media = if let Some(grant) = legacy {
                    engine.legacy_media(grant, document)
                } else if let Some(inputs) = inputs {
                    engine.certified_media_for_document(inputs, document)
                } else {
                    engine.unadmitted_media(document)
                };
                MediaPorts {
                    playback: media.clone(),
                    analysis: media.clone(),
                    exporter: media,
                    unavailable,
                }
            }
            #[cfg(test)]
            Self::Stub {
                playback,
                analysis,
                exporter,
            } if legacy.is_some() => MediaPorts {
                playback: Arc::clone(playback),
                analysis: Arc::clone(analysis),
                exporter: Arc::clone(exporter),
                unavailable,
            },
            #[cfg(test)]
            Self::Stub { .. } => MediaPorts {
                playback: Arc::new(RefusedMedia),
                analysis: Arc::new(RefusedMedia),
                exporter: Arc::new(RefusedMedia),
                unavailable: true,
            },
        }
    }
}

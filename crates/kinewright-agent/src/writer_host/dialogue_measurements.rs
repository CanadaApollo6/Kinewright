//! Native task measurements retain the run's owned sources and the task's admission.
use super::LazyHostMedia;
use crate::{
    dialogue_measurements::{CachedDialogueMeasurements, failed},
    dialogue_tools::{BoundDialogue, DialogueMeasurements, MediaIdentity, Refusal},
};
use kinewright_core::{Command, Core, Document, Event, Query, QueryResult};
use kinewright_media::owned_io::PlaybackInputs;
use std::sync::{Arc, OnceLock};

fn document(core: &Core) -> Result<Arc<Document>, String> {
    let Event::QueryResult(QueryResult::Document(document)) = core
        .request(Command::Query(Query::Document))
        .map_err(|e| e.to_string())?
    else {
        return Err("task document unavailable".into());
    };
    Ok(document)
}

pub(super) struct TaskDialogueMeasurements {
    core: Core,
    media: Arc<LazyHostMedia>,
    inputs: Arc<PlaybackInputs>,
    admitted: Vec<MediaIdentity>,
    provider: OnceLock<Result<CachedDialogueMeasurements, String>>,
}
impl TaskDialogueMeasurements {
    pub(super) fn new(
        core: Core,
        media: Arc<LazyHostMedia>,
        inputs: Arc<PlaybackInputs>,
        manifest: &kinewright_project::owned_inputs::OwnedInputManifest,
    ) -> Result<Self, super::HostError> {
        let document = document(&core).map_err(super::HostError::Protocol)?;
        let admitted = document
            .media_pool
            .iter()
            .filter(|asset| {
                manifest
                    .entries
                    .iter()
                    .any(|entry| entry.matches_media_asset(asset))
            })
            .map(MediaIdentity::of)
            .collect::<Result<_, _>>()
            .map_err(|e| super::HostError::Protocol(e.to_string()))?;
        Ok(Self {
            core,
            media,
            inputs,
            admitted,
            provider: OnceLock::new(),
        })
    }
    fn provider(&self, identity: &MediaIdentity) -> Result<&CachedDialogueMeasurements, Refusal> {
        if !self.admitted.contains(identity) {
            return Err(Refusal::Identity);
        }
        self.inputs
            .check_read_authority()
            .map_err(|e| failed(e.to_string()))?;
        self.provider
            .get_or_init(|| {
                let document = document(&self.core)?;
                let media = self
                    .media
                    .get()
                    .map_err(|e| e.to_string())?
                    .certified_media_for_document(Arc::clone(&self.inputs), document);
                Ok(CachedDialogueMeasurements::new(self.core.clone(), media))
            })
            .as_ref()
            .map_err(failed)
    }
}
impl DialogueMeasurements for TaskDialogueMeasurements {
    fn audio_stream(&self, identity: &MediaIdentity) -> Result<u32, Refusal> {
        self.provider(identity)?.audio_stream(identity)
    }
    fn get(&self, identity: &MediaIdentity, stream: u32) -> Result<Arc<BoundDialogue>, Refusal> {
        self.provider(identity)?.get(identity, stream)
    }
}

use kinewright_project::bundle_records::RecordKind;

/// A wire descriptor for the accompanying legacy bytes, never a host `ObjectRef`.
#[derive(serde::Serialize)]
struct ProviderReference {
    sha256: String,
    byte_length: u64,
    record_kind: RecordKind,
}
impl ProviderReference {
    fn from_host(reference: &ObjectRef) -> Self {
        Self {
            sha256: reference.sha256.clone(),
            byte_length: reference.byte_length,
            record_kind: reference.record_kind,
        }
    }
}
pub(super) struct ExpandedResult {
    reference: ProviderReference,
    pub(super) contents: serde_json::Value,
}

pub(super) fn provider_accounting_view(
    record: &AccountingRecord,
) -> Result<AccountingRecord, HostError> {
    use kinewright_project::orchestration_accounting::{AccountingRefusal, normalize};
    let mut visible = record.clone();
    visible.revision = record
        .revision
        .checked_sub(record.observer_publications)
        .ok_or_else(|| refused("observer revision exceeds accounting revision"))?;
    visible.observer_publications = 0;
    for task in &mut visible.tasks {
        task.measurements = None;
        task.streams.clear();
        let mut identities = std::collections::BTreeSet::new();
        let mut complete = std::collections::BTreeMap::new();
        let mut suppressed_eof = false;
        task.usage.retain_mut(|sample| {
            if sample.event.starts_with("turn.started:") {
                return false;
            }
            if let Some(snapshot) = &sample.snapshot
                && let Some((kind, identity)) = sample.event.rsplit_once(':')
                && identity.len() == 16
                && identity.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                sample.event = format!("{kind}:{}", snapshot.payload_sha256);
            }
            sample.snapshot = None;
            let turn = (sample.attempt.clone(), sample.turn);
            if sample.event.starts_with("native.eof:") && complete.get(&turn) == Some(&true) {
                suppressed_eof = true;
                return false;
            }
            if !identities.insert((turn.clone(), sample.event.clone())) {
                return false;
            }
            complete.insert(turn, sample.complete);
            true
        });
        task.totals = normalize(&task.usage).map_err(refused)?;
        if suppressed_eof
            && task.totals.tokens_complete
            && task.pause == Some(AccountingRefusal::UsageUnavailable)
        {
            task.pause = None;
        }
    }
    Ok(visible)
}
impl HostCommandOwner {
    pub(super) fn expanded_result(
        &self,
        reference: &ObjectRef,
    ) -> Result<ExpandedResult, HostError> {
        if reference.byte_length > (crate::protocol::limits::FRAME_BYTES / 2) as u64 {
            return Err(refused("expanded result capacity"));
        }
        let mut descriptor = ProviderReference::from_host(reference);
        let contents = match reference.record_kind {
            RecordKind::OwnedEvidence => {
                let bytes = self.owner()?.store()?.read_evidence(reference)?;
                match std::str::from_utf8(&bytes) {
                    Ok(text) => serde_json::Value::String(text.into()),
                    Err(_) => serde_json::json!({"encoding":"bytes", "contents":bytes.to_vec()}),
                }
            }
            RecordKind::RunIndex => {
                let mut record = (*self.owner()?.store()?.read_run_record(reference)?).clone();
                if let OrchestrationPayload::Accounting(accounting) = &mut record.payload {
                    **accounting = provider_accounting_view(accounting)?;
                    let bytes = serde_json::to_vec(&record)?;
                    descriptor.sha256 = kinewright_media::sha256_bytes(&bytes);
                    descriptor.byte_length = bytes.len() as u64;
                }
                serde_json::to_value(record)?
            }
            RecordKind::SpecialistBase => serde_json::to_value(
                &*self
                    .owner()?
                    .store()?
                    .read_record::<kinewright_project::bundle_records::SpecialistBaseRecord>(
                        reference,
                    )?,
            )?,
            _ => return Err(refused("unsupported expanded result record")),
        };
        Ok(ExpandedResult {
            reference: descriptor,
            contents,
        })
    }
    pub(super) fn result_wake_payload(
        &self,
        references: &[&ObjectRef],
    ) -> Result<Vec<u8>, HostError> {
        let mut expanded = Vec::new();
        for reference in references.iter().copied() {
            let record = self.owner()?.store()?.read_run_record(reference)?;
            let OrchestrationPayload::Result(result) = &record.payload else {
                return Err(refused("wake reference is not a task result"));
            };
            let contents = |reference: &ObjectRef| {
                self.expanded_result(reference)
                    .map(|expanded| expanded.contents)
            };
            let usage = self.expanded_result(&result.usage_ref)?;
            let mut provider_result = serde_json::to_value(result)?;
            provider_result["usage_ref"] = serde_json::to_value(usage.reference)?;
            expanded.push(serde_json::json!({
                "result": provider_result,
                "explanation": contents(&result.explanation)?,
                "delta": result.delta_ref.as_ref().map(contents).transpose()?,
                "artifacts": result.artifacts.iter().map(contents).collect::<Result<Vec<_>,_>>()?,
                "evidence": result.evidence.iter().map(contents).collect::<Result<Vec<_>,_>>()?,
                "usage": usage.contents,
            }));
        }
        Ok(serde_json::to_vec(&expanded)?)
    }
    pub(super) fn finish_accounting_observation(
        previous: &AccountingRecord,
        next: &mut AccountingRecord,
    ) -> Result<(), HostError> {
        let revision = std::mem::replace(&mut next.revision, previous.revision);
        let changed = *next != *previous;
        next.revision = revision;
        if !changed {
            return Ok(());
        }
        let before = provider_accounting_view(previous)?;
        let mut after = provider_accounting_view(next)?;
        after.revision = before.revision;
        if after == before {
            next.observer_publications = next
                .observer_publications
                .checked_add(1)
                .ok_or_else(|| refused("observer revision capacity"))?;
        }
        Ok(())
    }
}

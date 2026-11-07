//! Provider-visible accounting projection; authoritative host usage stays unchanged.
use super::{HostError, HostRefusal};
use kinewright_project::orchestration_accounting::AccountingRecord;
fn refused(reason: impl std::fmt::Debug) -> HostError {
    HostRefusal::new("native_invocation", &format!("{reason:?}")).into()
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

//! A task request is answered once by the host, never by the legacy UI policy.
use super::{ConfirmationBroker, ConfirmationDecision};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct TaskConfirmation {
    broker: ConfirmationBroker,
    id: u64,
    pub digest: String,
    pub description: String,
    created: Instant,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Retain the receiver until the RAII refusal has been sent"
        )
    )]
    answer: crossbeam_channel::Receiver<ConfirmationDecision>,
}
pub(crate) struct TaskAnswer {
    sender: Option<crossbeam_channel::Sender<ConfirmationDecision>>,
    created: Instant,
    timeout: Duration,
}
impl TaskAnswer {
    pub fn approve(mut self) -> Result<(), String> {
        if self.created.elapsed() >= self.timeout {
            return Err("confirmation expired; time passing is not consent".into());
        }
        self.sender
            .take()
            .ok_or("confirmation already answered")?
            .send(ConfirmationDecision::Approved)
            .map_err(|_| "confirmation cancelled".into())
    }
}
impl Drop for TaskAnswer {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(ConfirmationDecision::Rejected(
                "task confirmation refused".into(),
            ));
        }
    }
}
impl Drop for TaskConfirmation {
    fn drop(&mut self) {
        // An unanswered request, including panic/early return, is a refusal.
        let _ = self.broker.claim_task(self).map(drop);
    }
}
impl ConfirmationBroker {
    #[cfg(test)]
    pub(crate) fn pending_task_count(&self) -> usize {
        self.task_pending.lock().unwrap().len()
    }
    pub(crate) fn request_task(
        &self,
        binding: &serde_json::Value,
        description: String,
    ) -> Result<TaskConfirmation, String> {
        #[cfg(test)]
        let binding = {
            let mut value = binding.clone();
            if let Some(hook) = self.task_binding_hook.lock().unwrap().take() {
                hook(&mut value);
            }
            value
        };
        let digest = kinewright_media::sha256_bytes(
            &serde_json::to_vec(&binding).map_err(|e| e.to_string())?,
        );
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let mut bound = self
            .task_pending
            .lock()
            .map_err(|_| "confirmation broker stopped")?;
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "confirmation broker stopped")?;
        if pending.len() >= kinewright_project::orchestration_records::ITEM_LIMIT {
            return Err("confirmation capacity".into());
        }
        bound.insert(id, digest.clone());
        pending.insert(id, sender);
        drop(pending);
        drop(bound);
        let request = TaskConfirmation {
            broker: self.clone(),
            id,
            digest,
            description,
            created: Instant::now(),
            answer: receiver,
        };
        #[cfg(test)]
        if let Some(hook) = self.task_hook.lock().unwrap().take() {
            hook();
        }
        Ok(request)
    }
    pub(crate) fn claim_task(&self, request: &TaskConfirmation) -> Result<TaskAnswer, String> {
        if !Arc::ptr_eq(&self.pending, &request.broker.pending) {
            return Err("confirmation broker changed".into());
        }
        let mut bound = self
            .task_pending
            .lock()
            .map_err(|_| "confirmation broker stopped")?;
        if bound.get(&request.id) != Some(&request.digest) {
            return Err("confirmation identity changed or already answered".into());
        }
        let sender = self
            .pending
            .lock()
            .map_err(|_| "confirmation broker stopped")?
            .remove(&request.id)
            .ok_or("confirmation cancelled")?;
        bound.remove(&request.id);
        let answer = TaskAnswer {
            sender: Some(sender),
            created: request.created,
            timeout: self.timeout,
        };
        if request.created.elapsed() >= self.timeout {
            return Err("confirmation expired; time passing is not consent".into());
        }
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn te15_b21_broker_binds_digest_and_refuses_late_replayed_or_legacy_answers() {
        let broker = ConfirmationBroker::default();
        let mut request = broker
            .request_task(
                &serde_json::json!({"run":"a","batch":"one"}),
                "Remove one clip".into(),
            )
            .unwrap();
        let answer = request.answer.clone();
        assert!(!broker.approve(request.id), "B21_NO_LEGACY_ANSWER");
        let original = request.digest.clone();
        request.digest = "wrong digest".into();
        assert!(broker.claim_task(&request).is_err(), "B21_DIGEST_BOUND");
        request.digest = original;
        let other = ConfirmationBroker::default();
        let other_request = other
            .request_task(
                &serde_json::json!({"run":"a","batch":"one"}),
                "Remove one clip".into(),
            )
            .unwrap();
        let other_answer = other_request.answer.clone();
        assert_eq!(request.id, other_request.id);
        assert_eq!(request.digest, other_request.digest);
        assert!(
            other.claim_task(&request).is_err(),
            "B21_BROKER_INSTANCE_BOUND"
        );
        other.claim_task(&other_request).unwrap().approve().unwrap();
        assert_eq!(
            other_answer.try_recv().unwrap(),
            ConfirmationDecision::Approved
        );
        broker.claim_task(&request).unwrap().approve().unwrap();
        assert_eq!(answer.try_recv().unwrap(), ConfirmationDecision::Approved);
        assert!(broker.claim_task(&request).is_err(), "B21_ANSWER_ONCE");
        assert!(!broker.approve(request.id), "B21_LATE_ANSWER_REFUSED");
        let request = broker
            .request_task(&serde_json::json!({"batch":"two"}), "Remove a track".into())
            .unwrap();
        let answer = request.answer.clone();
        broker.reject_all("Stop or retirement");
        assert!(
            matches!(answer.try_recv(), Ok(ConfirmationDecision::Rejected(_))),
            "B21_PENDING_CANCELLED"
        );
        assert!(
            broker.claim_task(&request).is_err(),
            "B21_CANCEL_NOT_REVIVED"
        );
    }
    #[test]
    fn te15_b21_broker_never_treats_elapsed_time_or_no_answer_as_consent() {
        let broker = ConfirmationBroker::default();
        let mut request = broker
            .request_task(&serde_json::json!({"batch":"one"}), "Remove a clip".into())
            .unwrap();
        let answer = request.answer.clone();
        request.created -= std::time::Duration::from_secs(61);
        assert!(broker.claim_task(&request).is_err(), "B21_TIME_IS_REFUSAL");
        assert!(matches!(
            answer.try_recv().unwrap(),
            ConfirmationDecision::Rejected(_)
        ));
        let request = broker
            .request_task(
                &serde_json::json!({"batch":"late-answer"}),
                "Remove a clip".into(),
            )
            .unwrap();
        let answer = request.answer.clone();
        let mut held = broker.claim_task(&request).unwrap();
        held.created -= Duration::from_secs(61);
        assert!(held.approve().is_err(), "B21_ANSWER_DEADLINE");
        assert!(matches!(
            answer.try_recv().unwrap(),
            ConfirmationDecision::Rejected(_)
        ));
        let request = broker
            .request_task(&serde_json::json!({"batch":"two"}), "Remove a track".into())
            .unwrap();
        let answer = request.answer.clone();
        drop(request);
        assert!(
            matches!(answer.try_recv(), Ok(ConfirmationDecision::Rejected(_))),
            "B21_UNANSWERED_REFUSED"
        );
    }
    #[test]
    fn te15_b21_pending_broker_capacity_is_bounded_and_reusable() {
        let broker = ConfirmationBroker::default();
        let requests: Vec<_> = (0..kinewright_project::orchestration_records::ITEM_LIMIT)
            .map(|i| {
                broker
                    .request_task(&serde_json::json!({"batch":i}), "Remove a clip".into())
                    .unwrap()
            })
            .collect();
        assert!(
            broker
                .request_task(
                    &serde_json::json!({"batch":"overflow"}),
                    "Remove a track".into()
                )
                .is_err(),
            "B21_PENDING_CAPACITY"
        );
        drop(requests);
        assert_eq!(broker.pending_task_count(), 0);
        let request = broker
            .request_task(
                &serde_json::json!({"batch":"retry"}),
                "Remove a track".into(),
            )
            .unwrap();
        broker.claim_task(&request).unwrap().approve().unwrap();
    }
}

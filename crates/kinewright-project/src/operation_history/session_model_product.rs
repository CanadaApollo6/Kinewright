//! Native manual-stream lane of the generated session. The oracle keeps the
//! original logical commands and runs each directly against a reference owner.
use super::*;
use crate::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle},
    editing_session::{Ja1EditingSession, ManualAuthority},
    io_observer::fail_once,
    operation_actor::Core,
    operation_inputs::ManualInputs,
    operation_protocol::{
        EncodedMessage, Message as Wire, PENDING_COMMANDS, Producer, ProducerError,
    },
    receipts::{CommandId, IssuerId},
};
use kinewright_core::TimelineRevision;
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

struct Packet {
    wire: Wire,
    commands: Vec<Message>,
    exported: bool,
    attempted: bool,
}

#[derive(Default)]
pub(super) struct Counts {
    pub folds: usize,
    pub refusals: usize,
    pub issuer_refusals: usize,
    pub partial_refusals: usize,
    pub lost_acks: usize,
    pub retries: usize,
}

impl Counts {
    pub(super) fn accumulate(&mut self, other: &Self) {
        self.folds += other.folds;
        self.refusals += other.refusals;
        self.issuer_refusals += other.issuer_refusals;
        self.partial_refusals += other.partial_refusals;
        self.lost_acks += other.lost_acks;
        self.retries += other.retries;
    }
}

pub(super) struct ProductSession {
    writer: Ja1EditingSession,
    temp: TempDirectory,
    authority: ManualAuthority,
    producer: Producer,
    gui: OwnerHistory,
    acceptance: OwnerHistory,
    durable: OwnerHistory,
    outbox: VecDeque<Packet>,
    accepted_stream: StreamState,
    durable_stream: StreamState,
    published_generation: u64,
    pub counts: Counts,
}

impl ProductSession {
    pub(super) fn new() -> Self {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "hx-product-model");
        let core = Core::spawn_owner(
            reference_owner(),
            origin(1).branch,
            TimelineRevision::default(),
        )
        .unwrap();
        let mut writer = Ja1EditingSession::activate(
            &temp.path("project.kinewright"),
            BundleWriteAdmission::new_document(),
            &core,
            None,
        )
        .unwrap();
        writer.save_active().unwrap();
        let lease = writer
            .issue_manual_lease("generated-gui".into(), "generated-lease".into())
            .unwrap();
        let authority = writer.manual_authority(lease.clone()).unwrap();
        let accepted_stream = StreamState::installed(lease.state.seed().clone()).unwrap();
        assert_eq!(lease.state, accepted_stream, "HX_MODEL_STARTS_EMPTY");
        let mut gui = reference_owner();
        let producer = Producer::bind(lease, &mut gui).unwrap();
        let loaded = LoadedBundle::read(
            &temp.path("project.kinewright"),
            &HistoryPool::new(PoolLimits::default()),
        )
        .unwrap();
        let published_generation = loaded.root().unwrap().generation;
        Self {
            writer,
            temp,
            authority,
            producer,
            gui,
            acceptance: reference_owner(),
            durable: reference_owner(),
            outbox: VecDeque::new(),
            durable_stream: accepted_stream.clone(),
            accepted_stream,
            published_generation,
            counts: Counts::default(),
        }
    }

    pub(super) fn send(&mut self, mut command: Message) {
        // The same grammar also generates native consent and owned-resource
        // commands. Their existing owner/collection lane owns those routes.
        if command.approval.is_some()
            || command.transition.is_some()
            || matches!(&command.action, Action::Do { operations, .. } if ManualInputs::imports(operations))
        {
            self.assert_reopened();
            return;
        }
        command.creation.branch = self.authority.state.seed().branch.clone();
        let folds = self.outbox.back().is_some_and(|packet| {
            !packet.exported
                && matches!((&packet.wire.action, &command.action),
                    (Action::Do { gesture: Some(previous), .. }, Action::Do { gesture: Some(next), .. })
                    if previous == next
                        && self.acceptance.history().undo().back().and_then(Step::coalesce_key)
                            == Some(next))
        });
        let actual =
            self.producer
                .execute(&mut self.gui, command.action.clone(), command.context());
        if self.outbox.len() == PENDING_COMMANDS && !folds {
            assert!(
                matches!(
                    actual,
                    Err(ProducerError::Protocol(
                        crate::operation_protocol::Conflict::Backpressure
                    ))
                ),
                "HX_MODEL_FULL_OUTBOX_MUST_REFUSE: {actual:?}"
            );
        } else {
            let expected = command.apply(&mut self.acceptance);
            match (actual, expected) {
                (Ok(actual), Ok(expected)) => {
                    assert_eq!(actual, expected, "HX_MODEL_GUI_COMMAND_TRANSITION");
                    self.enqueue(command, folds, expected.accepted);
                }
                (Err(ProducerError::History(actual)), Err(expected)) => {
                    assert_eq!(actual, expected, "HX_MODEL_GUI_REFUSAL");
                }
                (actual, expected) => panic!("HX_MODEL_GUI_ACCEPTANCE: {actual:?} vs {expected:?}"),
            }
        }
        self.assert_producer();
        self.assert_reopened();
    }

    fn enqueue(&mut self, command: Message, folds: bool, first_key: CommandKey) {
        if folds {
            let packet = self.outbox.back_mut().unwrap();
            let Action::Do { operations, .. } = &command.action else {
                unreachable!("the expectation groups only hand edits");
            };
            packet.wire.folded.push(FoldedBatch {
                creation: command.creation.clone(),
                operations: operations.clone(),
            });
            packet.wire.tip = packet.wire.chained().unwrap();
            packet.commands.push(command);
            self.accepted_stream
                .replace_tail_receipt(expected_receipt(packet));
            self.counts.folds += 1;
        } else {
            let wire = Wire {
                publisher: self.writer.manual_publisher().unwrap(),
                id: CommandId {
                    issuer: self.authority.issuer.clone(),
                    sequence: self.accepted_stream.sequence() + 1,
                },
                sequence: self.accepted_stream.sequence() + 1,
                predecessor: self.accepted_stream.tip(),
                tip: [0; 32],
                accepted: first_key,
                action: command.action.clone(),
                context: command.context(),
                folded: vec![],
            };
            let packet = Packet {
                wire: Wire {
                    tip: wire.chained().unwrap(),
                    ..wire
                },
                commands: vec![command],
                exported: false,
                attempted: false,
            };
            self.accepted_stream
                .record_receipt(expected_receipt(&packet));
            self.outbox.push_back(packet);
        }
    }

    fn assert_producer(&self) {
        assert_eq!(
            self.producer.pending_count(),
            self.outbox.len(),
            "HX_MODEL_EXPORTED_PACKET_STAYS_IMMUTABLE"
        );
        assert_eq!(
            self.producer.state(),
            &self.accepted_stream,
            "HX_MODEL_PRODUCER_RECEIPTS_MATCH_ACCEPTED_COMMANDS"
        );
        assert_same_values(&self.acceptance, &self.gui);
    }

    pub(super) fn drain(&mut self) {
        if !self.outbox.is_empty() {
            self.group(PENDING_COMMANDS, GroupFault::None);
            assert!(self.outbox.is_empty(), "HX_MODEL_RETRY_DRAINS_OUTBOX");
        }
    }

    pub(super) fn group(&mut self, width: usize, fault: GroupFault) {
        let mut sent = self.producer.pending();
        assert_eq!(
            sent.len(),
            self.outbox.len(),
            "HX_MODEL_EXPORTS_THE_ENTIRE_OUTBOX"
        );
        for (wire, packet) in sent.iter().zip(&mut self.outbox) {
            assert_eq!(
                wire.decode().unwrap(),
                packet.wire,
                "HX_MODEL_EXPORTED_PACKET_STAYS_IMMUTABLE"
            );
            packet.exported = true;
        }
        let width = width.min(sent.len());
        if width == 0 {
            return;
        }
        let width = if let GroupFault::StopBeforeRoot(at) = fault {
            1 + at % width
        } else {
            width
        };
        sent.truncate(width);
        let already_saved = self
            .outbox
            .iter()
            .take_while(|packet| packet.wire.sequence <= self.durable_stream.sequence())
            .count();
        let refusal = adjust_request(&mut sent, fault, already_saved);
        let accepted = refusal.unwrap_or(width);
        let writes = accepted > already_saved;
        let io = match fault {
            GroupFault::StopBeforeRoot(_) if writes => Some("root_rename_io"),
            GroupFault::StopAfterRoot if writes => Some("root_parent_sync"),
            _ => None,
        };
        for packet in self.outbox.iter_mut().take(width) {
            self.counts.retries += usize::from(packet.attempted);
            packet.attempted = true;
        }
        self.counts.refusals += usize::from(refusal.is_some());
        self.counts.issuer_refusals +=
            usize::from(refusal.is_some() && matches!(fault, GroupFault::IssuerAt(_)));
        self.counts.partial_refusals +=
            usize::from(refusal.is_some() && matches!(fault, GroupFault::RefuseFoldedAt(_)));

        let received = self.publish(&sent, fault, refusal, io, accepted, writes);
        if !matches!(io, Some("root_rename_io")) {
            self.advance_durable(accepted);
        }
        self.assert_reopened();
        if io.is_some() {
            self.counts.lost_acks += usize::from(matches!(fault, GroupFault::StopAfterRoot));
            // The live writer owns the unresolved candidate. Resend the
            // original prefix before offering a distinct fault or fresh suffix.
            self.group(width, GroupFault::None);
        } else if matches!(fault, GroupFault::LoseAcknowledgement) {
            self.counts.lost_acks += 1;
        } else {
            self.producer.acknowledge(&received.unwrap()).unwrap();
            self.outbox.drain(..accepted);
        }
        self.assert_producer();
    }

    fn publish(
        &mut self,
        sent: &[EncodedMessage],
        fault: GroupFault,
        refusal: Option<usize>,
        io: Option<&'static str>,
        accepted: usize,
        writes: bool,
    ) -> Option<Vec<Receipt>> {
        let result = match io {
            Some(point) => fail_once(point, || {
                self.writer
                    .commit_manual_history_group(&self.authority, sent)
            }),
            None => self
                .writer
                .commit_manual_history_group(&self.authority, sent),
        };
        if io.is_some() {
            assert!(
                matches!(result, Err(BundleError::DurabilityUnknown)),
                "HX_MODEL_FAILED_PUBLICATION_RETURNS_NO_RECEIPTS"
            );
            None
        } else {
            let result = result.unwrap_or_else(|error| {
                assert!(
                    !matches!(fault, GroupFault::RefuseFoldedAt(_)),
                    "HX_MODEL_REFUSED_FOLD_PRESERVES_DURABLE_PREFIX: {error}"
                );
                panic!("HX_MODEL_GROUP_RETURNS_EXPECTED_PREFIX: {error}")
            });
            let expected: Vec<_> = self
                .outbox
                .iter()
                .take(accepted)
                .map(expected_receipt)
                .collect();
            assert_eq!(
                result.receipts, expected,
                "HX_MODEL_GROUP_ACKNOWLEDGES_ONLY_ACCEPTED_PREFIX"
            );
            assert_eq!(
                result.refusal.is_some(),
                refusal.is_some(),
                "HX_MODEL_GROUP_REFUSAL"
            );
            if refusal.is_some() && matches!(fault, GroupFault::IssuerAt(_)) {
                assert!(
                    matches!(result.refusal, Some(BundleError::HistoryChanged)),
                    "HX_MODEL_ISSUER_ADMISSION_REFUSES"
                );
            }
            assert_eq!(
                result.report.is_some(),
                writes,
                "HX_MODEL_GROUP_PUBLISHES_ONLY_ABSENT_PACKETS"
            );
            Some(result.receipts)
        }
    }

    fn advance_durable(&mut self, count: usize) {
        let before = self.durable_stream.sequence();
        for packet in self
            .outbox
            .iter()
            .take(count)
            .filter(|packet| packet.wire.sequence > before)
        {
            for command in &packet.commands {
                command
                    .apply(&mut self.durable)
                    .expect("HX_MODEL_REPLAYS_ACCEPTED_LOGICAL_COMMAND");
            }
            self.durable_stream.record_receipt(expected_receipt(packet));
        }
        self.published_generation += u64::from(self.durable_stream.sequence() != before);
    }

    pub(super) fn assert_reopened(&self) {
        let pool = HistoryPool::new(PoolLimits::default());
        let mut loaded = LoadedBundle::read(&self.temp.path("project.kinewright"), &pool)
            .expect("HX_MODEL_REOPENS_THE_PUBLISHED_ROOT");
        assert_eq!(
            loaded.root().unwrap().generation,
            self.published_generation,
            "HX_MODEL_ROOT_GENERATION"
        );
        assert_eq!(
            loaded.manual_checkpoint().unwrap().stream.as_ref(),
            Some(&self.durable_stream),
            "HX_MODEL_REOPENED_RECEIPT_WINDOW"
        );
        let reopened = loaded.open_manual().unwrap().export_history().unwrap();
        let expected = self.durable.capture().unwrap();
        assert_eq!(
            reopened.history().document,
            expected.history().document,
            "HX_MODEL_REOPENED_DOCUMENT"
        );
        assert_steps(
            reopened.history().undo.iter(),
            expected.history().undo.iter(),
            "undo",
        );
        assert_steps(
            reopened.history().redo.iter(),
            expected.history().redo.iter(),
            "redo",
        );
        assert_eq!(
            reopened.identity().unwrap(),
            expected.identity().unwrap(),
            "HX_MODEL_REOPENED_OWNER"
        );
    }
}

fn assert_steps<'a>(
    actual: impl ExactSizeIterator<Item = &'a Step>,
    expected: impl ExactSizeIterator<Item = &'a Step>,
    direction: &str,
) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "HX_MODEL_REOPENED_UNDO_BOUNDARIES {direction} depth"
    );
    for (actual, expected) in actual.zip(expected) {
        assert_eq!(
            (actual.coalesce_key(), actual.commands()),
            (expected.coalesce_key(), expected.commands()),
            "HX_MODEL_REOPENED_UNDO_BOUNDARIES {direction} commands"
        );
    }
}

fn reference_owner() -> OwnerHistory {
    fresh(&HistoryPool::new(PoolLimits::default()))
}

fn expected_receipt(packet: &Packet) -> Receipt {
    Receipt {
        id: packet.wire.id.clone(),
        sequence: packet.wire.sequence,
        accepted: packet.wire.accepted,
        tip: packet.wire.tip,
        command: packet.wire.digest().unwrap(),
        additional_batches: u64::try_from(packet.commands.len() - 1).unwrap(),
    }
}

fn adjust_request(
    sent: &mut [EncodedMessage],
    fault: GroupFault,
    already_saved: usize,
) -> Option<usize> {
    let index = match fault {
        GroupFault::RefuseAt(at) | GroupFault::RefuseFoldedAt(at) => at % sent.len(),
        GroupFault::IssuerAt(at) => (at % sent.len()).max(already_saved).max(1),
        _ => return None,
    };
    let mut refused = sent.get(index)?.decode().unwrap();
    match fault {
        GroupFault::RefuseAt(_) => refused.tip[0] ^= 1,
        GroupFault::IssuerAt(_) => {
            refused.id.issuer = IssuerId("f".repeat(32));
            refused.tip = refused.chained().unwrap();
        }
        GroupFault::RefuseFoldedAt(_) => {
            assert!(
                index >= already_saved,
                "HX_MODEL_PARTIAL_REFUSAL_NEEDS_FRESH_PACKET"
            );
            refused.folded.last_mut().unwrap().operations = vec![Operation::SetTrackSyncLock {
                track: TrackId(u64::MAX),
                locked: true,
            }];
            refused.tip = refused.chained().unwrap();
        }
        _ => unreachable!(),
    }
    sent[index] = serde_json::from_value(serde_json::Value::String(
        serde_json::to_string(&refused).unwrap(),
    ))
    .unwrap();
    Some(index)
}

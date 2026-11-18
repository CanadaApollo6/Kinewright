//! Actor-local permits fence every mutation; freezing keeps the exact outbox
//! readable so recovery can reconcile it before resuming another authority.
use super::{
    AcceptedCut, Arc, AtomicBool, Attachment, AttachmentState, BatchId, Command, CommandContext,
    CommandMessage, Core, CoreMutationRefusal, CreationOrigin, Effects, Error, HistoryError,
    Message, Notification, Ordering, Refusal, Sender, State, TypeId, command_token,
};
use crate::operation_protocol::{
    Action, Conflict, EncodedMessage, Lease, Producer, ProducerError, Receipt,
};

#[derive(Clone)]
pub struct ManualPermit {
    core: Core,
    generation: u64,
}
impl ManualPermit {
    /// Whether this permit still names the actor's installed command stream.
    pub fn is_current(&self) -> Result<bool, Error> {
        self.core
            .call(|reply| Message::CurrentProducer(self.generation, reply))
    }
    pub fn send(&self, command: Command) -> Result<(), Error> {
        self.core
            .sender
            .send(Message::Command(Box::new(CommandMessage {
                command,
                context: None,
                permit: Some(self.generation),
                reply: None,
            })))
            .map_err(|_| Error::Disconnected)
    }
    pub fn request(&self, command: Command) -> Notification {
        let token = command_token(&command);
        self.core
            .call(|reply| {
                Message::Command(Box::new(CommandMessage {
                    command,
                    context: None,
                    permit: Some(self.generation),
                    reply: Some(reply),
                }))
            })
            .map_err(|error| Refusal { error, token })?
    }
    pub fn close_gesture(&self) -> Result<Effects, Error> {
        self.core
            .call(|reply| Message::Close(Some(self.generation), reply))?
    }
    /// Export pending packets and freeze their wire identities for retry.
    pub fn pending(&self) -> Result<Vec<EncodedMessage>, Error> {
        self.core
            .call(|reply| Message::Pending(self.generation, reply))?
    }
    /// Observe the window without exporting any packets.
    pub fn pending_count(&self) -> Result<usize, Error> {
        self.core
            .call(|reply| Message::PendingCount(self.generation, reply))?
    }
}
pub(super) struct BoundProducer {
    pub stream: Producer,
    live: Arc<AtomicBool>,
    permit: u64,
    inputs: crate::operation_inputs::ManualInputs,
}
pub(super) struct BindRequest {
    owner: TypeId,
    cut: Option<AcceptedCut>,
    lease: Lease,
    inputs: crate::operation_inputs::ManualInputs,
    live: Arc<AtomicBool>,
    reply: Sender<Result<u64, Error>>,
}
impl<P: 'static> Attachment<P> {
    pub fn manual_permit(
        &self,
        _proof: &P,
        cut: Option<AcceptedCut>,
        lease: Lease,
        inputs: crate::operation_inputs::ManualInputs,
        live: Arc<AtomicBool>,
    ) -> Result<ManualPermit, Error> {
        let generation = self.core.call(|reply| {
            Message::Producer(Box::new(BindRequest {
                owner: TypeId::of::<P>(),
                cut,
                lease,
                inputs,
                live,
                reply,
            }))
        })??;
        Ok(ManualPermit {
            core: self.core.clone(),
            generation,
        })
    }
    pub(crate) fn acknowledge_frozen(
        &self,
        _proof: &P,
        cut: AcceptedCut,
        receipts: Vec<Receipt>,
    ) -> Result<(), Error> {
        self.core
            .call(|reply| Message::AcknowledgeFrozen(TypeId::of::<P>(), cut, receipts, reply))?
    }
    pub fn acknowledge_manual(
        &self,
        _proof: &P,
        permit: &ManualPermit,
        receipts: Vec<Receipt>,
    ) -> Result<(), Error> {
        if self.core.identity != permit.core.identity {
            return Err(Error::MutationRefused(CoreMutationRefusal::OwnerRequired));
        }
        self.core.call(|reply| {
            Message::Acknowledge(TypeId::of::<P>(), permit.generation, receipts, reply)
        })?
    }
}
impl State {
    pub(super) fn check_drained(&self) -> Result<(), Error> {
        if self
            .producer
            .as_ref()
            .is_some_and(|bound| bound.stream.pending_count() != 0)
        {
            return Err(Error::Protocol(Conflict::Backpressure));
        }
        Ok(())
    }

    pub(super) fn current_producer(&self, generation: u64) -> Result<&BoundProducer, Error> {
        self.producer
            .as_ref()
            .filter(|bound| bound.permit == generation)
            .ok_or(Error::MutationRefused(
                CoreMutationRefusal::ManualPermitRequired,
            ))
    }
    pub(super) fn current_producer_mut(
        &mut self,
        generation: u64,
    ) -> Result<&mut BoundProducer, Error> {
        self.producer
            .as_mut()
            .filter(|bound| bound.permit == generation)
            .ok_or(Error::MutationRefused(
                CoreMutationRefusal::ManualPermitRequired,
            ))
    }
    pub(super) fn check_permit(&self, permit: Option<u64>) -> Result<(), Error> {
        match (&self.producer, permit) {
            // Revoking an installed stream must not turn escaped raw handles
            // into producers. Once manual authority existed, only a new permit
            // may mutate this actor, even during continuity re-acquisition.
            (None, None) if self.permit_generation == 0 => Ok(()),
            (Some(bound), Some(generation))
                if bound.permit == generation && bound.live.load(Ordering::Acquire) =>
            {
                Ok(())
            }
            _ => Err(Error::MutationRefused(
                CoreMutationRefusal::ManualPermitRequired,
            )),
        }
    }
    fn bind_stream(&mut self, request: &BindRequest) -> Result<(), Error> {
        self.authorize(request.owner)?;
        match request.cut {
            Some(cut) => self.check_cut(request.owner, cut)?,
            None if self.attachment == AttachmentState::Live && self.producer.is_none() => {}
            _ => return Err(Error::CaptureChanged),
        }
        if request.inputs.reference() != &request.lease.inputs {
            return Err(Error::History(HistoryError::InputRequired));
        }
        if !request.live.load(Ordering::Acquire) || request.lease.state.seed().branch != self.branch
        {
            return Err(Error::Protocol(Conflict::Detached));
        }
        Ok(())
    }
    pub(super) fn bind_producer(&mut self, request: BindRequest) {
        let checked = self.bind_stream(&request);
        let BindRequest {
            lease,
            inputs,
            live,
            reply,
            ..
        } = request;
        let result = checked.and_then(|()| {
            self.history
                .as_mut()
                .ok_or(Error::Retired)?
                .promote_live()?;
            let generation = self
                .permit_generation
                .checked_add(1)
                .ok_or(Error::CounterExhausted)?;
            let owner = self.history.as_mut().ok_or(Error::Retired)?;
            if let Some(bound) = &mut self.producer {
                bound.stream.rebind(lease, owner).map_err(producer_error)?;
                bound.live = live;
                bound.inputs = inputs;
                bound.permit = generation;
            } else {
                let stream = Producer::bind(lease, owner).map_err(producer_error)?;
                self.producer = Some(BoundProducer {
                    stream,
                    live,
                    permit: generation,
                    inputs,
                });
            }
            self.permit_generation = generation;
            self.attachment = AttachmentState::Live;
            Ok(generation)
        });
        let _ = reply.send(result);
    }
    pub(super) fn acknowledge(
        &mut self,
        owner: TypeId,
        generation: u64,
        receipts: &[Receipt],
    ) -> Result<(), Error> {
        self.authorize(owner)?;
        self.current_producer_mut(generation)?
            .stream
            .acknowledge(receipts)
            .map_err(Error::Protocol)
    }
    pub(super) fn default_context(&self) -> Result<CommandContext, Error> {
        Ok(CommandContext {
            creation: CreationOrigin {
                branch: self.branch.clone(),
                batch: BatchId::fresh().map_err(|error| Error::System(error.to_string()))?,
            },
            inputs: vec![],
            approval: None,
            transition: None,
        })
    }
    pub(super) fn apply_action(
        &mut self,
        action: Action,
        context: CommandContext,
    ) -> Result<Effects, Error> {
        let owner = self.history.as_mut().ok_or(Error::Retired)?;
        match &mut self.producer {
            Some(bound) => bound
                .stream
                .execute_with_inputs(owner, action, context, Some(&bound.inputs))
                .map_err(producer_error),
            None => action.execute(context, owner).map_err(Error::from),
        }
    }
}
fn producer_error(error: ProducerError) -> Error {
    match error {
        ProducerError::Protocol(Conflict::CommandTooLarge) => {
            Error::MutationRefused(CoreMutationRefusal::ManualHistoryCapacity)
        }
        ProducerError::Protocol(error) => Error::Protocol(error),
        ProducerError::History(HistoryError::InputRequired) => {
            Error::MutationRefused(CoreMutationRefusal::ManualInputRequired)
        }
        ProducerError::History(HistoryError::History(
            kinewright_core::undo::Error::StepTooLarge
            | kinewright_core::undo::Error::HistoryFull
            | kinewright_core::undo::Error::Capacity
            | kinewright_core::undo::Error::DocumentCapacity,
        )) => Error::MutationRefused(CoreMutationRefusal::ManualHistoryCapacity),
        ProducerError::History(error) => Error::History(error),
    }
}

//! A bounded portable capture for GUI/writer adoption. The sender retains the
//! capture's full lease until its last transport holder drops. The receiver
//! admits its own fixed lease before decoding or replaying the raw body.
use super::{
    CreationSizing, Error, HistoryPool, HistoryReservation, OwnerSnapshot, Role, SnapshotIdentity,
    ValidatedOwnerSnapshot, snapshot::SnapshotData,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
const HEADER_BYTES: u64 = 4096;

fn json_string_bytes(body: &RawValue) -> u64 {
    let raw = body.get();
    // Inside valid raw JSON, the only literal control bytes are JSON
    // whitespace. Like quotes and backslashes, tab, LF and CR expand to
    // two bytes. A control such as NUL already has a textual escape.
    let extra = raw
        .bytes()
        .filter(|byte| matches!(byte, b'"' | b'\\' | b'\t' | b'\n' | b'\r'))
        .count();
    raw.len() as u64 + extra as u64 + 2
}

fn encode(data: &SnapshotData, limit: u64) -> Result<Box<RawValue>, Error> {
    struct Writer {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let end = self
                .bytes
                .len()
                .checked_add(bytes.len())
                .filter(|end| *end <= self.limit)
                .ok_or_else(|| std::io::Error::other("snapshot transport capacity"))?;
            if end > self.bytes.capacity() {
                let goal = end
                    .max(self.bytes.capacity().saturating_mul(2))
                    .min(self.limit);
                self.bytes
                    .try_reserve_exact(goal - self.bytes.len())
                    .map_err(std::io::Error::other)?;
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        bytes: Vec::new(),
        limit: usize::try_from(limit).map_err(|_| Error::Capacity("snapshot transport"))?,
    };
    serde_json::to_writer(&mut writer, data).map_err(|_| Error::Capacity("snapshot transport"))?;
    let encoded = String::from_utf8(writer.bytes).map_err(|_| Error::InvalidRecord)?;
    RawValue::from_string(encoded).map_err(|_| Error::InvalidRecord)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotTransport {
    role: Role,
    sizing: CreationSizing,
    identity: SnapshotIdentity,
    #[serde(with = "crate::operation_wire")]
    body: Box<RawValue>,
    #[serde(skip)]
    reservation: Option<HistoryReservation>,
}
impl OwnerSnapshot {
    pub fn into_transport(self) -> Result<SnapshotTransport, Error> {
        let window = self.reservation.workspace()?;
        let identity = self.identity()?;
        let limit = self
            .reservation
            .limits()
            .envelope
            .retained
            .min(self.reservation.limits().envelope.workspace / 2)
            .checked_sub(HEADER_BYTES)
            .ok_or(Error::Capacity("snapshot transport"))?;
        let body = encode(&self.data, limit)?;
        let Self { data, reservation } = self;
        let role = data.role;
        let sizing = data.sizing;
        drop(data);
        drop(window);
        Ok(SnapshotTransport {
            role,
            sizing,
            identity,
            body,
            reservation: Some(reservation),
        })
    }
}
impl std::fmt::Debug for SnapshotTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotTransport")
            .field("role", &self.role)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}
impl SnapshotTransport {
    /// Exact bytes of a previously validated retained transport can be used
    /// again without decoding a second owner. Identity alone is not this proof.
    #[must_use]
    pub fn same_encoded_state(&self, other: &Self) -> bool {
        self.role == other.role
            && self.sizing == other.sizing
            && self.identity == other.identity
            && self.body.get() == other.body.get()
    }
    /// The raw JSON bytes of the body, before it travels as a JSON string.
    #[must_use]
    pub fn body_bytes(&self) -> u64 {
        self.body.get().len() as u64
    }
    /// Bytes of the body encoded as a JSON string, including both quotes.
    #[must_use]
    pub fn escaped_body_bytes(&self) -> u64 {
        json_string_bytes(&self.body)
    }
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }
    #[must_use]
    pub const fn identity(&self) -> SnapshotIdentity {
        self.identity
    }
    /// The declared identity is a precheck only; this is the sole constructor
    /// of the installation token and always performs full semantic validation.
    pub fn validate(
        self,
        pool: &HistoryPool,
        expected: Role,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        if self.role != expected || self.sizing.kind.role() != expected {
            return Err(Error::InvalidPolicy);
        }
        let limits = self.sizing.limits()?;
        let bytes = self.body.get().len() as u64;
        if bytes
            .checked_add(HEADER_BYTES)
            .is_none_or(|n| n > limits.envelope.retained)
        {
            return Err(Error::Capacity("snapshot transport"));
        }
        let Self {
            body,
            identity,
            role,
            sizing,
            reservation,
        } = self;
        let reservation = match reservation {
            Some(reservation) if reservation.pool().same_pool(pool) => reservation,
            sender => {
                // A locally forwarded sender may still own its raw buffer in
                // another process model's pool. Admit the receiver first.
                let admitted = pool.admit_sized(sizing)?;
                drop(sender);
                admitted
            }
        };
        let window = reservation.workspace()?;
        let snapshot = Self::decode_body(&body, role, sizing, reservation, window)?;
        drop(body);
        Self::validate_decoded(snapshot, identity)
    }
    pub fn receive(
        self: std::sync::Arc<Self>,
        pool: &HistoryPool,
        expected: Role,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        match std::sync::Arc::try_unwrap(self) {
            Ok(transport) => transport.validate(pool, expected),
            Err(shared) => shared.validate_shared(pool, expected),
        }
    }
    pub fn validate_shared(
        &self,
        pool: &HistoryPool,
        expected: Role,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        if self.role != expected || self.sizing.kind.role() != expected {
            return Err(Error::InvalidPolicy);
        }
        let reservation = pool.admit_sized(self.sizing)?;
        let window = reservation.workspace()?;
        let snapshot = Self::decode_body(&self.body, self.role, self.sizing, reservation, window)?;
        Self::validate_decoded(snapshot, self.identity)
    }
    fn decode_body(
        body: &RawValue,
        role: Role,
        sizing: CreationSizing,
        reservation: HistoryReservation,
        window: super::reservation::Workspace,
    ) -> Result<OwnerSnapshot, Error> {
        let limits = sizing.limits()?;
        let bytes = body.get().len() as u64;
        if bytes
            .checked_add(HEADER_BYTES)
            .is_none_or(|n| n > limits.envelope.retained)
        {
            return Err(Error::Capacity("snapshot transport"));
        }
        // This frame includes both documents, unlike an operation payload.
        // Use the existing JSON preflight against the already admitted shared
        // workspace, with its raw frame subtracted. The operation payload cap
        // and all document/history role limits are unchanged.
        let ingress = crate::storage_budget::StorageLimits {
            decoded_record_bytes: limits
                .envelope
                .workspace
                .checked_sub(bytes)
                .ok_or(Error::Capacity("snapshot transport decode"))?,
            ..crate::storage_budget::StorageLimits::default()
        };
        crate::store::preflight_record(body.get().as_bytes(), &ingress)
            .map_err(|_| Error::Capacity("snapshot transport decode"))?;
        let data: SnapshotData =
            serde_json::from_str(body.get()).map_err(|_| Error::InvalidRecord)?;
        if data.role != role || data.sizing != sizing {
            return Err(Error::InvalidRecord);
        }
        drop(window);
        Ok(OwnerSnapshot { data, reservation })
    }
    fn validate_decoded(
        snapshot: OwnerSnapshot,
        identity: SnapshotIdentity,
    ) -> Result<ValidatedOwnerSnapshot, Error> {
        let validated = snapshot.validate()?;
        if validated.identity() != identity {
            return Err(Error::IdentityMismatch);
        }
        Ok(validated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_history::{
        OwnerHistory, PoolLimits,
        tests::{document, origin},
    };
    use kinewright_core::{Command, Operation, TrackId};

    proptest::proptest! {
        #[test]
        fn escaped_snapshot_body_bytes_match_serde_json(text in proptest::arbitrary::any::<String>()) {
            let controls: String = (0_u8..32).map(char::from).chain(['"', '\\']).collect();
            let raw = RawValue::from_string(format!(
                "[\t\n\r{},{}]",
                serde_json::to_string(&controls).unwrap(),
                serde_json::to_string(&text).unwrap(),
            )).unwrap();
            proptest::prop_assert_eq!(
                json_string_bytes(&raw),
                serde_json::to_string(raw.get()).unwrap().len() as u64
            );
        }
    }

    #[test]
    fn portable_capture_validates_both_owner_and_core_before_install() {
        let source_pool = HistoryPool::new(PoolLimits::default());
        let target_pool = HistoryPool::new(PoolLimits::default());
        let mut source = OwnerHistory::new(
            &source_pool,
            Role::Task,
            Role::Task.policy(),
            &origin(1),
            || Ok(document()),
        )
        .unwrap();
        source
            .execute(
                Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(81),
                    locked: true,
                }),
                origin(2),
                vec![],
                None,
            )
            .unwrap();
        source
            .execute(Command::Undo, origin(3), vec![], None)
            .unwrap();
        let expected = source.identity().unwrap();
        let wire = source.capture().unwrap().into_transport().unwrap();
        let encoded = serde_json::to_vec(&wire).unwrap();
        assert_eq!(
            source_pool.lease_counts(Role::Task).unwrap().0,
            2,
            "PORTABLE_SENDER_RETAINS_ITS_CAPTURE_LEASE"
        );
        let received: SnapshotTransport = serde_json::from_slice(&encoded).unwrap();
        let opened = received
            .validate(&target_pool, Role::Task)
            .unwrap()
            .open()
            .unwrap();
        assert_eq!(
            opened.identity().unwrap(),
            expected,
            "PORTABLE_CAPTURE_REPLAYS_THE_COMBINED_OWNER_IDENTITY"
        );
        assert_eq!(
            opened.history().redo().len(),
            1,
            "PORTABLE_CAPTURE_PRESERVES_THE_REDO_TAIL"
        );
        let mut corrupt: SnapshotTransport = serde_json::from_slice(&encoded).unwrap();
        corrupt.identity.owner[0] ^= 1;
        assert!(
            matches!(
                corrupt.validate(&target_pool, Role::Task),
                Err(Error::IdentityMismatch)
            ),
            "PORTABLE_CAPTURE_REFUSES_A_FORGED_OWNER_IDENTITY"
        );
        drop((source, wire));
        assert_eq!(
            source_pool.usage(Role::Task).unwrap(),
            (0, 0),
            "LAST_PORTABLE_SENDER_RELEASES_THE_ORIGINAL_CAPTURE_LEASE"
        );
    }
    #[test]
    fn portable_capture_opens_the_twenty_thousand_clip_product_fixture() {
        let source_pool = HistoryPool::new(PoolLimits::default());
        let target_pool = HistoryPool::new(PoolLimits::default());
        let source = OwnerHistory::new(
            &source_pool,
            Role::Task,
            Role::Task.policy(),
            &origin(1),
            || Ok(super::super::sizing_tests::fixture::document(20_000, 8, 90)),
        )
        .unwrap();
        let expected = source.identity().unwrap();
        let wire = source.capture().unwrap().into_transport().unwrap();
        let encoded = serde_json::to_vec(&wire).unwrap();
        let received: SnapshotTransport = serde_json::from_slice(&encoded).unwrap();
        let opened = received
            .validate(&target_pool, Role::Task)
            .expect("MAXIMAL_PROJECT_CAPTURE_MUST_OPEN_THROUGH_THE_PORTABLE_DOOR")
            .open()
            .unwrap();
        assert_eq!(
            opened.identity().unwrap(),
            expected,
            "MAXIMAL_PORTABLE_CAPTURE_PRESERVES_COMBINED_IDENTITY"
        );
    }
}

#[cfg(test)]
mod operation_cases {
    use super::*;
    use crate::operation_history::{OwnerHistory, PoolLimits, tests::origin};
    use kinewright_core::{Command, Document, Operation};

    #[test]
    fn every_operation_kind_roundtrips_through_the_portable_capture() {
        let cases: serde_json::Map<String, serde_json::Value> = serde_json::from_str(include_str!(
            "../../../kinewright-core/tests/support/history_operation_cases.json"
        ))
        .unwrap();
        assert_eq!(cases.len(), 67);
        for (name, mut case) in cases {
            let source_pool = HistoryPool::new(PoolLimits::default());
            let target_pool = HistoryPool::new(PoolLimits::default());
            let document: Document = serde_json::from_value(case["document"].take()).unwrap();
            let operation: Operation = serde_json::from_value(case["operation"].take()).unwrap();
            let mut source = OwnerHistory::new(
                &source_pool,
                Role::Task,
                Role::Task.policy(),
                &origin(1),
                || Ok(document),
            )
            .unwrap();
            source
                .execute(Command::Do(operation), origin(2), vec![], None)
                .unwrap();
            source
                .execute(Command::Undo, origin(3), vec![], None)
                .unwrap();
            let expected = source.identity().unwrap();
            let sender = source.capture().unwrap().into_transport().unwrap();
            let encoded = serde_json::to_vec(&sender).unwrap();
            let received: SnapshotTransport = serde_json::from_slice(&encoded).unwrap();
            let opened = received
                .validate(&target_pool, Role::Task)
                .unwrap_or_else(|error| {
                    panic!("PORTABLE_OPERATION_CAPTURE_MUST_OPEN {name}: {error}")
                })
                .open()
                .unwrap();
            assert_eq!(
                opened.identity().unwrap(),
                expected,
                "PORTABLE_OPERATION_CAPTURE_PRESERVES_BOTH_STACKS_AND_OWNERS {name}"
            );
        }
    }
}

#[cfg(test)]
mod tagged_command_tests {
    use super::*;
    #[test]
    fn portable_capture_roundtrips_inside_an_internally_tagged_origin() {
        #[derive(Serialize, Deserialize)]
        #[serde(tag = "origin")]
        enum Envelope {
            Gui { snapshot: SnapshotTransport },
        }
        let pool = HistoryPool::new(crate::operation_history::PoolLimits::default());
        let source = super::super::OwnerHistory::new(
            &pool,
            Role::Ordinary,
            Role::Ordinary.policy(),
            &super::super::tests::origin(1),
            || Ok(super::super::tests::document()),
        )
        .unwrap();
        let expected = source.identity().unwrap();
        let wire = serde_json::to_vec(&Envelope::Gui {
            snapshot: source.capture().unwrap().into_transport().unwrap(),
        })
        .unwrap();
        let Envelope::Gui { snapshot } = serde_json::from_slice(&wire)
            .expect("PORTABLE_CAPTURE_DECODES_INSIDE_TAGGED_GUI_ORIGIN");
        let restored = snapshot
            .validate(
                &HistoryPool::new(crate::operation_history::PoolLimits::default()),
                Role::Ordinary,
            )
            .unwrap();
        assert_eq!(
            restored.identity(),
            expected,
            "TAGGED_ORIGIN_PRESERVES_COMBINED_OWNER_IDENTITY"
        );
    }
}

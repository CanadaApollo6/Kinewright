//! A full manual window crosses the real HTTP ingress; only the codec refuses more.
//!
//! Each publication re-encodes and syncs the whole history (ticket HX-G1), so
//! no witness here publishes 64 messages over a 2 MiB history. The size gate
//! sends the largest admissible body without a lease; the committed witnesses
//! fill the byte limit with three messages or the count limit with small ones.
use super::*;
use kinewright_agent::writer_host::{
    COMMIT_HISTORY_BODY_BYTES, COMMIT_HISTORY_WINDOW_BYTES, HOST_ENVELOPE_BYTES, HostCommand,
};
use kinewright_project::operation_protocol::{EncodedMessage, PENDING_BYTES, PENDING_COMMANDS};

/// Each quote in a label is two bytes of raw JSON and four once the message
/// travels as a JSON string, so these labels are the worst-case expansion.
/// Two such edits sit just under the 1 MiB step cap.
const QUOTES: usize = 500_000;
/// The edits that fill the byte limit: two near the step cap and the rest.
const BYTE_EDITS: usize = 3;
/// The old `CommitHistory` ingress cap, before it was derived from the window.
const OLD_INGRESS_CAP: usize = 512 * 1024;

fn marker(id: u64, quotes: usize, plain: usize) -> Command {
    let mut label = "\"".repeat(quotes);
    label.push_str(&"a".repeat(plain));
    Command::Do(Operation::AddMarker {
        marker: Marker {
            id: MarkerId(id),
            position: TimeCode(i64::try_from(id).unwrap()),
            label,
            color_token: 0,
        },
    })
}
fn window(f: &Fixture) -> Vec<EncodedMessage> {
    f.gate().permit.pending().unwrap()
}
fn window_bytes(messages: &[EncodedMessage]) -> u64 {
    messages.iter().map(EncodedMessage::byte_len).sum()
}
fn send(f: &mut Fixture, command: Command) {
    f.app.focused_mut().send_manual_command(command).unwrap();
    f.attempt();
}
/// The encoded width of one 32-byte hash field of a held message.
fn hash_width(message: &EncodedMessage, field: &str) -> u64 {
    let raw = serde_json::to_value(message).unwrap();
    let wire: serde_json::Value = serde_json::from_str(raw.as_str().unwrap()).unwrap();
    serde_json::to_vec(&wire[field]).unwrap().len() as u64
}
/// A message's own `tip` is the hash of its content, written as 32 decimal
/// bytes, so its width is known only after encoding: 63 to 127 bytes.
const TIP_MIN: u64 = 32 + 31;
const TIP_MAX: u64 = 3 * 32 + 31;
/// Fill all but the last of `BYTE_EDITS` with held edits, then size the last
/// edit for the widest (`Fit`) or narrowest (`Over`) tip it can have.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Last {
    /// The largest edit that always fits: the window ends within 64 bytes of
    /// `PENDING_BYTES`, never past it.
    Fit,
    /// The smallest edit that never fits: at least one byte past the window.
    Over,
}
fn fill_to_limit(f: &mut Fixture, last_edit: Last) -> u64 {
    assert!(window(f).is_empty(), "MANUAL_WINDOW_STARTS_EMPTY");
    let first = 1_000_u64;
    let last = first + BYTE_EDITS as u64 - 1;
    for id in first..last {
        send(f, marker(id, QUOTES, 0));
    }
    let held = window(f);
    assert_eq!(held.len(), BYTE_EDITS - 1, "MANUAL_WINDOW_HELD");
    // The last message differs from its predecessor only in its label, its
    // predecessor hash and its own tip. Its predecessor is the previous tip, so
    // that width carries over. Ids, positions and sequence numbers keep their
    // digit counts. `fixed` is everything except the label and the new tip.
    let previous = held.last().unwrap();
    let fixed = previous.byte_len() - 2 * QUOTES as u64 - hash_width(previous, "predecessor");
    let remaining = PENDING_BYTES - window_bytes(&held);
    let need = match last_edit {
        Last::Fit => remaining - fixed - TIP_MAX,
        Last::Over => remaining - fixed - TIP_MIN + 1,
    };
    send(
        f,
        marker(
            last,
            usize::try_from(need / 2).unwrap(),
            usize::try_from(need % 2).unwrap(),
        ),
    );
    remaining
}
/// Wait until the held window drains. A refused request would freeze the
/// attachment into recovery, which also empties the window, so every poll
/// checks that the window drained through acknowledgement instead.
fn until_acknowledged(f: &mut Fixture, drained: impl Fn(&Fixture) -> bool) {
    f.until(|f| {
        assert!(
            !f.attachment().frozen,
            "MANUAL_WINDOW_NOT_RECOVERED status={}",
            f.app.status
        );
        drained(f)
    });
}
fn assert_cold_document(f: &mut Fixture) {
    let gui = Arc::clone(&f.app.focused().document);
    let path = f.temp.path("p.kinewright");
    f.app.focused_mut().ja1.as_mut().unwrap().close();
    f.app.focused_mut().ja1 = None;
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let cold = loaded.open_manual().unwrap();
    assert_eq!(
        *cold.export_history().unwrap().history().document,
        *gui,
        "MANUAL_FULL_WINDOW_COLD_DOCUMENT"
    );
}
/// A forged window of `total` raw bytes in `PENDING_COMMANDS` messages, built
/// entirely from the worst-case escaping: each message is a JSON string of
/// `\"` pairs, padded with one plain byte when its count is odd.
fn forged_window(total: u64) -> Vec<EncodedMessage> {
    let share = total / PENDING_COMMANDS as u64;
    let mut forged = Vec::with_capacity(PENDING_COMMANDS);
    for index in 0..PENDING_COMMANDS as u64 {
        let raw_bytes = if index == 0 {
            total - share * (PENDING_COMMANDS as u64 - 1)
        } else {
            share
        };
        let pairs = (raw_bytes - 2) / 2;
        let mut raw = String::from("\"");
        raw.push_str(&"\\\"".repeat(usize::try_from(pairs).unwrap()));
        raw.push_str(&"a".repeat(usize::try_from((raw_bytes - 2) % 2).unwrap()));
        raw.push('"');
        forged.push(serde_json::from_value(serde_json::Value::String(raw)).unwrap());
    }
    assert_eq!(window_bytes(&forged), total, "MANUAL_FORGED_WINDOW_BYTES");
    forged
}

#[test]
fn manual_largest_window_crosses_http_and_the_codec_to_the_lease_check() {
    let mut f = Fixture::new();
    let host = f.attachment().host.as_ref().unwrap();
    // Same width as the GUI's manual client, but not the one holding the lease.
    let leased = host.manual_identity_for_test();
    let stranger: String = leased
        .chars()
        .map(|c| if c == '0' { '1' } else { '0' })
        .collect();
    assert_ne!(stranger, leased, "MANUAL_SIZE_GATE_CLIENT_HOLDS_NO_LEASE");
    let command = HostCommand::CommitHistory {
        client: stranger,
        messages: forged_window(PENDING_BYTES),
    };
    let encoded = serde_json::to_vec(&command).unwrap().len();
    println!(
        "MANUAL_WORST_EXPANSION raw={PENDING_BYTES} command={encoded} per_10000={}",
        encoded as u64 * 10_000 / PENDING_BYTES
    );
    assert!(encoded > OLD_INGRESS_CAP, "MANUAL_WINDOW_EXCEEDS_OLD_CAP");
    assert!(
        encoded as u64 <= COMMIT_HISTORY_BODY_BYTES,
        "MANUAL_WORST_WINDOW_FITS_INGRESS_CAP encoded={encoded}"
    );
    let reply = host.client.request(command).unwrap();
    assert_eq!(
        reply.error.as_ref().map(|error| error.code.as_str()),
        Some("manual_lease"),
        "MANUAL_LARGEST_WINDOW_REACHES_LEASE_CHECK {:?}",
        reply.error
    );
    assert!(
        reply.manual_ack.is_empty(),
        "MANUAL_SIZE_GATE_COMMITS_NOTHING"
    );
    f.finish();
}

#[test]
fn manual_byte_limit_window_crosses_http_and_reopens() {
    let mut f = Fixture::new();
    fill_to_limit(&mut f, Last::Fit);
    let held = window(&f);
    assert_eq!(held.len(), BYTE_EDITS, "MANUAL_WINDOW_FULL_COUNT");
    let bytes = window_bytes(&held);
    println!(
        "MANUAL_WINDOW_HELD bytes={bytes} below_limit={}",
        PENDING_BYTES - bytes
    );
    assert!(
        bytes <= PENDING_BYTES && PENDING_BYTES - bytes <= TIP_MAX - TIP_MIN,
        "MANUAL_WINDOW_AT_LIMIT {bytes}"
    );
    let command = serde_json::to_vec(&HostCommand::CommitHistory {
        client: f
            .attachment()
            .host
            .as_ref()
            .unwrap()
            .manual_identity_for_test(),
        messages: held,
    })
    .unwrap()
    .len();
    println!(
        "MANUAL_WINDOW_EXPANSION raw={bytes} command={command} per_10000={}",
        command as u64 * 10_000 / bytes
    );
    assert!(command > OLD_INGRESS_CAP, "MANUAL_WINDOW_EXCEEDS_OLD_CAP");
    assert!(
        (command + HOST_ENVELOPE_BYTES) as u64 <= COMMIT_HISTORY_BODY_BYTES,
        "MANUAL_WINDOW_FITS_INGRESS_CAP"
    );
    f.release();
    until_acknowledged(&mut f, |f| {
        window(f).is_empty() && !f.edits().attempt_running()
    });
    assert!(
        f.edits().notice().is_none(),
        "MANUAL_FULL_WINDOW_ACKNOWLEDGED {:?}",
        f.edits().notice()
    );
    assert_eq!(
        f.app.focused().document.markers.len(),
        BYTE_EDITS,
        "MANUAL_FULL_WINDOW_ON_SCREEN"
    );
    f.finish();
    assert_cold_document(&mut f);
}

#[test]
fn manual_count_limit_window_is_acknowledged_and_reopens() {
    let mut f = Fixture::new();
    for id in 1_000..1_000 + PENDING_COMMANDS as u64 {
        send(&mut f, marker(id, 0, 8));
    }
    assert_eq!(
        window(&f).len(),
        PENDING_COMMANDS,
        "MANUAL_WINDOW_FULL_COUNT"
    );
    f.release();
    until_acknowledged(&mut f, |f| {
        window(f).is_empty() && !f.edits().attempt_running()
    });
    assert!(
        f.edits().notice().is_none(),
        "MANUAL_COUNT_WINDOW_ACKNOWLEDGED {:?}",
        f.edits().notice()
    );
    assert_eq!(
        f.app.focused().document.markers.len(),
        PENDING_COMMANDS,
        "MANUAL_COUNT_WINDOW_ON_SCREEN"
    );
    f.finish();
    assert_cold_document(&mut f);
}

#[test]
fn manual_window_one_byte_over_is_refused_by_the_codec_not_http() {
    let mut f = Fixture::new();
    let remaining = fill_to_limit(&mut f, Last::Over);
    let held = window(&f);
    assert_eq!(held.len(), BYTE_EDITS - 1, "MANUAL_OVER_LIMIT_NOT_ENCODED");
    assert_eq!(
        PENDING_BYTES - window_bytes(&held),
        remaining,
        "MANUAL_OVER_LIMIT_WINDOW_UNCHANGED"
    );
    assert!(
        f.attachment().manual_edits_pending(),
        "MANUAL_OVER_LIMIT_EDIT_WAITS"
    );
    // A forged window one byte over, built entirely from the worst-case
    // escaping, reaches the host's codec check through real HTTP.
    let host = f.attachment().host.as_ref().unwrap();
    let command = HostCommand::CommitHistory {
        client: host.manual_identity_for_test(),
        messages: forged_window(PENDING_BYTES + 1),
    };
    let encoded = serde_json::to_vec(&command).unwrap().len();
    // Past the window term, inside the envelope allowance: the reply below
    // shows HTTP admitted it and the codec refused it.
    assert!(
        encoded as u64 > COMMIT_HISTORY_WINDOW_BYTES
            && (encoded as u64) < COMMIT_HISTORY_BODY_BYTES,
        "MANUAL_WORST_WINDOW_WITHIN_INGRESS_CAP encoded={encoded} cap={COMMIT_HISTORY_BODY_BYTES}"
    );
    let reply = host.client.request(command).unwrap();
    assert_eq!(
        reply.error.as_ref().map(|error| error.code.as_str()),
        Some("manual_capacity"),
        "MANUAL_OVER_LIMIT_REFUSED_BY_CODEC"
    );
    f.release();
    until_acknowledged(&mut f, |f| {
        !f.attachment().manual_edits_pending() && !f.edits().attempt_running()
    });
    assert!(
        f.edits().notice().is_none(),
        "MANUAL_WAITING_EDIT_LANDS {:?}",
        f.edits().notice()
    );
    assert_eq!(
        f.app.focused().document.markers.len(),
        BYTE_EDITS,
        "MANUAL_EVERY_EDIT_ON_SCREEN"
    );
    f.finish();
    assert_cold_document(&mut f);
}

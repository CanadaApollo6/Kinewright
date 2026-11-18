use kinewright_core::{Command, Marker, MarkerId, Operation, TimeCode};

pub(super) struct Random(pub u64);
impl Random {
    pub(super) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub(super) fn index(&mut self, length: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(length).unwrap()).unwrap()
    }
}

pub(super) fn marker(id: u64) -> Operation {
    Operation::AddMarker {
        marker: Marker {
            id: MarkerId(id),
            position: TimeCode(0),
            label: format!("marker {id}"),
            color_token: 0,
        },
    }
}

pub(super) fn command(random: &mut Random, operations: &[&Operation], index: usize) -> Command {
    let operation = if random.next().is_multiple_of(3) {
        marker(10_000 + u64::try_from(index).unwrap())
    } else {
        (*operations[random.index(operations.len())]).clone()
    };
    match random.next() % 14 {
        0 => Command::Undo,
        1 => Command::Redo,
        2 => Command::Do(operation),
        3 => Command::DoBatch(vec![
            operation,
            marker(20_000 + u64::try_from(index).unwrap()),
        ]),
        4..=6 => Command::DoBatchCoalesced {
            operations: vec![operation],
            coalesce_key: "drag-a".into(),
        },
        7 => Command::DoBatchCoalesced {
            operations: vec![operation],
            coalesce_key: "drag-b".into(),
        },
        8 => Command::DoBatch(vec![]),
        9 => Command::DoBatchCoalesced {
            operations: vec![],
            coalesce_key: "drag-a".into(),
        },
        10 => Command::DoBatch(vec![
            operation,
            Operation::RemoveMarker {
                marker: MarkerId(u64::MAX),
            },
        ]),
        12 => Command::DoIfRevision {
            expected: kinewright_core::TimelineRevision(0),
            operation,
            token: None,
        },
        13 => Command::DoBatchIfRevision {
            expected: kinewright_core::TimelineRevision(0),
            operations: vec![operation],
        },
        _ => Command::Do(Operation::RemoveMarker {
            marker: MarkerId(u64::MAX),
        }),
    }
}

pub(super) fn initial_commands(first: Command) -> [Command; 12] {
    [
        Command::Undo,
        Command::Redo,
        first,
        Command::DoIfRevision {
            expected: kinewright_core::TimelineRevision(0),
            operation: marker(9080),
            token: None,
        },
        Command::DoBatchIfRevision {
            expected: kinewright_core::TimelineRevision(0),
            operations: vec![marker(9081)],
        },
        Command::DoBatchCoalesced {
            operations: vec![marker(9000)],
            coalesce_key: "drag-a".into(),
        },
        Command::DoBatchCoalesced {
            operations: vec![marker(9001)],
            coalesce_key: "drag-a".into(),
        },
        Command::DoBatchCoalesced {
            operations: vec![marker(9002)],
            coalesce_key: "drag-b".into(),
        },
        Command::Undo,
        Command::Redo,
        Command::Undo,
        Command::DoBatchCoalesced {
            operations: vec![marker(9003)],
            coalesce_key: "drag-a".into(),
        },
    ]
}

pub(super) fn plain_command(command: &Command) -> Command {
    match command {
        Command::DoIfRevision { operation, .. } => Command::Do(operation.clone()),
        Command::DoBatchIfRevision { operations, .. } => Command::DoBatch(operations.clone()),
        _ => command.clone(),
    }
}

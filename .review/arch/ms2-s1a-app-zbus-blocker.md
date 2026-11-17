# Slice 1a app dependency conflict

Stopped after helper commit `2d926a59`, while qualifying the supervisor.
The helper patch is committed with rulings 32 and 33. Linux admission is still
in process; slice 1a is incomplete.

Ruling 28 requires zbus to be linked only into the supervisor. The design's
dependency gate at `docs/MS2-MEDIA-SANDBOX.md:762` also requires that neither
the app nor helper links zbus. The committed base already violates the app
part of that gate.

At `9aef3952b76095334e144ebf7ff7513e52d34c3c`, the root manifest selects
`eframe = "=0.35.0"` with default features. Its default `accesskit` feature
enables this Linux dependency path:

```text
kinewright-app
└── eframe 0.35.0 [default → accesskit]
    └── egui-winit 0.35.0 [accesskit]
        └── accesskit_winit 0.32.2 [default → accesskit_unix]
            └── accesskit_unix 0.21.1
                └── zbus 5.13.2
```

The base lockfile, downloaded crate manifests, checksums and feature edges
are recorded in `docs/reports/ms2-s1a-evidence/app-zbus-base.json`.
Defined zbus symbols in the existing debug app corroborate the dependency;
that artifact was not rebuilt or attributed to the qualified base. The Cargo
inverse-tree request was cancelled before Cargo started while waiting for
another worker's heavy lock. No graph-build gate is claimed.

Disabling AccessKit would remove existing accessibility support. I have not
changed app features or interpreted the approved rule as an implicit exemption.
The requested lead ruling is to preserve that existing app dependency and
restrict MS2's lifecycle D-Bus client to the supervisor. The helper must still
have no zbus dependency. The supervisor draft's exact zbus 5.19.0 pin also
updates the app's existing compatible transitive version, so integration will
require app regression gates.

The supervisor work is parked in `ms2-s1a-supervisor-wip.patch`, SHA256
`5fefd815a4cfbe65e68f1d6898472a7be3310e94348e1ee92853d12fe6208b4f`.
`git apply --check .review/arch/ms2-s1a-supervisor-wip.patch` passes at the
committed helper revision. It includes blocking zbus, typed anonymous
socketpair bootstrap, PID/starttime checks, PIDFD-bound scope attachment,
verified controls, bounded scope records and native tests. All-target check
and four test cases passed, including the child fixture. The scenarios cover
missing-manager refusal, limits before release and supervisor separation,
and bounded/self/stale request refusal. Full commit gates remain pending.
The logs' hashes, test results, binary hash and final unit inventory are in
`docs/reports/ms2-s1a-evidence/supervisor-draft.json`.

Every scope created by those tests was stopped. The final user-unit inventory
is empty. No host policy, MS1 files or FFmpeg setup changes were made.

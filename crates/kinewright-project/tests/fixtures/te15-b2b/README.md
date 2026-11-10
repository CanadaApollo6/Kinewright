These are exact JSON-string archives of a project root and its immutable objects,
compressed with gzip (`mtime=0`). No paths, locks, credentials or media payloads
are imported when unpacking. Each object name is checked against its SHA-256.

`good` was captured after a real scripted task edit, its approval and application
publication. The other three archives add one correctly hashed and receipted
semantic cut which respectively clears consent, approvals or applications. They
were produced with the cross-cut successor guard removed, through the normal
host publisher; both mutated source files were then restored byte-for-byte.
The generator's assertion that the bad publication must refuse went red in all
three cases. Ordinary object-integrity, envelope, receipt and cut checks remain
valid, so these fixtures distinguish the cross-cut check from a mere hash error.

The production `LoadedBundle::read` and `bundle_recovery::assess` tests require the
positive graph to open and each negative graph to refuse or quarantine without
rewriting it. These are recovery regression inputs, not measurement fixtures or
consent grants. Generation logs and original/restored hashes are recorded in
`target/te1-5/b2b/cut-generation*` in the implementation evidence.

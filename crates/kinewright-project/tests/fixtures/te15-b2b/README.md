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

C5 regenerates all four archives through that same publisher with the mandatory
retention header, introduced-reference ownership and compact event segments.
The three negative captures still fail their original publication-refusal
assertions while the guards are removed. Both source files are then restored
byte-for-byte. Current generation logs, source hashes and uncompressed archives
are in `target/te1-linear/cut-generation/`; the fresh-open test assertions are
unchanged. There is no old-shape reader or fixture conversion in the product.

C4b regenerates the same four graphs through this publisher route with mandatory
checkpoint coverage and the current typed edit binding/removal targets. Each
negative capture still fails its original publication-refusal assertion; both
mutated source files are restored byte-for-byte before fresh-open verification.
The recovery test's positive-open, specific negative-error and no-rewrite
assertions are unchanged. Evidence is in `target/run-limit-c4b/cut-generation/`.
No compatibility decoder or manual graph/receipt rewrite was introduced.

The history-capacity merge regenerates all four archives through the current
writer with referenced history, compact lineage, C5 retention and C4b coverage.
It adds no old-shape reader or graph conversion. The three negative captures
still fail their original publication-refusal assertions with only the two
successor guards disabled. All three temporary source files are restored
byte-for-byte before normal reader tests. Current archives, expected-red logs
and restored SHA-256 values are under
`target/history-capacity/fixture-generation-merged/`.

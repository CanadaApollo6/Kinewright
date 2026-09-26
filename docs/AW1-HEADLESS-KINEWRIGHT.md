# AW1 — Headless Kinewright: design

Status: **promoted 2026-09-24**, revision 2 (recipe v2; critic verdict **revise**, N2 rulings
applied). Binding parent: `docs/AW0-AGENT-WORKBENCH.md` (AW1 §4, served
surface §7, security §8, determinism §9, sharing §10), N2 in
`orchestrator-notes.md` (binding, incl. Riel: external agents' edits land
LIVE, each commit one attributed undo entry), plus the memory principle:
no bundled runtimes; peak-RSS and startup-time budgets for headless
`kinewright mcp` are gates.

Conventions: prose cites symbols, never `file:line`. Numbers measured on
main unless marked (est). Rules R1–R40 are numbered and testable; gates
G1–G10 are named behaviours that fail today. Production line budgets
count new-or-changed production lines only; tests are counted separately
and every rule owns at least one test.

## 0. Changes in revision 2 (finding → section)

- B1 → §4a (new), §5, §15: no live-core server exists (only branch
  servers via `AgentThread::new`/`replace_branch`) — AW1 adds a
  per-project live-core server, a confirmation pump outside chat
  threads, and attributed undo entries; R17 moves to S5.
- B2 → §2: headless sidecar was undefined — extract a UI-free
  `SidecarSession` as a named logic change with its own tests; move
  list gains `load_document`, `RefuseRename`, `ProjectSaveError`,
  `ProjectSaveReport`, `sidecar_refused_observation`.
- B3 → §5: temp+rename races, pid reuse, tab-close liveness — OS
  advisory lock held for the owner's lifetime, `O_EXCL` create,
  hostname in the lock, backoff takeover.
- B4 → §5: reverse case (GUI opens a headless-held project; second
  `mcp` process) — headless binds its own authenticated endpoint;
  handover via `request_handover` (save, release, re-proxy).
- S5 → §6, §15: Bearer [REDACTED] argv — token via env var or 0600 temp
  config; `drivers.rs` updated in S2.
- S6 → §5, §6: token beside the project — token moves to the user
  runtime dir keyed by canonical-path FNV; lock holds pid/host/endpoint.
- S7 → §5: takeover checks `journal_file_name` for a pending journal →
  `pending_recovery` refusal.
- S8 → §8: elicitation needs rmcp's `elicitation` feature + a `Peer`
  in the consent path; negotiate at `initialize`; `allow_destructive`
  MCPB `user_config`; hosts prompt on `destructiveHint`.
- S9 → §7: `undo`/`revert_to_revision` require `expected_revision`;
  revert gets consent + snapshot; denylist gains `open_project` and
  `save_project`.
- S10 → §7, §9: roots + consent enforced on the proxy path too;
  project-less start requires `--allow-root` or falls back to cwd.
- S11 → §10: `headless(true)` is not lavapipe — default `auto` with
  provenance, proofs via `monitor_proof_for_document`, lavapipe lane
  for gates only; G4 and checklist 6 fixed.
- S12 → §11: sample `VmHWM` at points; proxy-mode budget B5 (no
  GPU/FFmpeg init); pins are measured + margin; macOS row.
- S13 → §15: cut over in S1 (no drift); registry commits land after
  MO1 merges (Part C in flight).
- Nits → §3 (exit-2 stdout empty; one-shot `save` redefined),
  §4 (`getrandom` =0.3.4 already in lock; dup fd 1, fd 1 → stderr),
  §7 (`open_project` releases the old lock), §12 (absolute path,
  `claude mcp add`, MCPB `lib/` + `$ORIGIN`, GPL notice,
  `compatibility.platforms`), §4 (`kinewright-eval` uses test-gated
  `test_engine` — production uses `FfmpegMediaEngine`).

## 0.1 Implementation errata (S1 fix round)

Review-1 (B1–B7, S1–S2, N1) and review-2 (L1–L4) findings against S1,
ruled by the lead (F1–F8) and implemented on `aw1/impl`. Items AF1–AF8
amend the sections cited; S2-D1 and S5-OBL are named deferrals, not
changes.

- AF1 → §5: the single lockfile is split. `<project>.lock` is a lock
  OBJECT (created if absent, never unlinked, contents unused); liveness
  is the held `try_lock_exclusive` alone. The claim (pid, hostname,
  endpoint, token_ref, mode, version, started_at, reclaimed_from) lives
  in `<project>.lock.json`, published atomically by the owner only while
  holding the lock and readable at any time — the old read-through-the-
  locked-file fails on Windows (`LockFileEx` denies second-handle reads,
  OS error 33). Release removes the discovery while holding the lock,
  then unlocks explicitly — never a last-close race against a forked
  duplicate. Stale discovery with a free lock reclaims with a warning.
  Backoff kept. (Fix round 2, G1: the publish uses a dedicated strict
  writer — temp beside the unresolved path with `create_new`,
  `write_all`+`sync_all` through one handle, `rename` over the discovery —
  so a planted link is replaced, never followed; no fallback, failures
  remove the temp and report `Io`. The holder sweeps its own stale publish
  temps after the flock. A lock object that is a symlink refuses with typed
  `Io` (Unix also re-checks the fd against the path after open), and
  discovery reads only regular files, bounded at 64 KiB. G2: every
  post-flock exit — all refusals and the success hand-off — goes through an
  acquired-flock RAII guard whose drop unlocks explicitly before closing, so
  the ForeignHost arm's former bare close is covered too. G7: "absent"
  splits from "present but unreadable" — an unreadable stale discovery with
  a free lock reclaims with a typed `lock_reclaimed` warning carrying
  `previous_unreadable: true` (and a pid-0/`unknown` sentinel triple), while
  absence reclaims silently. G9: `release` removes the discovery only if it
  still names the handle (pid, claim second, endpoint), otherwise leaving
  it and logging; `LockfileHandle::verify` detects an object deleted under
  a live owner (Unix fd-vs-path, typed `LockLost`, no write — checked
  before every headless save and every discovery re-publish) and is
  trivially true on Windows, where the object pins itself (fix round 3,
  H2: it opens with `share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)`, no
  DELETE, so no process can rename or delete it while any handle lives;
  contenders stay share-compatible and contend); only the `.lock.json`
  discovery may be hand-deleted, never the `.lock` object — if the object
  is hand-deleted anyway (Unix), the next claimant B owns a new object and
  its `lock_reclaimed` warning names the still-LIVE owner A; both hold a
  flock until A's next save or re-publish, whose `verify` refuses
  `LockLost` (race N-l). Fix round 3, H7: the Unix lock-object open adds
  `O_NOFOLLOW`, so a link planted after the symlink check is never
  followed (N-b); discovery and journal reads open `O_NONBLOCK` on Unix
  and read only an fd that `fstat`s as a regular file, so a FIFO swapped
  in after the pre-check never blocks a claimant holding the flock (N-c);
  the claim carries a random `claim_id` (serde default for older claims)
  and `release` compares it alone — G9's pid/second/endpoint triple is
  not unique within one process (N-d); the stale-temp sweep matches
  exactly `.<name>.<digits>.<digits>.tmp` as `OsStr` bytes, so a sibling
  project whose discovery name extends this one's is never touched and
  non-UTF-8 names sweep (N-a); a `PermissionDenied` publish rename
  (Windows AV/indexer) retries within the §5 3 × 250 ms budget, the same
  atomic rename each time, never a fallback (N-h). Recorded limit (N-k):
  a local writer that plants directories at the next 100 predicted temp
  names denies the publish with a typed `Io` — local-writer only, no
  hang.)
- AF2 → §5, §6: one canonical project identity — full canonical path
  when the target exists, else canonical parent dir plus file name
  (relative resolves at the cwd; raw path when nothing resolves). Lock,
  discovery, claim, token_ref, and journal naming all derive from it, so
  aliases share one lock and one journal name. (Fix round 2, G8: a
  dangling symlink leaf resolves through `read_link` — relative to the
  link's parent, depth-bounded at 40 — so dangling aliases share their
  target's identity. Fix round 3, H5: the bound is Linux `MAXSYMLINKS`
  (40 hops, what the kernel follows); after the 40th hop the reached path
  is checked — a non-link terminal resolves normally, a link still there
  (a longer chain or a cycle) or an unreadable link is a typed
  `ProjectIdentityError`, never a fallback: the lock refuses with
  `LockfileError::Identity`, the journal scan fails closed, and a journal
  header naming such a path never claims. Notes:
  `write_file_atomic` replaces a dangling PROJECT symlink with a regular
  file (pre-existing, unchanged); hard links are a known limitation —
  two hard links to one file hold distinct identities and can
  double-own (race N5); the AF5 network-FS limit still applies.)
- AF3 → §5/S7: the takeover check scans the recovery dir once — base
  journal, every allocator `-N` suffix, and alias-named journals via the
  header's `project_path` — after obtaining the lock, on every ownership
  path. Lookup IO errors fail closed (`RecoveryLookup`). §2's "journal
  retire" pipeline step is a no-op for headless: it owns no journals —
  only a session that replayed recovery data may retire it, so an
  unreplayed pending journal always survives a headless save. (Fix round
  2, G5: the scan also recognises legacy raw-path-hash names and their
  `-N` suffixes — including the ordinary spelling behind a Windows
  verbatim identity — and new journal headers persist the absolute
  canonical identity. Exact header rule: a non-name-matched journal is
  claimed by header only when `project_path` is absolute and
  canonical-identical; a relative, missing, or unparseable header never
  claims — ambiguous legacy identity is ignored, never rebound to the
  current cwd, so it cannot block an unrelated project. G6: the scan
  streams — `BufReader`, one magic read, one parsed header value; name-
  matched journals refuse without any header read; non-regular entries
  are skipped. Fix round 3, H3 (supersedes G6's 1 MiB header cut-off,
  which ignored big projects' alias journals — fail-open): the header
  value is parsed in full as a stream — `project_path` captured, every
  other value (the embedded `initial_document`) skipped as `IgnoredAny`,
  never buffered (a 64 MiB document adds < 2 MiB peak RSS) — and it
  claims only when committed: the byte right after the value must be
  `\n` (no newline, EOF, or trailing bytes = uncommitted, no claim).
  Torn or malformed data is merely unmatched; any other IO error (magic
  or header read) fails closed as `RecoveryLookup`. The only size guard
  is a generous 1 GiB ceiling per header: a header reaching it refuses
  typed (`FileTooLarge` → `RecoveryLookup`, fail closed), never ignored.
  Residuals: the parse costs time linear in the header (debug build
  ≈ 2.5 s per 64 MiB), and a hostile deeply nested skipped value costs
  serde_json one byte of scratch per nesting level, bounded by the
  ceiling (local recovery-dir writers only). Fix round 3, H4: the name match is evaluated first — a
  name-matched entry of ANY type (symlink, FIFO, dir) refuses; only
  non-matched non-regular entries are skipped, before any open. H5: a
  project path with no canonical identity names its journal by its raw
  spelling — a writer-side naming choice, not an identity fallback, since
  the lock and the scan both refuse such a path typed. G10, journal-writer rule: a journal for an identity is
  created or renamed only while holding that identity's lock, including
  Save-As and first-save transitions.)
- AF4 → §2: headless save shares the app's H12/J2/J3 transaction
  machinery (`SidecarRollback` in `kinewright-project`): snapshot and
  restore the destination sidecar and both generation baselines on
  project-write failure, including unreadable-sidecar preservation. (Fix
  round 2, G4: establishment membership rolls with the transaction too,
  plus G3's `.bak` move — the `.bak` moves back exactly, else the bytes
  plan runs; a `.bak` whose move-back fails is left beside the restore,
  never deleted.)
- AF5 → §5: claims carry the real OS hostname (new tiny `gethostname`
  dependency — std has none and the crate forbids `unsafe`; already in
  the lockfile). A stale claim from a KNOWN foreign host refuses
  takeover (`ForeignHost` naming the host); `unknown`-host claims
  predate real hostnames and still reclaim. (G7: the foreign check also
  reads leniently — a claim this build cannot parse still refuses when its
  hostname string names a known foreign host. G11: hostnames compare
  case-insensitively after trimming a trailing dot; an FQDN stays
  distinct and refuses, and the refusal names the discovery to delete if
  this machine was renamed. Fix round 3, H7/N-g: an empty hostname is
  `unknown`, so a blank-host claim reclaims instead of refusing with a
  blank name. N-e, recorded limit: the lenient pass reads at most the
  64 KiB discovery bound and serde_json's 128 nesting levels — a known
  foreign claim past either reads as unreadable and reclaims WITH the
  `previous_unreadable` warning, not `ForeignHost`.) Limit: flock
  liveness is host-local, so on local-lock network filesystems a free
  lock proves nothing about a foreign owner — AW1 claims no multi-host
  exclusion.
- AF6 → §2 (S1-delta refinement, GUARD-B): the session tracks an
  established baseline per stem — a load, a successful flush, or
  adopting the saved path establishes that stem. The empty-flush guard
  applies only to stems this session never loaded or wrote, so a changed
  project save to an established stem always pairs its sidecar. (Fix
  round 2, G3: an empty flush against an occupied unestablished stem
  reports `Occupied` instead of the benign `Skipped` — both save paths
  then preserve-and-replace: the foreign sidecar moves aside with the
  reopen path's `.bak` naming, a paired sidecar is written for the new
  document, and the stem is established. Benign skips — no path,
  suspended, unchanged generation — still report `Skipped`.)
- AF7 → §15 (fix round 2, G13: re-measured, ceiling raised): the
  production ceiling is now 1,500 lines (+300 for this round). The true
  figure from 6c2bdec to HEAD is 1,708 (rs 1,672 + manifests 36;
  method: `scripts/aw1-line-ledger.sh` — `git diff --no-renames`,
  tests uncounted, visibility/indent normalisation, multiset
  moved-span treatment; S1's 771 is unsupported and not used). The
  honest figure exceeds the ceiling — STOP reported, nothing trimmed
  to fit. Lead ruling (2026-09-25): 1,708 accepted for S0+S1 and the
  S0+S1 ceiling set to 1,750. The overrun is the review-driven lock
  and persistence hardening (AF1–AF6: object/discovery split, symlink-
  safe publisher, legacy journals, rollback of establishment), not
  scope growth. S2–S6 ceilings are unchanged; the ledger script is the
  method for every later stage. Fix round 3 (review-driven hardening:
  typed identity refusal, Windows share mode, streaming header parse,
  race nits) — ceiling 1,900; the growth is error handling and docs, not
  scope.
- S2-D1 (deferred, not fixed): an unloaded session's NON-EMPTY flush
  still replaces an occupied stem — pre-existing IN2B §2 rule-7
  behaviour, kept deliberately. A changed project save pairs (AF6); a
  first touch of a foreign stem still overwrites it when the log is
  non-empty.
- AF8 → §2 (fix round 2, G12: byte identity): AW1 sidecars are
  byte-identical to main's writer except where a ruled behavior differs
  from what main (or C) wrote. Exact cases from R2's byte table
  (pre-fix2 snapshot): (1) the C/app buggy second save (stale `e063…`
  vs main's paired `cbb2…`) — H matches main, not C (F1 repair); (2) an
  empty first touch of an occupied stem, where pre-G3 H preserved the
  foreign stem (`e063…`) and main overwrote with a pair (`4b75…`) —
  post-G3 the stem pairs as main's does, and the foreign history
  survives in a `.bak` main never wrote; (3) headless re-saving an
  occupied stem differed from app (`e063…` vs `a5dd…`) — R2 B1, now
  fixed (headless pairs like the app). Everything else in the table is
  byte-identical (lengths, R1/M13/digest stability).
- S5-OBL (gate obligation, not fixed): when S5 wires the GUI lock, port
  `defect_journal_appearing_after_the_scan_is_missed` to the GUI journal
  path — a journal appearing after the scan must be impossible once all
  writers hold the lock. The reproducer stays ignored in the tree until
  then.

## 1. Goal and non-goals

A `kinewright` CLI plus a `kinewright mcp` stdio mode serving the same
seven-tool runtime (`COMPACT_TOOL_NAMES`) — proxying to the GUI's
per-project live-core server when the project is open there (§4a),
headless otherwise — so stock Claude Desktop drives a real project.
External agents' edits land LIVE in the open GUI, each commit one
attributed undo entry (Riel). Lifecycle as registry-only capabilities,
destructive consent without a stderr prompt, file roots, skill v1, and
an MCPB package.

Non-goals: code clips (AW2), aggregate diagnostics/diffs/`explain_frame`
(AW3), measurement (AW4), decision log (AW3 — AW1 emits JSON warnings
the log will later consume), multi-machine sharing, OS sandboxing.
Attribution-ledger persistence is deferred to AW3 (in-memory + commit
responses in AW1).

## 2. Shared project-IO crate: `kinewright-project` (new)

`LutStore` is already media-owned and `PROJECT_FORMAT_VERSION` already
core-owned; everything else the save path needs is `pub(crate)` in the
app today. It moves — lifted `pub`, zero logic change except the named
`SidecarSession` extraction below:

- Envelope + gates: `ProjectFile`, `serialize_project_document`,
  `write_project_document`, `write_project_bytes`, `write_file_atomic`,
  `RefuseRename`, `ProjectSaveError`, `ProjectSaveReport`,
  `can_overwrite_save`, `canonical_session_key`, `load_document`,
  `derive_lut_store`, `project_newer_format_observation`.
- Sidecar writer (whole module): `SidecarWriter`,
  `sidecar_path_for_project`, `SidecarMode`, `LoadedSidecar`,
  `SidecarLoad`, `load_sidecar`, `sidecar_matches_project`,
  `refuse_sidecar`, `sidecar_refused_observation`,
  `build_sidecar_bytes`, `FlushOutcome`, `digest_bytes`, `write_synced`,
  `SIDECAR_SUFFIX`, `SIDECAR_FORMAT_VERSION`, both sidecar
  observations.
- `SidecarSession` (NAMED LOGIC CHANGE, B2): the session half of
  `sidecar_bytes_for_save` / `flush_incidents` /
  `load_session_sidecar` — carried records, `refused_by_id`,
  investigator stash, `sidecar_suspended`, confirmed/last-written
  generations, digest baselines — extracted UI-free with its own
  tests. The app keeps a thin wrapper for panel notes. An empty flush
  never wipes incidents (generation guard); a skipped flush never
  produces a digest mismatch (headless flushes on every save).
- Recovery-journal interplay (pure half only): `fnv1a_64`,
  `journal_file_name`, journal path allocation, retire-on-checkpoint,
  `restore_status`. The egui dialog, `Recovery::show_dialog`, and
  session suspend flags stay in the app. Headless never replays
  journals (S7).
- Lockfile (§5): path derivation, `O_EXCL` create, advisory-lock
  hold, read/reclaim — project-dir IO beside the sidecar.

Cutover happens in S1 (§15): the app deletes its copies and calls
through in the same stage — no drift window. No behaviour change is
proven by G6 (IN2B gates green + save byte-identity over a fixture
corpus), not by inspection. The app-only save prologue (Save-As
routing, newer-format card, mute injection, `adopt_saved_path`) stays
in the app; headless save is `serialize_project_document` →
`SidecarSession` flush → `write_project_bytes` → journal retire, the
same order, minus UI.

## 3. The `kinewright` binary (new crate `kinewright-cli`)

One crate, not a bin in `kinewright-agent`: the agent crate's default
feature is `eval-harness` (fixture data), and the shipped CLI must link
the agent library with `default-features = false`, exactly as the app
does. (Nit: `kinewright-eval.rs` drives `test_engine`, which is
test-gated — the production precedent is the shape, not the engine;
AW1 wires `FfmpegMediaEngine`.)

Verbs (twelve, per AW0 §4): `new`, `open`, `import`, `save`, `inspect`,
`schema`, `apply`, `proof`, `strip`, `check`, `export`, `branch`, plus
the `mcp` stdio mode:

| verb | reads project | mutates | lock behaviour |
| --- | :-: | :-: | --- |
| `new <path>` | no | creates file | refuses if locked; handoff §5 |
| `open <path>` | validates + prints summary JSON | no | read-only; warns if locked |
| `import <proj> <media>` | yes | adds asset, saves | refuses if locked; handoff §5 |
| `save <proj> [--as <path>]` | yes | load + rewrite / Save As | refuses if locked; handoff §5 |
| `inspect <proj>` | timeline state JSON | no | read-only |
| `schema [name]` | capability schemas | no | none (no project needed) |
| `apply <proj> <plan.json>` | yes | commits plan, saves | refuses if locked; handoff §5 |
| `proof <proj> --frame N` | renders one frame | no | read-only |
| `strip <proj> --frames a,b,c` | renders strip | no | read-only |
| `check <proj>` | QA report JSON | no | read-only |
| `export <proj> --preset P -o out` | renders via `Export` | writes output only | read-only on project |
| `branch <proj> list\|create\|merge\|cherry-pick\|discard` | yes | file-local branches | refuses if locked; handoff §5 |
| `mcp [--project P] [--allow-root R]* [--allow-destructive] [--gpu G]` | — | — | proxy-or-own §5 |

One-shot `save` always rewrites and reports `{bytes, digest}` — a
silent no-op is impossible by construction; newer-format overwrite
refuses per `can_overwrite_save`. One-shot mutating verbs never proxy:
a locked project → typed refusal + validated plan to `<stem>.plan.json`
for GUI branch import — never a surprise live mutation.

Stdout discipline: verb stdout is exactly one JSON document —
`{"ok": …}` or `{"error": {"code": …, "message": …}}`; exit 2 leaves
stdout EMPTY (no partial JSON). Diagnostics go to stderr. `mcp` stdout
carries MCP frames only: the CLI dups fd 1 for the rmcp transport, then
redirects fd 1 → stderr, so a stray `println!` cannot corrupt the
protocol. Proofs and strips return file references (paths), never
inline bytes.

Exit codes (the `kinewright-eval.rs` precedent: false→1, error→2):
0 success; 1 typed refusal (revision conflict, consent, contention,
newer-format, roots, `pending_recovery`) with the JSON error on stdout;
2 usage/IO/crash with stderr text and empty stdout. `mcp` exits 0 on
clean EOF, 2 on transport failure.

Branches: on the proxy path they are the GUI's own live `TimelineBranch`
set; headless mints file-local branches as
`<stem>.kinewright-branch-<name>.json` (envelope + base revision +
document), merged through the existing `merge_into` / `cherry_pick_into`
compare-then-apply shape. No new branch semantics.

## 4. Transport/handler split + stdio mode

Today the bind lives inside `start_configured_for_investigator` and
`run_server` only speaks streamable HTTP. AW1 splits it:

- `build_handler(…)` (same ten args plus investigator context) returns
  the configured `KinewrightMcp` with no socket, no thread.
- `serve_streamable_http(handler, …)` keeps today's listener + `axum`
  + `GRACEFUL_SHUTDOWN_TIMEOUT` behaviour byte for byte.
- `serve_stdio(handler)` serves the identical handler over the
  `transport-io` stdio pair (`rmcp::transport::io::stdio`, present in
  pinned `rmcp` 3.1.2 — AW1 adds the `transport-io` AND `elicitation`
  features to the workspace `rmcp` pin, no version change).

Transport is not surface: stdio serves the identical seven tools through
`call_exposed_blocking`, and the served quad (§7) is asserted on both
transports. The investigator constructor keeps its denylist (extended,
§7), broker timeout, and worker count; headless gets its own
constructor rather than stretching the eighth.

Headless engine wiring (copies the `AgentThread::new` shape without
`eframe`): one `FfmpegMediaEngine` built with
`new_with_gpu_and_data_dir(pinned_gpu, data_dir)` serves as the
`Arc<dyn Playback>` + `Arc<dyn Analysis>` + `Arc<dyn Export>` triple —
the engine implements `Playback` itself, so no new playback type.
Proofs render via `Analysis::monitor_proof_for_document` (full-res +
adapter metadata). Construction is LAZY: a proxying `mcp` process never
inits GPU/FFmpeg (B5). Whisper models stay lazy. FFmpeg is the pinned
FFmpeg 8 `third_party` build per `docs/BUILDING.md`, or fail closed.

## 4a. Per-project live-core server (B1; Riel: live, one undo each)

No live server exists: every app `McpServer` runs on a branch core.
AW1 adds one live-core server per open project (multi-project GUI holds
N lockfiles + N servers), started at project open:

- Built on the live `Core` with `publish_to_playback: true`, the full
  seven tools, `ConsentPolicy::GuiBroker`, and the roots guard (§9)
  defaulting to the project dir. Proxied commits go through
  `DoBatchIfRevision` on the live core: each commit is one history
  entry, undoable/redoable in the GUI, revision-continuous.
- Attribution: the live server keeps an attributed-commit ledger
  (revision → agent label + plan summary + timestamp), returned in
  every commit response. Ledger persistence is deferred to AW3.
- Confirmation pump outside chat threads: a project-level pump drains
  the live broker into a pending-confirmations panel — proxied
  destructive commits raise there, not in any thread.

## 5. GUI-proxy protocol: lockfile as discovery, flock as liveness

`<stem>.kinewright.lock` beside the project — pid/host/endpoint ONLY,
no secret (S6). Created `O_EXCL`; the owner holds an OS advisory lock
(`flock`/`LockFileEx`, new pinned `fs2` workspace dep) on the lockfile
fd for its lifetime. Liveness = holding the flock: pid reuse and
tab-close-keeping-pid-alive are dead as failure modes.

```json
{
  "format_version": 1,
  "mode": "gui",
  "project": "/canonical/path/proj.kinewright",
  "pid": 12345,
  "hostname": "riel-laptop",
  "endpoint": "http://127.0.0.1:PORT/mcp",
  "token_ref": "kinewright/a1b2c3….token",
  "kinewright_version": "0.1.0",
  "started_at_unix": 1750000000,
  "reclaimed_from": null
}
```

The lock covers project + sidecar + journal. `token_ref` names the
0600 token file in the user runtime dir
(`$XDG_RUNTIME_DIR/kinewright/<fnv16(canonical path)>.token`,
`%LOCALAPPDATA%` on Windows) — synced folders and Windows ACLs never
see the secret.

`kinewright mcp` startup: read lockfile → flock held + endpoint live
(TCP connect + authed `initialize`) → proxy every MCP call over
loopback with the bearer token (the already-enabled
`transport-streamable-http-client-reqwest` feature), forwarding the
session root set per request (§9). A proxy candidate with an absent
`endpoint` is a lock-check attempt target, not a proxy target: on
`endpoint: None` the startup sequence attempts the lock itself rather
than exiting 4. No live owner → own the lock
(`mode: "headless"`, fresh token), BIND its own authenticated endpoint
(B4 — Desktop + Code proxy to the headless owner instead of double
writing), and serve in-process against a local `Core`. The lock-check
answers exactly one question — "is this identity live?" — never a
four-valued shape; a `Proxy` outcome is not a degraded approval.

Reclaim/takeover: only when the lockfile is gone or re-owned
(different owner identity), with backoff (3 attempts, 250 ms apart);
a held flock is never stolen. A flock-free lockfile means the owner
died without cleanup → reclaim with a typed JSON warning on stderr
(the AW3 decision log consumes this shape) and `reclaimed_from` set.
(G11, hostname rule: two spellings name the same machine iff they agree
case-insensitively after ignoring one trailing dot, so `HOST` and
`host.` match; an unknown or unreadable local hostname is never a
live-match.)
Takeover first checks `journal_file_name` for a pending journal →
refuse `pending_recovery` naming GUI restore (S7); else reload
last-saved bytes, announce `base_revision_reset`, invalidate prepared
plans. Never a torn write.

Handover (B4): the GUI opening a headless-held project sends authed
`request_handover` (registry capability, §7) → headless saves,
releases lock + flock, then re-proxies to the GUI's live server (its
stdio clients unaffected); the GUI takes the lock and starts its live
server. `open_project` flushes the sidecar and releases the old lock
before acquiring the new one.

## 6. Auth on the loopback server

Every `McpServer` mints a 256-bit CSRNG token at start (`getrandom`
=0.3.4, already in `Cargo.lock` — no new version) and enforces
`Authorization: Bearer` via `axum` middleware on the `/mcp` route. No
unauthenticated local port, including tests. In-process clients
(`ScriptedSession`, harness sessions, eval paths) attach the header via
a new `McpServer::auth_token` accessor. External harness drivers
(`drivers.rs` `--mcp-config`, Codex `-c …url`, `acp_session`) receive
the token via the `KINEWRIGHT_MCP_TOKEN` env var or a 0600 temp config
— never argv (`/proc` leak). Missing/wrong token → `401` with no
project bytes. The token is never logged; the runtime-dir file is its
only resting place.

## 7. Lifecycle capabilities and served-surface cost

Five registry-only capabilities, reached only through
`invoke_capability`, added to `INSPECTOR_TOOL_NAMES` (the invocable
list) with `ToolAnnotations` (`destructiveHint` true on `undo`,
`revert_to_revision`, and the destructive half of `request_handover`'s
save path is false — handover only saves):

- `open_project { path }`: headless — validate + `load_document` via
  the shared crate, respawn the branch `Core` under a handler `RwLock`,
  reset the prepared-plan store, publish the project path, flush +
  release the old lock first; proxy — no-op success when `path` is the
  open project, typed refusal naming the GUI action otherwise.
- `save_project { path? }`: headless — shared-crate atomic save
  (sidecar-before-project, journal retire); proxy — forwarded, GUI saves
  through its own path (never a dialog; `path` required when unsaved).
  Headless also saves atomically after every commit; proxy never
  auto-saves (the person owns the GUI file).
- `undo { expected_revision }`: one `Command::Undo` on the live/headless
  core iff the revision matches — a proxied `undo` can never silently
  eat the person's edit. Returns the new revision + undone summary.
- `revert_to_revision { revision, expected_revision }`: bounded
  undo-loop (cap 512) down to the target; refuses above-current and
  below-history-floor with the nearest reachable revision named. Now
  destructive-gated: consent (§8) + snapshot like any destructive
  commit. No `Core` change: snapshots stay human-recovery files.
- `request_handover {}`: GUI → headless owner (§5). Refused on any
  other path with a typed error.

Roots + consent are enforced on the proxy path too (S10): the guards
live in the handler; the live server enforces the forwarded session
roots when present, else its project-dir default; consent raises in
the GUI broker. `INVESTIGATOR_CAPABILITY_DENYLIST` gains
`open_project` and `save_project` (respawning the core / writing the
person's file lands outside the branch document).

Served-surface cost: zero. The served quad stays frozen at
7 / 5,660 B / 3,510 B / 998 B (the M36 pin sites assert byte-identity on
both transports); the five capabilities add M36 ledger rows exactly as
AU4/AU5 additions did, and `prepare_edit_plan`'s input schema is
untouched. Registry pin-site count bumps by five — the only pin edit,
and it lands AFTER MO1 merges (§15).

## 8. Destructive consent (no stderr prompt)

The handler gains a `ConsentPolicy`: `GuiBroker` (today's
`ConfirmationBroker`, person clicks in the thread or the §4a panel) vs
`Headless { allow_destructive, elicitor }`. Non-destructive commits
never raise — unchanged. Host capabilities (elicitation support) are
negotiated at `initialize` and recorded on the session. A destructive
commit (the `is_destructive_operation` seven, described by
`plan_confirmation_description`) or `revert_to_revision` in headless
mode proceeds iff, in order: `--allow-destructive` was passed (or
`allow_destructive` in MCPB `user_config`); else elicitation was
negotiated and the host grants this call (one-shot, per call, naming
what is removed — a `Peer` is threaded into the consent path and
`call_exposed_blocking` gains a bounded sync elicitation bridge);
else refused with a typed error naming `--allow-destructive` and
quoting the removal description. `destructiveHint` annotations stay
accurate so hosts that prompt natively do. Proxy path: forwarded, GUI
broker raises; elicitation never fires there.

A snapshot lands before every destructive commit that proceeds:
`<stem>.kinewright-snapshots/<revision>-<unix>.kinewright`, shared-crate
atomic write, last 8 kept, pruning best-effort and never on the commit
critical path. Snapshot failure fails the commit closed.

## 9. File roots

`--allow-root <dir>` (repeatable) plus MCP client `roots`; a
project-less start requires `--allow-root` or falls back to cwd with a
stated warning. Canonicalised once at session start. Every filesystem
path entering through a plan, `import_media`, `relink_media`,
`import_lut_asset`, `open_project`, `save_project`, or export output
must canonicalise inside a root — absolute paths inside roots are fine,
so media import works; anything outside fails closed with a typed
`outside_roots` refusal naming the root set. CLI one-shot verbs resolve
relative paths against cwd, then gate. The proxy forwards its session
roots per request; the live server enforces the forwarded set when
present, else its default. GUI-project LUT stores derive inside the
project dir, so they pass by construction.

## 10. GPU selection and proof determinism

`GpuContext::headless(true)` is NOT lavapipe (WARP on Windows, fails on
Metal) — so `--gpu lavapipe|auto|adapter=<substr>` defaults to `auto`
(current LowPower-then-fallback chain), and every proof carries adapter
provenance (`backend`, `adapter`, `software_fallback`) in its JSON
sidecar. Proofs render via `monitor_proof_for_document`. The
lavapipe/software-fallback lane exists for gates only (G4): both sides
render the compared frame through a forced-fallback context +
`FrameRenderer::render` at the same revision — byte for byte or the
gate fails. Plan determinism rides the existing revision gate:
replaying a recorded plan against the same base revision yields the
same document, including CLI-applied plans. No wall clock, no RNG, no
system-font fallback in any AW1 path.

## 11. Budgets (release; pins are measured + 25% margin)

Memory-principle gates, all (est) — AW1 measures and pins. RSS is
`VmHWM` sampled at defined points (post-open idle, post-proof) in one
run, plus separate-run peaks; the pin records machine class:

- B1 cold start, spawn → `initialize` response: ≤ 3 s (est).
- B2 peak RSS idle, project open, server serving: ≤ 400 MB (est).
- B3 peak RSS during one 1080p proof render: ≤ 1.0 GB (est).
- B4 no Whisper model bytes mapped before first transcription request.
- B5 proxy mode: ≤ 120 MB RSS (est), ≤ 1 s cold start (est), and NO
  GPU/FFmpeg init (asserted — engine construction stays lazy).

Linux x86_64 and Windows x64 get separate pinned rows, never a shared
number. macOS is not a target platform (`docs/BUILDING.md`) — no pin;
revisit if ported.

## 12. Desktop config + MCPB

Claude Desktop config (absolute path, stock, no dev flags):

```json
{
  "mcpServers": {
    "kinewright": {
      "command": "/home/riel/.local/bin/kinewright",
      "args": ["mcp", "--allow-root", "/home/riel/Video"],
      "env": { "KINEWRIGHT_DATA_DIR": "/home/riel/.kinewright" }
    }
  }
}
```

or `claude mcp add kinewright -- /home/riel/.local/bin/kinewright mcp
--allow-root /home/riel/Video`. `--project` may pre-open one file;
otherwise the session starts project-less and `open_project` selects.
MCPB shape: `manifest.json` (`mcp_config`, `user_config` offering
`allow_root` + `project` + `allow_destructive`,
`compatibility.platforms`), `bin/` per-OS CLI + `lib/` (libav* with
`$ORIGIN` rpath, per the Linux bundle precedent), `LICENSES/` (GPL
notice — the pinned FFmpeg is GPL), `skills/kinewright/SKILL.md`
(skill v1: silence-cut, captions, lower-third, proof-before-export
craft defaults). The CLI ships inside the desktop bundle per the Riel
ruling — the MCPB wraps, not duplicates. Content grows in AW2/AW3;
the package shape is frozen here.

## 13. Rules (testable)

Shared crate: R1 save bytes are byte-identical pre/post cutover over
the corpus. R2 sidecar lands before project bytes with the new+previous
digest pair. R3 `can_overwrite_save` refuses newer-format overwrite and
Save-As stays enabled. R4 temp+rename atomicity; failed rename removes
its temp. R5 `load_document` is one read, envelope default v1, digest
reused by the sidecar gate. R6 `SidecarSession` keeps sequence/
drop-stale/generation semantics for both callers; empty flush never
wipes, skipped flush never mismatches.

Binary: R7 verb stdout is exactly one JSON document; exit 2 leaves
stdout empty. R8 `mcp` stdout is MCP frames only (byte-scan: no log
line; fd-1 discipline). R9 exit codes 0/1/2 per §3. R10 proofs/strips
return file references; no inline base64. R11 mutating verbs save
atomically after each commit. R12 `--help` exits 0 and lists all verbs.

Transport/auth: R13 stdio serves the identical seven tools (same bytes
as HTTP `list_tools`). R14 missing/wrong Bearer → 401, zero project
bytes. R15 token is 256-bit CSRNG, never logged, never on argv, 0600
runtime-dir file (unix). R16 in-process + driver clients attach the
header (existing harness suites pass unaltered in behaviour).

Proxy/lockfile/handover: R17 GUI open → proxied edit lands live,
revision-continuous, one attributed undo entry. R18 GUI absent →
headless serves, owns the lock, and binds its endpoint. R19 liveness
is the held flock; reclaim only on gone-or-re-owned with backoff,
typed warning, `reclaimed_from`. R20 GUI-close mid-session takes over
headless with `base_revision_reset` (after the `pending_recovery`
check), or re-proxies, or refuses — never a torn write. R21 two-writer
contention refuses with `.plan.json` handoff (one-shot verbs). R33
handover: GUI open of a headless-held project saves, releases, and
re-proxies with zero lost commits.

Lifecycle/consent/roots: R22 served quad byte-identical (7 / 5,660 /
3,510 / 998) on both transports. R23 five lifecycle capabilities
reachable only via `invoke_capability`. R24 destructive without consent
refuses naming `--allow-destructive`. R25 destructive with consent
snapshots first (file exists before commit returns). R26 proxy-path
destructive raises in the GUI broker/panel. R27 outside-roots path
fails closed naming the root set, proxy path included. R28
`revert_to_revision` refuses above-current and below-floor with the
nearest reachable revision. R34 `undo`/`revert` require matching
`expected_revision`. R35 investigator denylist refuses `open_project`
and `save_project`. R36 elicitation fires only when negotiated at
`initialize`, one-shot per call. R37 `open_project` flushes and
releases the old lock first.

Determinism/budgets: R29 lavapipe-lane CLI proof == GUI proof, byte for
byte, via `monitor_proof_for_document`. R30 plan replay against the
same base revision is document-identical. R31 B1–B5 within budget on
the pinned machine class. R32 no bundled runtime: the CLI spawns no
Node/Python/Chromium (process-tree scan during G1–G4). R38 proxy mode
performs no GPU/FFmpeg init. R39 pins equal measured + 25% margin with
machine class recorded. R40 hostname + pid in the lock; lockfile
carries no secret.

## 14. Exit gates (all fail today — no binary, no stdio, no lockfile)

- G1 headless round-trip: no GUI; stdio client opens a project, splits
  a clip, commits, pulls a proof; the GUI then opens the same file with
  the change present and provenance showing one CLI commit. (No
  cross-process undo claim — AW0 §12 B1 restatement.)
- G2 proxy liveness + attribution: GUI open; the session proxies; each
  commit lands live as ONE attributed undo entry; one GUI undo removes
  exactly the last commit; redo restores it. (Riel.)
- G3 consent: destructive without consent refuses naming the flag; a
  `RippleDeleteClip` with consent commits and its snapshot file exists;
  `revert_to_revision` is consent+gated the same way.
- G4 proof identity: CLI proof == GUI proof on the lavapipe gate lane
  via `monitor_proof_for_document`, byte for byte at the same revision.
- G5 served quad unchanged (7 / 5,660 B / 3,510 B / 998 B), both
  transports, ledger rows added.
- G6 cutover safety: IN2B gates green; save byte-identity corpus
  passes; journal + sidecar interplay unchanged.
- G7 budgets B1–B5 green on the pinned machine class (measured +
  margin, not estimated, at exit).
- G8 failover: GUI killed mid-session; takeover passes the
  `pending_recovery` check, reloads saved bytes with
  `base_revision_reset`; no torn file, lock re-owned.
- G9 handoff: mutating verb against a locked project refuses with
  `.plan.json`; the GUI imports it as a branch.
- G10 handover: GUI opens a headless-held project → headless saves,
  releases, re-proxies; zero lost commits; second `mcp` process
  proxies to the owner throughout.

## 15. Staging by crate (production budget; tests separate)

Total: ≤ 4,000 new-or-changed production lines. Moved lines count 0.

- S0 workspace: `transport-io` + `elicitation` on the `rmcp` pin,
  pinned `getrandom` =0.3.4 and `fs2`. Budget ≤ 30. Gates: build.
- S1 `kinewright-project` (new) + APP CUTOVER (S13 — no drift): the §2
  moves incl. `SidecarSession` + lockfile module; the app deletes its
  copies and calls through in the same stage. New: lockfile
  create/hold/reclaim, `SidecarSession` extraction delta, headless save
  orchestration. Budget ≤ 800 new. Rules R1–R6, R19, R40. IN2B green.
- S2 `kinewright-agent` transport+auth: `build_handler` split,
  `serve_stdio`, Bearer [REDACTED], `auth_token` plumbing through
  in-process clients, `drivers.rs`/harness token-via-env. Budget ≤ 800.
  Rules R8, R13–R16, R22.
- S3 `kinewright-agent` capabilities: proxy client (roots forwarding),
  five lifecycle ops, `ConsentPolicy` + `Peer` elicitation bridge +
  snapshots, handler roots guard, denylist extension. Budget ≤ 900.
  Rules R18, R20, R23–R28, R34–R37. LANDS AFTER MO1 MERGES (registry
  re-pin serialised; Part C in flight) — count +5 in registry only.
- S4 `kinewright-cli` (new): twelve verbs + `mcp` main + lazy engine +
  `--gpu` + fd-1 discipline + budget harness. Budget ≤ 700. Rules
  R7–R12, R29–R32, R38–R39 halves.
- S5 `kinewright-app` live server (B1, R17 moves here): per-project
  live-core server at project open, confirmation pump + panel,
  attribution ledger, lockfile publish/release, handover receive,
  `.plan.json` branch import. Budget ≤ 700. Rules R17, R21, R26, R33.
- S6 skill v1 + MCPB + docs (`docs/BUILDING.md` CLI section, Desktop
  snippet, `allow_destructive` user_config). Budget ≤ 70. No code
  gates; Riel's checklist §16.

Land order S0→S1→S2→S3→S4→S5→S6 with S3 held for the MO1 merge; S1's
cutover serialises against MO1's app edits (AW0 §12). Each stage ends
green-workspace-gate per commit; two Opus reviews per stage per AW0 §0.

## 16. Hands-on checklist for Riel (ends the slice)

1. Claude Desktop connects via the §12 snippet (no dev flags).
2. Agent edits a project while the GUI shows it open: each commit lands
   live within one turn as one attributed undo entry; revision
   advances; no dialog.
3. One GUI undo removes exactly the last commit; redo restores it.
4. Quit the GUI mid-session; the agent reports `base_revision_reset`
   and continues headless; reopen shows saved bytes only.
5. Destructive without the flag refuses with the flag named; with
   `--allow-destructive`, the snapshot file exists beside the project.
6. `kinewright proof` (lavapipe gate lane) matches the GUI monitor
   proof pixel for pixel; `strip` + `check` return file references
   and JSON; default `auto` carries adapter provenance.
7. GUI opens a headless-held project: save + release + re-proxy, zero
   lost commits; Desktop + Code share one owner.
8. Slice incomplete until findings are discharged or recorded deferrals.

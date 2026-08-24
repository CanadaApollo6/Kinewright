# M41 offline/relink and cache visibility

M41 is the first non-colour capability slice after CC0. It solves one concrete
editor job: reopen a project whose media moved, understand exactly what is online
or unavailable, relink the intended source without damaging the edit, and inspect
or clear Kinewright-owned derived caches without confusing them with source media.

This is a technical workflow. Acceptance is objective; it does not require a taste
review.

## Product contract

### Source identity

Each newly probed `MediaAsset` stores a SHA-256 content identity and byte length.
Legacy projects load with an explicitly unknown identity. The hash is the identity;
byte length is a diagnostic and cheap preflight, never proof by itself.

Availability is runtime state, not project state:

- **Online verified** — the regular file is readable and its observed identity
  matches the project.
- **Online unverified** — a legacy asset is present but the project has no stored
  identity yet.
- **Offline** — the recorded path is missing or is not a regular file.
- **Changed** — a file exists at the path but its identity no longer matches.
- **Unreadable** — the path exists but metadata or bytes cannot be read.

Projects remain valid and saveable while media is offline. Runtime availability,
cache paths, worker state, and byte counts are never serialized into the project or
operation journal.

### Relink

Human and agent workflows both probe and hash a user-selected candidate before
submitting one revision-gated `RelinkAsset` operation. Core application is
filesystem-free and deterministic.

A candidate must match the stored kind, exact frame rate, source-frame duration,
and resolution. When the project already has a source identity, SHA-256 and byte
length must also match. A legacy asset with unknown identity requires an explicit
`allow_unverified_source` decision; acceptance stores the candidate identity so
later checks are verified.

Relink changes only the locator and source identity. It preserves the asset id,
name, colour interpretation or override, clips, bins, string-outs, sync groups,
marks, and every timeline source range. The operation journals, branches, saves,
recovers, undoes, and redoes through the ordinary Core actor.

M41 does not recursively search disks, silently pick a same-named file, canonicalize
project-relative paths, or accept a merely similar replacement.

### Preview and caches

Terminology is deliberately strict:

- **Preview memory** is the existing ephemeral scaled decode path, capped at 1280
  pixels wide. Its decoded RGBA frames are memory-only and can be cleared.
- **Visual assets** are persistent content-addressed thumbnails and waveforms.
- **Derived analysis** is persistent content-addressed silence, scene, and beat
  data.
- **Transcripts** are persistent content-addressed speech analysis.
- **Generated proxy media** is not supported in M41. The UI and agent API report
  that fact explicitly; they never call a thumbnail, analysis file, or in-memory
  downscale a generated proxy.

Inventory reports each owned family, persistence, root where applicable, file
count, and byte count. Scoped clear operations may remove only the named
Kinewright-owned cache root or preview memory. They never remove source media,
projects, LUTs, exports, or downloaded models, and they do not dirty the project.
Running analysis may repopulate a cleared cache; the result states that limitation.

Export, delivery verification, and full-resolution proof continue to require and
decode original media. No offline proxy substitution exists.

## Human workflow

1. Media cards and the source monitor show availability and stored identity.
2. Offline, changed, and unreadable sources use warning treatment and do not imply
   that black/silence is a valid fallback.
3. `Relink…` opens a file picker and performs probe/hash work off the UI thread.
4. A mismatch is rejected with the exact incompatible field. A legacy unverified
   source requires a visible confirmation.
5. Success refreshes playback and analysis. `Ctrl+Z` restores the exact prior
   locator and identity.
6. A cache view explains preview memory, persistent derived families, and the
   unsupported generated-proxy state, with scoped clear controls.

## Agent workflow

- `get_media_status` returns the exact timeline revision plus availability,
  identity, preview mode, cache inventory, generated-proxy support, and analysis
  jobs.
- `relink_media` accepts `expected_revision`, asset id, candidate path, and the
  explicit legacy-unverified flag. It probes/hashes before applying the same Core
  operation used by the app.
- `get_cache_status` is read-only and reports the same family inventory.
- `clear_media_cache` is a scoped media side effect, not a Core edit. Its output
  names the cleared family and warns that active analysis can repopulate it.
- Source/storyboard/proof tools return an explicit offline/changed error rather
  than asking the model to infer failure from a generic decoder message.

## Exit gates

- Legacy JSON migrates with unknown identity; new imports persist a verified
  fingerprint.
- Same-content relink succeeds; hash, kind, rate, duration, and resolution
  mismatches each reject atomically.
- Relink is one revision-gated journal entry and preserves all editorial
  references through save/reopen, undo, redo, and recovery replay while paths are
  absent.
- Moving a generated fixture, reopening offline, relinking, and rendering produces
  the same decoded frame.
- Same-path source replacement invalidates memoized content identity.
- Human and agent surfaces distinguish all availability states and agree on
  identity and cache facts.
- Cache inventory and scoped clear are deterministic, idempotent, and confined to
  owned roots.
- Preview is labelled ephemeral; generated proxy media is explicitly unsupported;
  export still fails without original media.
- Workspace format, check, tests, and strict clippy pass on the Linux development
  host. Windows CI remains required; the next release-affecting smoke pass is on
  Windows and Omarchy, not CachyOS.

## Deferred work

Generated playable proxy creation/use, automatic directory search, project-relative
media roots, managed project-media copying, proxy-only offline editing, relink of
intentional alternate takes, and interchange are separate slices. A future
generated proxy must be a versioned content-addressed media artifact with a manifest,
verified atomic publication, cancellation, and explicit preview-only semantics;
delivery may never silently substitute it for source truth.

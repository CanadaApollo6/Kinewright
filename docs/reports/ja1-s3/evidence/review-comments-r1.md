I'm reporting only: nothing was edited. I read every comment and lint suppression added or changed since `4250d14`, plus the new untracked source files, in `t3-1c0887c5`.

**Delete (25 comment lines across 10 files)**
- `writer_host/s2h_tests.rs:195`
- `bundle_commit.rs:1619`, `:1893`. The doc on line 1893 sits on a `pub(crate)` item, so it is not public API.
- `writer_host/owned_media.rs:114-115`
- `project/owned_media.rs:38`, `:1039`, `:1214`
- `media/owned_io.rs:255`, `:980-981`, `:1107`, `:1169`, `:1234`, `:1345`, `:1347`, `:1485`
- `ja_s3_owned_inputs.rs:350`, `ja_s3_read_only_inputs.rs:60`, `:232-233`
- `ja1-s3-c0-media-mutations.py:151-152`, `:250`
- `kinewright-media/Cargo.toml:47`

**MUST KILL**
- The GUI-origin branch in `writer_host/owned_media.rs` (around line 113). Extract a named one-shot pin helper so the comment's point shows in the code.
- The literal `129` at `owned_media.rs:39`. Make it a named constant.
- The demuxer list behind `owned_io.rs:980`. Name it as the single-object allowlist.
- `deny_open` (`owned_io.rs:1220`). `ja1-s3-c0-media-mutations.py:108` finds its edit site by searching for the text of the comment at `:1234`. Point the script at `deny_open` before deleting `:1234`, or the mutation harness breaks.

**Keep: FFmpeg or Cargo constraints we can't change**
- `owned_io.rs:1098`: FFmpeg calls the callbacks synchronously, which is why the `Send` impl is sound.
- `owned_io.rs:1150`: `ff_copy_whiteblacklists`. Drop the "C0 … guard" label prefix only.
- `owned_io.rs:1157`, `:1170`, `:1210`, `:1273`, `:1484`, `:2027`.
- `kinewright-media/Cargo.toml:48-49`: Cargo can't override an inherited `forbid`. This follows the `kinewright-hash` precedent you mandated.

**Keep: public API doc comments**
- `media/lib.rs:24`
- `owned_inputs.rs:1`, `:157`
- `owned_io.rs:1`, `:77-78`, `:175-176`, `:502`, `:819`
- `project/owned_media.rs:1`, `:373`, `:382`, `:481`
- `read_only_inputs.rs:1`, `:20-21`, `:31`, `:41-42`, `:50-51`, `:80`, `:181`
- `editing_session/owned_media.rs:66`, `:253`, `:447`
- `storage_budget.rs:115`
- the module docs on `c0_fixtures`, `certified_resources` and both test files

**Suppressions: all survive**
- `#[allow(unsafe_code)]` at `owned_io.rs:964` is the local FFmpeg bridge you mandated.
- `unsafe_code = "deny"` at `Cargo.toml:51` is the precedent you mandated.
- The rest are `clippy::too_many_lines`, `too_many_arguments`, `needless_pass_by_value`, `missing_errors_doc` and `missing_panics_doc`. These are pedantic or style rules that don't catch correctness bugs.

**Skipped**
- Evidence logs, the `.md`, `.json` and `.tsv` files, and `__pycache__`.
- Bare `#[cfg]` attributes and comments that were already there before this change.
- Some `unsafe` blocks in `owned_io.rs` have no SAFETY comment (for example `:987`, `:1053`). That's outside this scope; I'm noting it only.

Yes... the meat is marked.

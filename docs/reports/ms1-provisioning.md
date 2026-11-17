# MS1 import-library and source-download fixes

Base `19236a8f`; no pull or reset. Full logs for run `44106990744` were read.
Windows job `134121904534` built the runtime with shared=yes/static=no into
`third_party/ffmpeg`, then the static recorder into `third_party/ffmpeg-recorder`.
The runtime linked `avcodec-62.dll` at 15:11:42Z; its build completed at 15:11:45Z.
The recorder completed at 15:14:39Z; setup failed on `lib/avcodec.lib` at 15:14:40Z.
Windows testgen had not started. No prefix overwrite or profile-order error occurred.

The verified FFmpeg 8.0.3 source explains the location mismatch:

- `configure:6004` sets SHLIBDIR to bin. Line 6012 puts `.lib` import libraries
  in SLIB_INSTALL_EXTRA_SHLIB; line 6013 puts only `.def` files in EXTRA_LIB.
- `ffbuild/library.mak:97` installs EXTRA_SHLIB in SHLIBDIR. The pkg-config
  generator still links through libdir (`ffbuild/pkgconfig_generate.sh:41`).
- The builder now moves the six runtime import libraries from bin to lib before
  publishing its marker. DLLs stay in bin. Recorder/testgen installs are separate.
  No upstream source, configure flags, codec registrations or system packages change.

Linux job `134121904434` failed fetching source with URLError/Errno 110.
Source downloads now use three attempts, a 30-second socket timeout and 2/4-second
backoff. Each attempt uses a new empty directory under `target/ffmpeg-source`;
failed partial responses are removed. SHA-256 must match the pin before atomic
publication to `<sha256>/source.tar.xz`. Restores are rehashed, and each build
copy is rehashed before extraction. Bad digests fail closed before extraction,
execution or replacement of the existing install.

CI restores/saves that single tarball with key `ffmpeg-source-<sha256>`.
Source-cache writes require a canonical-repository producer push and completed
verification. Forks do not restore it. Release jobs may reuse verified source;
the existing mandatory no-binary-cache flags still force compilation.

Evidence under `target/ms1-install`:

- `windows-install-excerpt.log` and `windows-install-rule.txt`: CI prefix/type
  trace and the verified upstream install rules.
- `python-tests.log`: `Ran 18 tests`, `OK`. Includes all six Windows import paths
  after both installs, interrupted-response cleanup, retry exhaustion and fresh/
  restored bad-source refusals that preserve the previous install.
- `mutations.log`: omitting relocation fails with `Required import library missing:
  avcodec.lib`; one download attempt fails on the first timeout; omitting the
  restored-cache hash check fails because the expected SystemExit is not raised.
- Live source was fetched from ffmpeg.org and verified against unchanged SHA-256
  `6136812ea6d4e68bdba27e33c2a94382711cdf4f8602ffef056ff792bd6f9818`.
- Actionlint, composite-action YAML parsing and slow-test manifest lint pass.
  The workflow's source-key generator also runs in a test and emits the exact pin.
- All three Linux profiles rebuilt with fingerprint
  `f46856912eb69e3d8eaa893dcfe8508e50422a20b5ae3fda4ed3420b2adecee3`;
  installed binary digests are logged. Runtime registry/size remains 38 decoders,
  14 demuxers, 16 parsers and 11,634,104 bytes. All six runtime library hashes
  remain identical after recorder/testgen installation. Recorder stays 4,784,504
  bytes and passes its closed registration and software ALSA PCM/WAV audits.
- MS1 media witnesses: `9 passed; 0 failed`; app build passed (same pre-existing
  `namespace/linux.rs:797` dead-code warning as the preceding gates).
- Workspace fast gate: exit 0; 4,531 passed, 0 failed, 144 ignored across 71
  suites. Raw runner summary: `fast tier on linux skipped exactly the 83 manifest
  tests and 61 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing
  else; 34 other-OS manifest tests ran and passed`.

Native makes and Cargo use the shared lock, nice 10 and four build jobs. Rust
tests use a synthetic home and software Vulkan. No Rust source changes; fmt,
clippy and the local slow tier are not rerun in this pass. Windows execution
after this fix awaits the lead's push of the `[slow-tier]` commit.
The actual GitHub source-cache restore/save round-trip likewise awaits CI.
The local decision trail is `target/ms1-install/decisions.tsv`; review remains
with the lead, and no independent trail review was delegated from this worker.
Completed logs are compressed and temporary FFmpeg source/build trees removed;
the verified tarball and installed outputs remain. `target/` is 36,813,856,768
bytes; r3/r4 are absent and r5/r6 remain.

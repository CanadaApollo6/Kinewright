**6.5/10. Request changes.** Three actionable findings. The first is a data-integrity blocker. Findings are source-derived; I did not run Cargo or mutations.

1. **C1 [blocker] [namespace/linux.rs:913](/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/crates/kinewright-project/src/namespace/linux.rs:913). Duplicate imports trust length instead of bytes.**
   When the destination digest filename already exists, `publish_hashed` checks only regular-file type and length, then deletes the newly written, correctly hashed temporary. A previously admitted object corrupted without changing length can therefore replace the correct incoming bytes. Certification checks only chunks fetched by probing, so corruption outside that set can escape admission. A previously populated publication memo can also skip the full-byte verification.

   Verify the existing object's complete digest before discarding the temporary and accepting the duplicate. Add a witness that corrupts an existing object's uncached payload, imports the correct original again, and requires refusal with unchanged document/manifest. This threatens the retained original once the person relies on successful import and removes the external copy.

2. **C2 [should] [owned_io.rs:984](/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/crates/kinewright-media/src/owned_io.rs:984). Default reservations reject ordinary HD media.**
   The padded raster, multiplied by 16 bytes per pixel and 24 codec buffers, requires **577 MiB for 1280×720** and **1,123 MiB for 1920×1080**. These are calculations from the code. `CertifiedInput::open` reserves that amount before probing, while the default metadata budget is only 512 MiB. Consequently, ordinary HD input cannot complete production admission, regardless of available machine memory. The current small fixtures miss this.

   Make the enforced decoder allocation bound compatible with supported production formats and existing limits. Review the simultaneous probe/playback reservations too. Add default-budget 720p and 1080p import, probe, decode, and reopen controls.

3. **C3 [should] [c0-mutations.json:102](/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/crates/kinewright-project/tests/c0-mutations.json:102), [c0_mutations.py:74](/home/riels/.t3/worktrees/Kinewright/t3-1c0887c5/crates/kinewright-project/tests/c0_mutations.py:74). The ordering mutation can earn a false behavioral red.**
   `unsaved-admission-before-install` inserts an early checkpoint install but leaves the original install path intact. The first successful-import setup then encounters the changed history and fails at its `unwrap`, before the capacity-refusal preservation assertions. The driver accepts any single failed test with exit 101, so it can count this setup failure as a protected witness.

   Move the installation operation rather than duplicating it, and require the intended preservation assertion to produce the red. Apply equivalent assertion-location validation to the other project mutation cases.

I checked the tracked diff, every new source/test/script, the required design boundaries, and the named evidence logs. All **43 files matched the frozen source snapshot** at completion. Existing logs report successful build/clippy and 17 media, 8 project-unit, 5 integration, 10 read-only, 1 host, and 2 App tests.

Mutations, native Windows, full suites, and CI remain **NOT RUN** here. No edits, Cargo, or agents were started. Opus retains the trail audit.

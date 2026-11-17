# Native dialogue measurement wiring plan

1. Update Linux/MSVC FFmpeg pins and Rust bindings to the latest stable FFmpeg 9 line; install locally.
2. Keep provisioning and all Cargo work within the laptop CPU, locking, disk, and no-GPU limits.
3. Create TaskDialogueMeasurements in writer_host::start_task_server; construct CachedDialogueMeasurements lazily.
4. Bind SessionMedia to retained owned run inputs; restrict measurements to the task brief's owned-input manifest.
5. Reject an unadmitted recording before stream lookup or cache access; never fall back to asset paths.
6. Reuse the existing media cache and shared resident cache, including speech/noise profile identity.
7. Add tests for measured admitted audio, out-of-admission refusal, and no-admitted-audio typed refusal.
8. Run each regression before its production change and retain the specific red/green outcome.
9. Fetch and merge newest origin/dq1 before manifest edits; build the merged checkout immediately.
10. If M5 is absent, finish provider/admission work and report before touching native manifests.
11. Add dialogue transcript reads, cut_dialogue for editing roles, and check_dialogue_cut to native manifests.
12. Flip M5's absence assertion to role-specific presence; verify the native-provider allowlist too.
13. Run fmt, touched-crate all-target clippy, slow-feature tests, MCP integration, and relevant text pins.
14. Record deliberate model-visible byte changes, final plan deviations, test evidence, and disk usage.
15. Commit locally with [slow-tier] in the final message and the Codex co-author line; do not push.

# S0 round-2 review fixes

Parent: `1b9413b4f6157c6d636951787522940654b54b52`.
One follow-up commit; no push and no new review round.

- **Astra N1:** only process/report exit pairs 0/0 and 101/nonzero are admitted.
  Timeouts, signals, other numeric exits and missing/incomplete common reports
  remain NOT RUN. Witnesses:
  `app_exit_classifier_rejects_abnormal_numeric_status_despite_completed_report`
  (Windows `from_raw(0xC0000005)`, Linux abnormal numeric equivalent), and
  `native_app_bridge_rejects_abnormal_exit_after_completed_report` (actual App
  test executable exits 86 after inner exit 0 and inner exit 1 reports).
  Existing 101/assertion and 101/setup witnesses remain required.
- **Muse new Minor 1:** finish-attempt state is independent of successful
  release. Drop does not repeat a receipted timeout. Witness:
  `timed_out_finish_is_not_retried_by_drop_after_its_receipt`; a deterministic
  blocked-descendant injection expires the actual deadline, then checks that
  the killed leader remains waitable after Drop, matching the failed receipt.
- **Muse residual 3:** graceful Worker finish and its early-error Drop window
  allow media teardown plus worker reap; explicit kills retain the reap-only
  window. Extended witness:
  `cleanup_deadline_outlives_all_fixed_role_teardown_allowances` calls real
  Worker spawn/finish with a fixed EOF worker taking three seconds to retire,
  requires a successful non-killed/reaped event, then checks outer arithmetic.
  The fixed worker is a deterministic teardown stand-in, not GPU timing.
- **Muse residual 2:** Windows abnormal-exit tree containment is deferred, as
  directed. README and JA1.G6's pending ledger identify the PID/taskkill
  limitation and owner: JA-1 guardian/native-parity owner (S4/S6). Windows lanes
  stay NOT RUN; no Windows Job Object or production guardian is added in S0.

All four decisions are accepted; no finding is rejected. The Windows
implementation is deliberately deferred by the lead. No format/production
semantics change, new dependency, private practice content or gate promotion.
All 48 Studio gates and their existing stage owners remain pending; Windows,
provider/practice, physical-display/human, production guardian and CI acceptance
are not supplied by these Linux runs. No release-profile or full-workspace test
run is claimed.

Reproduce the full ordered gate with `scripts/ja_s0_verify.py 5 --output DIR`
after sourcing setup-ffmpeg.sh; use `--tests-only` after the commit for the
final-HEAD harness/native/App/42-pin rerun. The four new targeted mutation cases
are `app-abnormal-classifier`, `app-abnormal-native`, `single-finish` and
`worker-teardown-window` in `scripts/ja_s0_mutations.py`. Each runs only its named
witness, requires a runtime assertion failure, restores exact source bytes,
then requires a passing run. The source/command/binary/log hashes, literal
failure assertions, raw summaries, unchanged-fixture baseline controls/reds
and final-HEAD identities are retained in the separate round-2 archive and
external integrity index, not committed logs.

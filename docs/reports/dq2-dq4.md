# DQ2 measured edges and DQ4 cut score

## Stage A — measured edges

Base: `75b8dfae`; branch `dq2`. This is a pure Core addition: no caller, I/O, Document, rendering or existing transcript/pacing behavior changes. Scarlet `2cbab542` was read only; `LICENSES/scarlet-MIT.txt` is its unmodified MIT notice.

Public surface: `SoundProfile::from_raw/from_hops`, `Hop`, `Word`, `Settings`, `Span`, prepared `Dialogue::{resolve,guard,sound_at,frame_edge}`, `CutRequest`, `ResolvedCut/ResolvedEdge`, `EdgePlacement`, `FramedEdge`, typed input/profile/edge refusals, and `pad_edge`. The prepared value owns one profile and its derived quiet/valley/gap/dip intervals; accessors borrow it. A refusal owns a word name so callers may keep the diagnostic after dropping the measurement.

Media contract: canonical non-overlapping 10,000-us hops on source time zero, explicit recording duration, shorter final hop. For the selected decoded audio stream, peak is the maximum absolute full-band sample over all channels in the hop; RMS is the maximum of per-channel sqrt(mean(sample squared)) in that hop. Never average channels before measuring. Convert amplitude by 20*log10 to signed hundredths of dBFS, round nearest, saturate to [-12000, 0]. Voice is true if any channel's VAD interval overlaps the half-open hop; do not derive it from ASR word times. The builder validates matching lengths, coverage, nonpositive levels and RMS <= peak, and saturates below the floor. The general constructor accepts 1..31000-us hops; the media producer must supply 10000 us. Words have positive, ordered source-microsecond spans within the recording; overlapping words are allowed with ordered ends. Source content hash, audio stream identity, provenance and cache binding belong to the later media layer, not this pure numeric type.

Scarlet differences and reasons:
- One complete finite recording profile replaces scan pages and alternate transcript arrays; missing evidence is a typed refusal, never an executable timing fallback or a planner review state.
- Digital recording endpoints are explicitly `SourceBoundary`, not claims of new measured quiet; a request with no quiet anywhere refuses.
- Integer centidB thresholds conservatively floor Scarlet's amplitude cutoffs. One Settings value owns all acoustic/timing policy, including 150/100-ms air, 120-ms snap, -55-dB dip and 100-ms in-word gap.
- Untranscribed sound and a short valley above the dip cutoff refuse conservatively. A pause that already fits its requested leave time refuses before clamping through intervening sound.
- Fractional frame instants use exact rational arithmetic. Nearest rounding is clamped into measured support and re-guarded without truncating microseconds. Dip support includes adjacent equally deep hops inside the same word search window; the framed report gives the actual returned hop and signed rounding shift.
- No TypeScript timeline/planner staging/review or rendering is ported. Source test names accompany each ported case.

Evidence: focused DQ2 24/0/0 (including four property tests); complete Core 836/0/2. Workspace all-target clippy with `-D warnings`, workspace build, workspace test compilation, fmt check and slow-test lint all pass (`a4-*-result.json`; lint inventories 140 slow tests, 160 ignored entries).. Two requested deliberate breaks: `frame-nearest`: red 0/1 at `DQ2_FRAME_ROUNDS_INSIDE_RUN`, restored green 1/0; `two-dips`: red 0/1 at `DQ2_TWO_DIPS_REFUSED`, restored green 1/0 (`pairs/*-result.json`).. Both restore the mutated Rust file byte-for-byte and restore the entire checkout diff. Logs remain untracked under `target/dq2/`.

An intermediate pair attempt was rejected by its checkout-diff guard because the harness output path was edited while the pair was active. Its Rust-byte restore passed; the rejected log is retained and is not counted. The final frozen-harness rerun is the qualification above.

The next agent-facing tool still needs source/clip/revision identity binding; media decoding, VAD and profile caching; source-to-clip/project mapping; checks against all simultaneously audible material and the final frame grid; and the existing consent/atomic-edit path. Real recording quality, media extraction, provider/tool integration, rendering, Windows execution and listening were NOT RUN.

The only edit after the final pairs was backtick formatting of one rustdoc field name, found by clippy; their executable Rust is unchanged. Target cleanup removed only build output: 30 GiB before, 7.1 GiB immediately afterwards, 23 GiB at Stage A completion; all earlier TE1 evidence remains.

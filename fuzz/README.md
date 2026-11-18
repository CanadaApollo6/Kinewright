# Fuzzing

This crate holds the libFuzzer harness for the MS2 IPC strict framing. It
is outside the workspace members, so `cargo build --workspace` and CI do
not build it.

The `ipc_framing` target exercises the host's real consuming path, in this
order:

1. `ProbeSession::read`: strict framing, rate charge and bounded JSON.
2. `ProbeSession::accept`: the state machine and metadata validation.
3. A round-trip check: every frame the host accepts must re-encode and
   decode as the same message.

Byte 0 of each input chooses the mode:

- **Raw mode** passes the rest of the input unchanged to the frame reader.
- **Structured mode** wraps records in valid headers, so mutations reach
  the JSON parser and the state validators.

`src/framing.rs` describes the input format.

## Build

Run this from the repository root:

```bash
fuzz/build.sh
```

The script builds with `nightly-2026-08-21` and with the flags cargo-fuzz
would set: SanitizerCoverage, AddressSanitizer, debug assertions and
overflow checks. It does not install cargo-fuzz. It builds offline, so
fetch dependencies first with
`cargo +nightly-2026-08-21 fetch --manifest-path fuzz/Cargo.toml`.

## Run

Run the fuzzer bounded in time and memory, with no network:

```bash
R=target/fuzz/x86_64-unknown-linux-gnu/release
$R/ipc_framing_seeds target/fuzz/corpus/ipc_framing
systemd-run --user --scope -p MemoryMax=3G -p MemorySwapMax=0 \
    unshare -rn $R/ipc_framing -dict=fuzz/ipc_framing.dict \
    -max_total_time=1800 -rss_limit_mb=2048 -malloc_limit_mb=1024 \
    -timeout=10 -print_final_stats=1 \
    -artifact_prefix=target/fuzz/artifacts/ target/fuzz/corpus/ipc_framing
```

If the fuzzer finds a crash, fix it at its root. Then add the crash input
as a regression case to `crates/kinewright-media-ipc/src/tests.rs`.

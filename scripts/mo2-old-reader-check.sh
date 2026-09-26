#!/usr/bin/env bash
# MO2 R7 / §13 gate 11: the real old-reader check (review 2 S5, lead ruling
# N10). A manual pre-land check run before A+B land — not a CI job; CI keeps
# the in-reader simulation in `kinewright-project`'s tests.
#
# What it does:
# 1. This checkout's shared writer emits the gate-11 corpus (the ignored
#    `project::tests::mo2_old_reader_corpus`): three v1 controls, the four
#    MO2 features (adjustment, solid, push_left, blend=screen) and
#    `blend-dropped`, the blend document with only its blend removed.
# 2. The frozen base reader (default f241aa5, the last pre-MO2 main) is
#    extracted with `git archive` into a scratch tree and built in its OWN
#    fresh Cargo target. Never point it at the head target: review 2's first
#    attempt shared one and silently reused head libraries (archived sources
#    carry commit-time mtimes), which is invalid evidence.
# 3. The base reader asserts review 2's table stage by stage:
#    adjustment / solid  -> serde parse refuses the unknown content variant
#    push_left           -> parses (v2), then validation refuses the name
#    blend=screen        -> opens advisory as v2, overwrite refused, the old
#                           re-save loses exactly the blend (== blend-dropped)
#                           and emits v1; the source bytes stay untouched
#    v1 controls         -> open as v1, overwrite allowed, byte-identical
#                           re-save of the head-written bytes
#
# Usage: ./scripts/mo2-old-reader-check.sh [BASE_REV]
#   MO2_OLD_READER_SCRATCH  scratch directory (default target/mo2-old-reader)
#   MO2_OLD_READER_KEEP=1   keep the scratch tree and its target afterwards
# Needs >= 6 GiB available RAM: the base build compiles kinewright-media.
set -euo pipefail

BASE_REV="${1:-f241aa5}"
repo="$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
scratch="${MO2_OLD_READER_SCRATCH:-$repo/target/mo2-old-reader}"
scratch="$(mkdir -p "$scratch" && cd "$scratch" && pwd)"
base_tree="$scratch/base"
base_target="$scratch/base-target"
corpus="$scratch/corpus"
head_target="$(cd "$repo" && realpath -m "${CARGO_TARGET_DIR:-target}")"

if [[ "$base_target" == "$head_target" ]]; then
    echo "refusing: the base reader must not share the head Cargo target ($head_target)" >&2
    exit 2
fi
available_gib=$(awk '/MemAvailable/ { print int($2 / 1048576) }' /proc/meminfo)
if (( available_gib < 6 )); then
    echo "refusing: ${available_gib} GiB available, the base build needs >= 6 GiB" >&2
    exit 2
fi

cleanup() {
    if [[ "${MO2_OLD_READER_KEEP:-0}" != 1 ]]; then
        rm -rf "$base_tree" "$base_target"
    fi
}
trap cleanup EXIT

# shellcheck source=/dev/null
source "$repo/scripts/setup-ffmpeg.sh" >/dev/null

rm -rf "$base_tree" "$base_target" "$corpus"
mkdir -p "$base_tree" "$corpus"

echo "== head $(git -C "$repo" rev-parse --short HEAD) writes the corpus"
(
    cd "$repo"
    MO2_OLD_READER_CORPUS="$corpus" cargo test -p kinewright-project --lib -- \
        --ignored --exact project::tests::mo2_old_reader_corpus --nocapture
)

echo "== base $BASE_REV reader, isolated target $base_target"
git -C "$repo" archive "$BASE_REV" | tar -x -C "$base_tree"
# Fresh mtimes as well as a fresh target: nothing may look older than a
# cached artefact.
find "$base_tree" -type f -exec touch {} +
mkdir -p "$base_tree/crates/kinewright-project/tests"
cat > "$base_tree/crates/kinewright-project/tests/mo2_old_reader.rs" <<'RUST'
//! Written by scripts/mo2-old-reader-check.sh; compiled only against the
//! frozen base reader. Asserts review 2's old-reader table stage by stage.
use std::{fs, path::PathBuf};

use kinewright_core::OpError;
use kinewright_project::{
    ProjectFile, can_overwrite_save, load_document, serialize_project_document,
};

#[test]
fn mo2_old_reader_table() {
    let dir = PathBuf::from(std::env::var_os("MO2_OLD_READER_CORPUS").expect("corpus dir"));
    let read = |name: &str| {
        let path = dir.join(format!("{name}.kinewright"));
        let bytes = fs::read(&path).unwrap_or_else(|error| panic!("{name}: {error}"));
        (path, bytes)
    };

    // Unknown content: serde refuses at parse, before any advisory handling.
    for name in ["adjustment", "solid"] {
        let (path, bytes) = read(name);
        let error = serde_json::from_slice::<ProjectFile>(&bytes)
            .expect_err(name)
            .to_string();
        assert!(error.contains(&format!("unknown variant `{name}`")), "{name}: {error}");
        assert_eq!(load_document(&path).expect_err(name), error, "{name}");
        assert_eq!(fs::read(&path).unwrap(), bytes, "{name}: source untouched");
        println!("{name}: PARSE refuses: {error}");
    }

    // Geometric transition: parses as v2, then validation refuses the name.
    let (path, bytes) = read("transition");
    let file: ProjectFile = serde_json::from_slice(&bytes).expect("push_left parses");
    assert_eq!(file.format_version, 2);
    let refusal = OpError::UnknownTransition("push_left".to_owned());
    assert_eq!(file.document.validate(), Err(refusal.clone()));
    assert_eq!(load_document(&path).expect_err("validation"), refusal.to_string());
    println!("transition: parses v2, VALIDATION refuses: {refusal}");

    // Blend only: opens advisory as v2, drops the blend, refuses overwrite.
    let (path, bytes) = read("blend");
    assert!(String::from_utf8_lossy(&bytes).contains("\"blend_mode\": \"screen\""));
    let (document, version, _) = load_document(&path).expect("blend-only opens");
    assert_eq!(version, 2, "the retained version is the file's");
    assert!(!can_overwrite_save(version), "overwrite is refused");
    let resave = serialize_project_document(&document).expect("old serialization");
    assert!(!resave.contains("blend_mode") && !resave.contains("format_version"));
    let (_, dropped) = read("blend-dropped");
    assert_eq!(resave.as_bytes(), dropped, "the old re-save loses exactly the blend");
    assert_eq!(fs::read(&path).unwrap(), bytes, "blend: source untouched");
    println!("blend: OPENS v2 advisory, overwrite=false, old re-save drops blend as v1");

    // v1 controls: open as 1, overwrite allowed, byte-identical re-save.
    for name in ["v1-default", "v1-fixture", "v1-m20"] {
        let (path, bytes) = read(name);
        let (document, version, _) = load_document(&path).expect(name);
        assert_eq!(version, 1, "{name}");
        assert!(can_overwrite_save(version), "{name}");
        let resave = serialize_project_document(&document).expect(name);
        assert_eq!(resave.as_bytes(), bytes, "{name}: byte-identical re-save");
        println!("{name}: OPENS v1, overwrite=true, re-save byte-identical");
    }
}
RUST
(
    cd "$base_tree"
    # Debug info off and bounded jobs: this target is thrown away.
    CARGO_TARGET_DIR="$base_target" MO2_OLD_READER_CORPUS="$corpus" \
        CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
        cargo test --locked -p kinewright-project --test mo2_old_reader -- --nocapture
)
echo "== MO2 old-reader check passed: head $(git -C "$repo" rev-parse --short HEAD) vs base $BASE_REV"

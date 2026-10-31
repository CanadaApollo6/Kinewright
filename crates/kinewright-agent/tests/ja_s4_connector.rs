//! Installed subscription qualification. Normal CI never runs this test.
#![cfg(all(target_os = "linux", feature = "eval-harness"))]

#[test]
#[ignore = "on-demand: installed Codex subscription Director qualification"]
fn s4b_installed_codex_director() {
    kinewright_agent::writer_host::run_installed_codex_director_witness();
}

#[test]
#[ignore = "on-demand: installed Codex unpaid local policy qualification"]
fn s4b_installed_codex_local_policy() {
    kinewright_agent::writer_host::run_installed_codex_local_policy_witness();
}

#[test]
#[ignore = "on-demand: unpaid installed Codex sandbox network qualification"]
fn s4b_installed_codex_network() {
    kinewright_agent::writer_host::run_installed_codex_network_witness();
}

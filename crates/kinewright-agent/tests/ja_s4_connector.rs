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

#[test]
#[ignore = "on-demand: unpaid installed Codex dry launch with provider egress denied"]
fn s4b_installed_codex_dry_launch() {
    kinewright_agent::writer_host::run_installed_codex_dry_launch();
}

#[test]
#[ignore = "on-demand: unpaid policy egress and read-only sign-in witness, probe only"]
fn s4b_c5_fix8_unpaid_policy() {
    kinewright_agent::writer_host::run_unpaid_policy_witness();
}

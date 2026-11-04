//! Installed-provider end-to-end tests. Normal CI runs only the grant guard;
//! every installed-CLI test is on-demand.
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

#[test]
#[ignore = "on-demand: unpaid installed Claude Code dry launch with provider egress denied"]
fn s4b_installed_claude_dry_launch() {
    kinewright_agent::writer_host::run_installed_claude_dry_launch();
}

#[test]
#[ignore = "on-demand: installed Claude Code subscription Director, lead grant required"]
fn s4b_installed_claude_director() {
    kinewright_agent::writer_host::run_installed_claude_director_witness();
}

/// No paid Claude run starts without a lead grant: the witness stops before
/// its dry launch, any provider process or any send.
#[test]
#[should_panic(expected = "C6_NO_LEAD_GRANT")]
fn s4b_claude_director_refuses_without_a_lead_grant() {
    kinewright_agent::writer_host::run_installed_claude_director_witness();
}

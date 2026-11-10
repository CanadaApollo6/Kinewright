//! Explicit paid entry. Only the lead may invoke this with an approved grant.
#![cfg(target_os = "linux")]
#[test]
#[ignore = "on-demand: per-run lead grant and same-head unpaid dry pass required"]
fn te1_one_granted_run() {
    #[cfg(not(feature = "te1-eval"))]
    panic!(
        "the paid entry requires the explicit te1-eval feature; no grant or provider was opened"
    );
    #[cfg(feature = "te1-eval")]
    {
        let result = kinewright_agent::te1_eval::run_one_granted(std::path::Path::new(env!(
            "CARGO_BIN_EXE_kinewright-job-guardian"
        )))
        .expect("granted evaluation refused or interrupted; never retry this grant");
        println!("{}", serde_json::to_string(&result).unwrap());
    }
}

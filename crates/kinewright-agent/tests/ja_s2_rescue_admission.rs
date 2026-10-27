#[path = "../../kinewright-project/tests/support/ja_s2_c6_admission.rs"]
mod admission;
#[test]
fn w14_local_agent_admission_precedes_mutation() {
    admission::matrix(|path| {
        let mut session = kinewright_agent::LocalEditingSession::open(path)?;
        session.save()?;
        drop(session);
        let mut resumed = kinewright_agent::LocalEditingSession::open(path)?;
        resumed.save()?;
        Ok(())
    });
}

fn durable_test_root() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
}

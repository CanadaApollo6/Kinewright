#[test]
fn s4b_c3_fix2_session_commands_refuse_unpaired_task_checkpoint() {
    use kinewright_core::Command;
    for close in [false, true] {
        let f = Fixture::new();
        let start = f.start();
        let run: RunId = start.envelope.identity("run").unwrap();
        let claim = crate::execution_claim::MachineClaim::acquire_fixture(
            &f.temp.path("history-machine"),
            start.envelope.project.as_str(),
            run.0.as_str(),
        )
        .unwrap();
        let (execution, _, _, core) = history_task_endpoint(&f, &run);
        for id in [1, 2] {
            core.request(Command::Do(history_marker(id))).unwrap();
        }
        core.request(Command::Undo).unwrap();
        let expected = core.export_history().unwrap();
        assert!(!expected.cursor.redo.is_empty());
        let producer = f.producer(&run);
        let saved = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .checkpoint_task_history(&producer, &run, &execution.task)
            .unwrap();
        for id in [3, 4] {
            core.request(Command::Do(history_marker(id))).unwrap();
        }
        core.request(Command::Undo).unwrap();
        let unsaved = core.export_history().unwrap();
        let reply = f
            .editor
            .request(if close {
                HostCommand::CloseSession {
                    branch: saved.source.branch.clone(),
                }
            } else {
                HostCommand::SaveSession {
                    branch: saved.source.branch.clone(),
                }
            })
            .unwrap();
        let refused = reply.error.is_some();
        let current = {
            let host = f.host.owner.upgrade().unwrap();
            let owner = host.lock().unwrap();
            owner
                .owner()
                .unwrap()
                .candidate_binding(&saved.source.branch)
                .unwrap()
        };
        assert_eq!(core.export_history().unwrap(), unsaved);
        drop(core);
        drop(claim);
        let f = restart_history_fixture(f);
        let claim =
            crate::execution_claim::MachineClaim::recover_fixture(&f.temp.path("history-machine"))
                .unwrap();
        let host = f.host.owner.upgrade().unwrap();
        let (claim, _) = crate::effect_pump::recover_run(&host, claim, &run).unwrap();
        // Check the shared boundary before trying strict continuation, so the mutation
        // fails on the original publication bypass, rather than a setup refusal.
        assert!(
            refused && current == saved.source,
            "C3_ALL_CHECKPOINT_WRITERS_PAIRED close={close} error={:?}",
            reply.error
        );
        let (_, _, _, restored) = history_task_endpoint(&f, &run);
        assert_eq!(restored.export_history().unwrap(), expected);
        {
            let host = f.host.owner.upgrade().unwrap();
            let mut owner = host.lock().unwrap();
            let session = owner.owner_mut().unwrap();
            let (_, mut guard) = session
                .durable_sessions()
                .into_iter()
                .find(|(_, g)| g.branch_id() == &saved.source.branch)
                .unwrap();
            assert!(
                session.close_durable_branch(&guard).is_err(),
                "C3_DIRECT_CLOSE_BOUNDARY"
            );
            assert!(
                session.freeze_registered_candidate(&mut guard).is_err(),
                "C3_DIRECT_FREEZE_BOUNDARY"
            );
            assert_eq!(
                session.candidate_binding(&saved.source.branch).unwrap(),
                saved.source
            );
        }
        restored.request(Command::Redo).unwrap();
        assert_eq!(restored.export_history().unwrap().cursor.redo.len(), 0);
        drop(restored);
        drop(host);
        claim.finish().unwrap();
        eprintln!(
            "C3_SAVE_CLOSE_RESTART close={close} refused=true checkpoint_unchanged=true exact_history=true redo=true"
        );
    }
}

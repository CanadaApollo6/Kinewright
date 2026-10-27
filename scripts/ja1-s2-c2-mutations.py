#!/usr/bin/env python3
"""C2 targeted behavioral mutations; invoke after sourcing FFmpeg, at nice 19 with -I.

Every case restores the original bytes, runs exactly one nonignored witness in each
phase, and records source and binary hashes. Compile failures never count as reds.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
CORE = 'crates/kinewright-core/src/actor.rs'
ATTACH = 'crates/kinewright-project/src/history_save.rs'
BUNDLE = 'crates/kinewright-project/src/bundle.rs'
C = 'attachment::tests::'
P = 'linux::'


def cases():
    # Each replacement changes production code only; witness bodies stay frozen.
    return {
        'eager-binding': (CORE, 'owner = Some(requested);', 'let _ = requested;', 'core', C+'m01_eager_binding_and_install_capability_cannot_be_bypassed'),
        'separate-actor-binding': (CORE, 'owner = Some(requested);', 'let _ = requested;', 'project', 'w01_separate_actor_and_eager_binding_control'),
        'begin-floor-gate': (CORE, '''CoreMessage::BeginFloor(requested, transition, reply) => {
                let result = authorize_attachment(attachment, owner, Some(requested))''', '''CoreMessage::BeginFloor(requested, transition, reply) => {
                let result = Ok::<(), HistoryError>(())''', 'core', C+'w05_seal_checks_cut_and_all_mutation_paths'),
        'tag-floor-gate': (CORE, '''CoreMessage::TagFloor(requested, id, tag, ready, reply) => {
                let result =
                    authorize_attachment(attachment, owner, Some(requested)).and_then''', '''CoreMessage::TagFloor(requested, id, tag, ready, reply) => {
                let result =
                    Ok::<(), HistoryError>(()).and_then''', 'core', C+'w05_sealed_ticket_rejects_tag_commit_release_and_cross_actor_cut'),
        'settle-floor-gate': (CORE, '''let result =
                    authorize_attachment(attachment, owner, Some(requested)).and_then(|()| {
                        if let Some(expected)''', '''let result =
                    Ok::<(), HistoryError>(()).and_then(|()| {
                        if let Some(expected)''', 'core', C+'w05_sealed_ticket_rejects_tag_commit_release_and_cross_actor_cut'),
        'install-owner': (CORE, 'owner.is_some() && owner != requested', 'false && owner.is_some() && owner != requested', 'core', C+'m01_eager_binding_and_install_capability_cannot_be_bypassed'),
        'install-cut': (CORE, '''} else if expected
                    .as_deref()''', '''} else if false && expected
                    .as_deref()''', 'core', C+'m01_eager_binding_and_install_capability_cannot_be_bypassed'),
        'frozen-arrival': (CORE, 'attachment != AttachmentState::Live {', 'attachment == AttachmentState::Sealed {', 'core', C+'w05_seal_checks_cut_and_all_mutation_paths'),
        'sealed-commands': (CORE, '!matches!(command, Command::Query(_))', 'false && !matches!(command, Command::Query(_))', 'core', C+'w05_seal_checks_cut_and_all_mutation_paths'),
        'seal-actor-id': (CORE, 'if cut.actor != identity', 'if false', 'core', C+'w05_sealed_ticket_rejects_tag_commit_release_and_cross_actor_cut'),
        'seal-cut': (CORE, '''if cut.actor != identity
                            || cut.revision != state.revision
                            || cut.history != state.history()''', 'if false', 'core', C+'w05_seal_checks_cut_and_all_mutation_paths'),
        'sealed-owner-paths': (CORE, 'if attachment == AttachmentState::Sealed {\n        return Err', 'if false && attachment == AttachmentState::Sealed {\n        return Err', 'core', C+'w05_seal_checks_cut_and_all_mutation_paths'),
        'sealed-restore': (CORE, 'let admitted = if attachment == AttachmentState::Sealed {', 'let admitted = if false && attachment == AttachmentState::Sealed {', 'core', C+'w05_seal_checks_cut_and_all_mutation_paths'),
        'retag-ticket': (CORE, '''if pending.save.ready && !ready
                                    || tag.as_ref().is_some_and''', '''if false && pending.save.ready && !ready
                                    || false && tag.as_ref().is_some_and''', 'core', C+'w09_tickets_reject_retag_and_matching_numbers_on_another_actor'),
        'actor-ticket-collision': (CORE, '''if let Some(expected) = expected
                            &&''', '''if let Some(expected) = expected
                            && false &&''', 'core', C+'w09_tickets_reject_retag_and_matching_numbers_on_another_actor'),
        'suffix-equality': (CORE, 'if candidate.document != self.document', 'if false && candidate.document != self.document', 'core', 'actor::tests::w09_floor_commit_checks_suffix_equality_losslessly'),
        'sealed-destination-call': (ATTACH, '== kinewright_core::AttachmentState::Sealed', '== kinewright_core::AttachmentState::Frozen', 'project', P+'w09_destination_proof_and_exact_own_ticket_settlement'),
        'captured-core-fallback': ('crates/kinewright-project/src/bundle_copy.rs', '''    pub fn reconcile_destination(
        &mut self,
        live: Option<&Core>,
    ) -> Result<Option<BundleSaveReport>, BundleError> {
''', '''    pub fn reconcile_destination(
        &mut self,
        live: Option<&Core>,
    ) -> Result<Option<BundleSaveReport>, BundleError> {
        let retained = self.destination.as_ref().and_then(|d| d.pending_save.as_ref().map(|p| p.core.clone()));
        let live = live.or(retained.as_ref());
''', 'project', P+'w09_destination_proof_and_exact_own_ticket_settlement'),
        'absent-forced-commit': (ATTACH, 'let commit = crate::bundle::reconcile_save_tag_continuous(tag)?;', 'let commit = if !tag.destination.exists() { true } else { crate::bundle::reconcile_save_tag_continuous(tag)? };', 'project', P+'w09_destination_proof_and_exact_own_ticket_settlement'),
        'reconcile-phase-order': (BUNDLE, '        let observed_own = self.publication_tag.as_ref().is_some_and(|tag| {\n            tag.root_sha256 == kinewright_media::sha256_bytes(self.admission.raw_bytes())\n        });\n        if observed_own {\n            self.publication_state = PublicationState::Committed;\n        } else if self.publication_state == PublicationState::Unknown {\n            self.publication_state = PublicationState::DefinitelyRefused;\n        }\n        self.settle_destination_ticket(live)?;\n', '        self.settle_destination_ticket(live)?;\n        let observed_own = self.publication_tag.as_ref().is_some_and(|tag| {\n            tag.root_sha256 == kinewright_media::sha256_bytes(self.admission.raw_bytes())\n        });\n        if observed_own {\n            self.publication_state = PublicationState::Committed;\n        } else if self.publication_state == PublicationState::Unknown {\n            self.publication_state = PublicationState::DefinitelyRefused;\n        }\n', 'project', P+'w16_reconcile_floor_error_preserves_proven_publication'),
        'copy-reconcile-phase': ('crates/kinewright-project/src/bundle_copy.rs', 'if destination.publication_state() == crate::bundle::PublicationState::Committed', 'if false && destination.publication_state() == crate::bundle::PublicationState::Committed', 'project', P+'w16_reconcile_floor_error_preserves_proven_publication'),
        'stuck-ticket': (ATTACH, 'settle(core, &attempt.ticket, commit).map_err(crate::bundle::history_error)?;', 'let _ = commit;', 'project', P+'w16_exact_attempt_ticket_commit_release_control'),
        'foreign-actor-current-ticket': (ATTACH, '        let core = &self.core;\n', '''        let core = &self.core;
        let substituted;
        let attempt = if core.actor_identity() != attempt.actor() {
            substituted = AttemptBinding { ticket: core.pending_history_save().map_err(crate::bundle::history_error)?.unwrap() };
            &substituted
        } else { attempt };
''', 'project', P+'w09_destination_proof_and_exact_own_ticket_settlement'),
        'newer-current-ticket': (ATTACH, '        let core = &self.core;\n', '''        let core = &self.core;
        let substituted;
        let attempt = if core.pending_history_save().map_err(crate::bundle::history_error)?.as_ref().is_some_and(|p| p.actor == attempt.actor() && p.id != attempt.ticket.id) {
            substituted = AttemptBinding { ticket: core.pending_history_save().map_err(crate::bundle::history_error)?.unwrap() };
            &substituted
        } else { attempt };
''', 'project', P+'w16_exact_attempt_ticket_commit_release_control'),
        'wrong-root-proof': (BUNDLE, '        if continuous\n            && (!tagged_parent\n                || (hash.is_some()\n                    && hash.as_deref() != Some(&tag.root_sha256)\n                    && hash != tag.previous_root_sha256))', '        if continuous && !tagged_parent', 'project', P+'w09_destination_proof_and_exact_own_ticket_settlement'),
        'absent-root-proof': (BUNDLE, '        if continuous\n            && (!tagged_parent\n                || (hash.is_some()\n                    && hash.as_deref() != Some(&tag.root_sha256)\n                    && hash != tag.previous_root_sha256))', '        if continuous && (!tagged_parent || (hash.as_deref() != Some(&tag.root_sha256) && hash != tag.previous_root_sha256))', 'project', P+'w09_destination_proof_and_exact_own_ticket_settlement'),
        'absent-owner-release': (BUNDLE, '            self.settle_destination_ticket(live)?;\n            return Err(BundleError::Corrupt("missing reconciliation root".into()));', '            return Err(BundleError::Corrupt("missing reconciliation root".into()));', 'project', P+'w09_destination_proof_and_exact_own_ticket_settlement'),
        'changed-parent': (BUNDLE, '''(!tagged_parent
                ||''', '''(false
                ||''', 'project', P+'w09_changed_parent_does_not_settle_an_exact_root_at_a_new_identity'),
        'no-ticket-restore': ('crates/kinewright-project/src/receipts.rs', '        self.reconcile_destination(Some(core))?;\n', '''        self.reconcile_destination(Some(core))?;
        let checkpoint = self.branches.get(&self.manual_branch_id).unwrap();
        let history = self.store.read_record::<HistorySnapshot>(&checkpoint.history_ref)?;
        self.attach_core(core)?.restore_history((*history).clone()).map_err(crate::bundle::history_error)?;
''', 'project', P+'w10_no_ticket_destination_reconcile_keeps_live_history'),
        'extra-floor': (ATTACH, '''        Ok(true)
''', '''        let before = core.export_history().map_err(crate::bundle::history_error)?;
        if let Ok(floor) = HistoryTransition::floor(before, 1) {
            self.install_history_transition(floor).map_err(crate::bundle::history_error)?;
        }
        Ok(true)
''', 'project', P+'w16_exact_attempt_ticket_commit_release_control'),
        'fixture-seal': ('crates/kinewright-agent/src/ja_harness/s2_observer.rs', '    if revised {\n', '    if false && revised {\n', 'agent', 's2_additive_core_seal_fixture_observes_retained_clone_mutation'),
    }


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def main():
    evidence = Path(sys.argv[1]).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    selected = sys.argv[2:] or list(cases())
    identity = ROOT / 'scripts/ja1-s2-c1-evidence.py'
    subprocess.run([sys.executable, '-I', str(identity), 'identity', str(evidence/'source-before.json')], cwd=ROOT, check=True)
    for name in selected:
        file, old, new, target, witness = cases()[name]
        path = ROOT / file
        original = path.read_bytes()
        text = original.decode()
        if text.count(old) != 1:
            raise SystemExit(f'{name}: expected one replacement, got {text.count(old)}')
        directory = evidence / name
        directory.mkdir()
        (directory / 'original.rs').write_bytes(original)
        mutated = text.replace(old, new).encode()
        (directory / 'identity.json').write_text(json.dumps({'source_file': file, 'original_sha256': sha(original), 'mutated_sha256': sha(mutated), 'witness': witness}, indent=2)+'\n')
        args = ['test', '-p', 'kinewright-'+target]
        if target == 'core':
            args += ['--lib']
        else:
            args += ['--features', 'eval-harness' if target == 'agent' else 'test-util', '--test', 'ja_s1' if target == 'agent' else 'ja_s2_rescue']
        args += [witness, '--', '--exact', '--nocapture', '--test-threads=4']
        try:
            for phase in ['red', 'green']:
                path.write_bytes(mutated if phase == 'red' else original)
                started = time.monotonic()
                while int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))) < 6291456:
                    if time.monotonic()-started > 600:
                        raise SystemExit('6 GiB memory prerequisite unavailable')
                    time.sleep(5)
                command = ['nice', '-n', '10', 'cargo', *args]
                (directory / (phase+'.command.json')).write_text(json.dumps(command)+'\n')
                with (directory / (phase+'.log')).open('w') as log:
                    result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=False)
                output = (directory / (phase+'.log')).read_text()
                summary = re.findall(r'^test result:.*$', output, re.M)
                expect = r'^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' if phase == 'red' else r'^test result: ok\. 1 passed; 0 failed; 0 ignored;'
                if result.returncode != (101 if phase == 'red' else 0) or not re.search(expect, output, re.M):
                    print(output[-9000:])
                    raise SystemExit(f'{name}/{phase}: missing required single runtime outcome (exit {result.returncode})')
                executable = re.findall(r'^\s*Running .* \(([^()]+)\)$', output, re.M)
                binary = (ROOT/executable[0]).resolve()
                with binary.open('rb') as stream:
                    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
                (directory/(phase+'.result.json')).write_text(json.dumps({'exit':result.returncode, 'raw_summary':summary, 'binary':str(binary), 'binary_sha256':digest},indent=2)+'\n')
                print(f'{name}/{phase}: {summary[0]}', flush=True)
        finally:
            path.write_bytes(original)
        if path.read_bytes() != original:
            raise SystemExit('source restoration failed')

    subprocess.run([sys.executable, '-I', str(identity), 'identity', str(evidence/'source-after.json')], cwd=ROOT, check=True)
    if (evidence/'source-before.json').read_bytes() != (evidence/'source-after.json').read_bytes():
        raise SystemExit('complete source identity changed across mutations')

if __name__ == '__main__':
    main()

"""Verify retained source/log identities and report literal full-suite outcomes."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[5]
OUT = Path(__file__).resolve().parent


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    sources = {}
    for line in (OUT / 'source.sha256').read_text().splitlines():
        expected, relative = line.split('  ', 1)
        assert digest(ROOT / relative) == expected, relative
        sources[relative] = expected
    gates = {}
    for label in ['app', 'media', 'project', 'project-test-util', 'build', 'clippy', 'fmt', 'windows']:
        path = OUT / f'{label}.log.gz'
        body = gzip.open(path, 'rt', errors='replace').read()
        code = int((OUT / f'{label}.exit').read_text())
        gates[label] = {
            'command': (OUT / f'{label}.command').read_text().strip(),
            'exit': code, 'log': path.name, 'log_sha256': digest(path),
            'raw_summaries': [line for line in body.splitlines() if line.startswith('test result:')],
            'failed_tests': [line for line in body.splitlines() if line.startswith('test ') and line.endswith(' ... FAILED')],
            'not_run_messages': [line for line in body.splitlines() if 'NOT RUN' in line],
        }
        if label == 'windows':
            assert code != 0 and 'aws-lc-sys' in body
        else:
            assert code == 0, (label, code)
    pairs = json.loads((OUT / 'mutations/summary.json').read_text())
    assert len(pairs) == 16
    for row in pairs:
        assert row['runtime_red'] and row['runtime_green'] and row['restored_exactly'], row['case']
        for phase in ['red', 'green']:
            path = OUT / 'mutations' / f'{row["case"]}-{phase}.log.gz'
            assert digest(path) == row[f'{phase}_log_sha256']
    comparisons = OUT / 'parent-comparisons.json'
    report = {
        'base_commit': 'ef4526eeb5c34f615abdddcf055bd1f30f48cb4f',
        'failure_comparison_commit': 'c7224d8b4795e6534c020471cf326e8a5a92a853',
        'source_and_fixture_generator_sha256': sources,
        'gates': gates,
        'full_suite_failure_comparisons': json.loads(comparisons.read_text()) if comparisons.exists() else [],
        'mutation_count': 16, 'new_pairs': 2, 'prior_pairs_rerun': 14,
        'mutation_details': 'mutations/summary.json',
        'all_sources_exactly_restored': True,
        'lint': (OUT / 'slow-lint.log').read_text().strip(),
        'manifest_selftest_log': 'slow-selftest.log',
        'owner_teardown_boundary': 'Normal teardown tries every slot. A failed settle or crash may discard unpublished derived ASR. Acknowledged audio/log remain durable, and a note without a committed original ASR must reopen transcribable and editable. No durable pending-ASR recovery storage was added.',
        'diagnostic_repro': 'repro-c722-handle uses c722 App handle code plus one witness, against current dependencies with the new owner slots unused. It is a mechanism reproducer, not a full baseline-tree gate or a credited mutation pair.',
        'not_run': [
            'full workspace aggregate, slow tier and forced ignores',
            'Media integration targets and doctests (requested Media gate is full serial lib)',
            'native Windows Rust compilation/runtime beyond aws-lc-sys prerequisites',
            'real Whisper/model-installed spoken note and commentary controls; no model downloads',
            'actual microphone, presenter-displayed frame and recorded-audio clock integration',
            'native model/workspace and capture-staging resource accounting/cleanup',
            'C6/shared-host streamed ingress, private range enforcement, incomplete-status UI and G11 App closure',
            'persisted remark split/merge/re-anchor/delete, collection/reopen and post-edit mutation',
            'native filesystem unknown-outcome fault injection (committed lost reply is emulated)',
            'actual process-crash pending-ASR recovery control (failed normal teardown is exercised)',
            'live S4 specialist and equipped GPU/provider/media-pack lanes',
            'hosted CI', 'laptop offload (probe timeout124; Tailscale authentication required)',
            'independent review rounds (excluded for this implementation worker)',
        ],
    }
    (OUT / 'gate-summary.json').write_text(json.dumps(report, indent=2) + '\n')
    for label, gate in gates.items():
        print(label, 'exit', gate['exit'], *gate['raw_summaries'], sep='\n')
    print('16 runtime red/restored-green pairs; exact source and log hashes verified')


if __name__ == '__main__':
    main()

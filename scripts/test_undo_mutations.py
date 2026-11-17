#!/usr/bin/env python3
"""Fault checks for the mutation runner; no Cargo or product processes run."""
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('undo_mutations', ROOT / 'scripts/ja1-s2-c6-mutations.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class MutationEvidenceTests(unittest.TestCase):
    def fixture(self, directory):
        source = directory / 'subject.rs'
        source.write_text('original\n')
        binary = directory / 'fake-test'
        binary.write_text('''#!/bin/sh
if [ "$1" = --list ]; then
  printf 'fixture::witness: test\\n'
else
  printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\\n'
fi
''')
        binary.chmod(0o755)
        row = dict(name='fault', status='retargeted', package='kinewright-project', target='--lib',
                   witness='fixture::witness', diagnostic=None, environment={}, child_app=False,
                   edits=[dict(file='subject.rs', old='original', new='mutated')])
        section = dict(original_count=1, cases=[row], original_names_sha256=hashlib.sha256(
            json.dumps(['fault']).encode()).hexdigest())
        artifact = dict(executable=str(binary), profile=dict(test=True))
        return source, section, artifact

    def run_fixture(self, directory, section, builder):
        def identity(path):
            path.write_bytes(b'fixture identity\n')
        with mock.patch.object(runner, 'ROOT', directory), \
                mock.patch.object(runner, 'PREFIX', []), \
                mock.patch.object(runner, 'catalogue', return_value=section), \
                mock.patch.object(runner, 'build_binary', side_effect=builder), \
                mock.patch.object(runner, 'source_identity', side_effect=identity), \
                mock.patch.object(runner.subprocess, 'check_output', return_value=b'fixture compiler\n'), \
                mock.patch.object(sys, 'argv', ['runner', str(directory / 'evidence'), 'fault']):
            runner.main()

    def test_build_failure_restores_the_mutated_source_and_is_not_a_runtime_red(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'target', prefix='mutation-runner-') as temp:
            directory = Path(temp)
            source, section, artifact = self.fixture(directory)
            def builder(_args, _directory, label):
                if label == 'red-build':
                    self.assertEqual(source.read_text(), 'mutated\n')
                    raise SystemExit('build prerequisite failed; NOT a runtime red')
                return artifact
            with self.assertRaisesRegex(SystemExit, 'NOT a runtime red'):
                self.run_fixture(directory, section, builder)
            self.assertEqual(source.read_text(), 'original\n', 'BUILD_FAILURE_RESTORES_SOURCE')
            self.assertFalse((directory / 'evidence/fault/red.result.json').exists())

    def test_surviving_mutant_restores_source_and_cannot_earn_a_red(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'target', prefix='mutation-runner-') as temp:
            directory = Path(temp)
            source, section, artifact = self.fixture(directory)
            with self.assertRaisesRegex(SystemExit, 'expected one runtime outcome'):
                self.run_fixture(directory, section, lambda *_: artifact)
            self.assertEqual(source.read_text(), 'original\n', 'SURVIVOR_RESTORES_SOURCE')
            self.assertFalse((directory / 'evidence/fault/red.result.json').exists())

    def test_an_original_case_cannot_disappear_through_retirement(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'target', prefix='mutation-runner-') as temp:
            directory = Path(temp)
            _, section, _ = self.fixture(directory)
            section['cases'] = []
            with mock.patch.object(runner, 'catalogue', return_value=section), \
                    self.assertRaisesRegex(SystemExit, 'original case accounting changed'):
                runner.check('c6')


if __name__ == '__main__':
    unittest.main()

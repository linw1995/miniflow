import copy
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

import pr_validation as validation


class ValidationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.git('init', '-q', '-b', 'main')
        for key, value in [('user.name', 'Fixture'), ('user.email', 'fixture@example.com'),
                           ('commit.gpgsign', 'false'), ('core.hooksPath', '/dev/null')]:
            self.git('config', key, value)
        (self.root / '.gitignore').write_text('/target/\n')
        self.base = self.commit('base')
        self.heads, self.merges = [], []
        for index in range(3):
            head = self.commit(str(index))
            merge = self.git('commit-tree', self.git('rev-parse', 'HEAD^{tree}'), '-p', self.base, '-p', head, '-m', 'merge')
            self.heads.append(head)
            self.merges.append(merge)
        self.git('checkout', '-q', '--detach', self.merges[-1])
        self.current = validation.current_context(self.root, 'owner/repo', 7,
            {'id': 9, 'run_attempt': 1, 'workflow_id': 2}, {'coverage_target': 95, 'policy': validation.POLICY})

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, stderr=subprocess.PIPE, text=True).strip()

    def commit(self, content):
        (self.root / 'source.rs').write_text(content + '\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'test(ci): fixture')
        return self.git('rev-parse', 'HEAD')

    def candidate(self, index, run_id=1, attempt=1):
        run = {'id': run_id, 'run_attempt': attempt, 'workflow_id': 2, 'head_sha': self.heads[index],
               'status': 'completed', 'conclusion': 'success'}
        record = self.current | {'run_id': run_id, 'run_attempt': attempt, 'head': self.heads[index],
                                'merge': self.merges[index], 'tree': self.git('rev-parse', self.merges[index] + '^{tree}')}
        record['comparison'] = validation.cumulative(record, 'First validation')
        record['packages'] = ['sample']
        record['coverage'] = {'status': 'passed', 'base': self.base, 'head': record['merge'], 'tree': record['tree'],
                              'target': 95, 'total': 100, 'covered': 95}
        return run, record

    def test_nearest_ancestor_ignores_completion_order_and_unsuccessful_intermediates(self):
        a, b = self.candidate(0, 10), self.candidate(1, 5)
        result = validation.choose_baseline(self.root, self.current, [a, b])
        self.assertEqual(result['base'], self.merges[1])
        for status, conclusion in [('completed', 'failure'), ('completed', 'cancelled'), ('in_progress', None)]:
            b[0].update(status=status, conclusion=conclusion)
            self.assertEqual(validation.choose_baseline(self.root, self.current, [b, a])['base'], self.merges[0])
        self.assertEqual(validation.choose_baseline(self.root, self.current, [self.candidate(2)])['base'], self.merges[2])

    def test_rejects_identity_policy_attempt_and_coverage_mismatches(self):
        candidate = self.candidate(1)
        mutations = [
            ('repository', 'other/repo'), ('pr', 8), ('version', 2), ('workflow_id', 3),
            ('run_attempt', 2), ('head', self.heads[0]), ('base', self.heads[0]),
            ('merge', 'f' * 40), ('tree', self.base), ('signature', {'coverage_target': 90}),
            ('packages', None), ('coverage', {}),
        ]
        for field, value in mutations:
            with self.subTest(field=field):
                run, record = copy.deepcopy(candidate)
                record[field] = value
                self.assertFalse(validation.eligible(self.root, self.current, run, record))
        for field, value in [('covered', 94), ('status', 'failed'), ('head', self.base), ('total', -1), ('target', 90)]:
            with self.subTest(coverage=field):
                run, record = copy.deepcopy(candidate)
                record['coverage'][field] = value
                self.assertFalse(validation.eligible(self.root, self.current, run, record))
        run, record = copy.deepcopy(candidate)
        record['coverage'].update(covered=0, total=0, status='not_applicable')
        self.assertTrue(validation.eligible(self.root, self.current, run, record))
        record['coverage']['status'] = 'passed'
        self.assertFalse(validation.eligible(self.root, self.current, run, record))

    def test_rewritten_history_and_target_base_reset_to_cumulative_scope(self):
        candidate = self.candidate(1)
        other = self.git('commit-tree', self.git('rev-parse', 'HEAD^{tree}'), '-p', self.base, '-m', 'rewritten')
        current = self.current | {'head': other}
        self.assertEqual(validation.choose_baseline(self.root, current, [candidate])['scope'], 'cumulative')
        for override in [{'base': self.heads[0]}, {'signature': {'coverage_target': 96}}]:
            current = self.current | override
            result = validation.choose_baseline(self.root, current, [candidate])
            self.assertEqual(result['base'], current['base'])
            self.assertEqual(result['scope'], 'cumulative')

    def test_artifact_discovery_is_bounded_and_attempt_specific(self):
        run, record = self.candidate(1, attempt=2)
        def archive(record, name=validation.RECORD_FILE):
            stream = io.BytesIO()
            with zipfile.ZipFile(stream, 'w') as zipped:
                zipped.writestr(name, json.dumps(record))
            return stream.getvalue()
        api = Mock()
        api.request.side_effect = [{'workflow_runs': [run]}, {'artifacts': [
            {'id': 1, 'name': 'pr-validation-1-1', 'expired': False},
            {'id': 2, 'name': validation.artifact_name(run), 'expired': False},
        ]}]
        download = Mock(return_value=archive(record))
        result = validation.discover_baseline(self.root, self.current, api, 'feature', download)
        self.assertEqual(result['base'], self.merges[1])
        download.assert_called_once_with(2)
        for response in [OSError('offline'), {'workflow_runs': [run] * 100}, {'workflow_runs': []}]:
            api = Mock()
            if isinstance(response, Exception): api.request.side_effect = response
            else: api.request.return_value = response
            self.assertEqual(validation.discover_baseline(self.root, self.current, api, 'feature', download)['scope'], 'cumulative')
        api = Mock()
        api.request.return_value = {'workflow_runs': [run]}
        with patch.object(validation.time, 'monotonic', side_effect=[0, 61]):
            self.assertEqual(validation.discover_baseline(self.root, self.current, api, 'feature', download)['scope'], 'cumulative')
        for expired, payload in [(True, archive(record)), (False, b'invalid'), (False, archive(record, '../other')), (False, archive(record | {'merge': 'f' * 40}))]:
            api = Mock()
            api.request.side_effect = [{'workflow_runs': [run]}, {'artifacts': [
                {'id': 2, 'name': validation.artifact_name(run), 'expired': expired}]}]
            self.assertEqual(validation.discover_baseline(self.root, self.current, api, 'feature', lambda _: payload)['scope'], 'cumulative')

    def test_current_snapshot_and_signature_require_valid_execution_identity(self):
        with patch.object(validation, 'command', return_value='rustc fixture'), patch.dict(validation.os.environ, {}, clear=True):
            signature = validation.execution_signature(self.root, 95)
            self.assertEqual(signature['environment'], {})
            self.assertNotIn('Cargo.lock', signature)
            for invalid in [-1, 101, float('nan')]:
                with self.assertRaises(ValueError): validation.execution_signature(self.root, invalid)
        (self.root / 'untracked').write_text('dirty')
        with self.assertRaisesRegex(ValueError, 'clean'):
            validation.current_context(self.root, 'owner/repo', 7, {'id': 9}, {})
        self.git('clean', '-f')
        self.git('checkout', '-q', '--detach', self.heads[-1])
        with self.assertRaisesRegex(ValueError, 'two-parent'):
            validation.current_context(self.root, 'owner/repo', 7, {'id': 9}, {})


if __name__ == '__main__':
    unittest.main()

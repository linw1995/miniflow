import contextlib
import copy
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

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

    def test_current_snapshot_and_signature_require_valid_execution_identity(self):
        with patch.object(validation, 'command', return_value='rustc fixture'), patch.dict(validation.os.environ, {}, clear=True):
            signature = validation.execution_signature(self.root, 95)
            self.assertEqual(len(signature['environment']), 5)
            self.assertTrue(all(value is None for value in signature['environment'].values()))
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

    def coverage_inputs(self):
        current = copy.deepcopy(self.current)
        current['signature'].update(toolchain='fixture', features='all', target=None, profile='ci', environment={})
        comparison = {'base': current['base'], 'head': current['merge'], 'scope': 'cumulative', 'previous': None, 'reason': 'fixture'}
        plan = {'base': comparison['base'], 'head': comparison['head'], 'comparison_mode': 'snapshot',
                'packages': ['sample'], 'ignored_files': []}
        report = self.root / 'target/result/lcov.info'
        report.parent.mkdir(parents=True, exist_ok=True)
        identity = {'head': current['merge'], 'tree': current['tree'], 'dirty': False, 'initial_profiles': [], 'input_environment': {},
                    'toolchain': 'fixture', 'features': 'all', 'target': None, 'profile': 'ci'}
        report.with_name('source.json').write_text(json.dumps(identity))
        return current, comparison, plan, report

    def test_incremental_coverage_counts_only_changed_lines_and_merges_duplicate_records(self):
        current, comparison, plan, report = self.coverage_inputs()
        report.write_text('SF:source.rs\nDA:1,0\nLF:1\nend_of_record\n')
        result = validation.evaluate_coverage(self.root, current, comparison, plan, report)
        self.assertEqual((result['status'], result['covered'], result['total']), ('failed', 0, 1))
        report.write_text(report.read_text() + 'SF:source.rs\nDA:1,2\nLF:1\nend_of_record\n')
        result = validation.evaluate_coverage(self.root, current, comparison, plan, report)
        self.assertEqual((result['status'], result['covered'], result['total']), ('passed', 1, 1))
        exact = plan | {'packages': [], 'tests': [{'package': 'sample', 'binary': 'sample', 'name': 'changed'}]}
        self.assertEqual(validation.evaluate_coverage(self.root, current, comparison, exact, report), result)
        comparison['base'] = plan['base'] = current['merge']
        self.assertEqual(validation.evaluate_coverage(self.root, current, comparison, plan, report)['status'], 'not_applicable')
        plan['packages'] = []
        report.unlink()
        self.assertEqual(validation.evaluate_coverage(self.root, current, comparison, plan, report)['total'], 0)

    def test_nonexecutable_proof_does_not_hide_functions_or_custom_macros(self):
        for source in ['// Documentation\n', 'mod model;\npub use model::{Item, Other as Alias};',
                       '#[doc(hidden)]\npub use inventory::submit as __submit_builtin_flow;',
                       '#[cfg(all(test, unix))]\n#[path = "../tests/common.rs"]\nmod support;',
                       '#[macro_use]\npub mod nodes;\nextern crate sample as _;']:
            with self.subTest(source=source): self.assertTrue(validation.declarations_only(source))
        for source in ['fn execute() {}', 'pub mod inner { pub fn execute() {} }',
                       '#[custom_attribute]\npub mod model;', 'inventory::submit! { Item }',
                       '#[doc(hidden)]\nfn hidden() {}',
                       '#[doc(hidden)]\ninventory::submit! { Item }',
                       '#[doc(hidden)]\n#[custom_attribute]\npub use model::Item;',
                       '#[doc = r#"[ \" //"#]\nfn hidden() {}',
                       '#[path = "//"] fn hidden() {}', 'pub static VALUE: u32 = run();']:
            with self.subTest(source=source): self.assertFalse(validation.declarations_only(source))

    def test_missing_stale_and_malformed_coverage_cannot_pass(self):
        current, comparison, plan, report = self.coverage_inputs()
        for content in ['', 'DA:1,1\n', 'SF:source.rs\nDA:1,1\n',
                        'SF:source.rs\nDA:1,-1\nend_of_record\n',
                        'SF:source.rs\nDA:1,1\nLF:2\nend_of_record\n',
                        'SF:other.rs\nDA:1,1\nend_of_record\n']:
            with self.subTest(content=content), self.assertRaises(ValueError):
                report.write_text(content)
                validation.evaluate_coverage(self.root, current, comparison, plan, report)
        report.unlink()
        with self.assertRaises(OSError): validation.evaluate_coverage(self.root, current, comparison, plan, report)
        report.write_text('SF:source.rs\nDA:1,1\nend_of_record\n')
        identity = json.loads(report.with_name('source.json').read_text())
        report.with_name('source.json').write_text(json.dumps(identity | {'head': self.base}))
        with self.assertRaisesRegex(ValueError, 'stale'): validation.evaluate_coverage(self.root, current, comparison, plan, report)
        report.with_name('source.json').write_text(json.dumps(identity))
        with self.assertRaisesRegex(ValueError, 'identities'):
            validation.evaluate_coverage(self.root, current, comparison, plan | {'base': self.heads[0]}, report)
        with self.assertRaisesRegex(ValueError, 'no selected'):
            validation.evaluate_coverage(self.root, current, comparison, plan | {'packages': []}, report)
        (self.root / 'source.rs').write_text('dirty')
        with self.assertRaisesRegex(ValueError, 'checkout'):
            validation.evaluate_coverage(self.root, current, comparison, plan, report)

    def test_declaration_changes_preserve_every_unmapped_executable_item(self):
        bodies = [
            'fn run() { let text = r#"pub use hidden::New;"#; }',
            'macro_rules! emit { () => { pub fn run() { invoke(); } } }',
            'inventory::submit! { Registration { factory: execute } }',
            'pub mod inner { use super::Old; pub fn run() { invoke(); } }',
        ]
        for body in bodies:
            with self.subTest(body=body):
                before = 'pub use old::{Item, Other};\n' + body
                after = '#[doc(hidden)]\npub use new::{Item, Other};\n' + body + '\nmod added;'
                self.assertTrue(validation.declaration_changes_only(before, after))
                self.assertFalse(validation.declaration_changes_only(before, after.replace(body, body + '\nfn added() {}')))
                self.assertFalse(validation.declaration_changes_only(before, after.replace(body, body.replace('{', '{ changed();', 1))))
                self.assertFalse(validation.declaration_changes_only(before, after + '\n#[custom]\nmod generated;'))
        self.assertFalse(validation.declaration_changes_only('fn run() { use old::Item; }',
                                                             'fn run() { use new::Item; }'))
        with patch.object(validation.subprocess, 'run', side_effect=OSError('unavailable')):
            self.assertFalse(validation.declaration_changes_only('mod old;', 'mod new;'))

    def test_unmapped_module_changes_require_unchanged_executable_items(self):
        source = self.root / 'lib.rs'
        source.write_text('pub use old::Item;\nmacro_rules! emit { () => { run(); } }\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'test(ci): unmapped base')
        base = self.git('rev-parse', 'HEAD')
        source.write_text('pub use new::Item;\nmacro_rules! emit { () => { run(); } }\nmod added;\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'test(ci): declaration changes')
        head = self.git('rev-parse', 'HEAD')
        merge = self.git('commit-tree', self.git('rev-parse', 'HEAD^{tree}'), '-p', base, '-p', head, '-m', 'merge')
        self.git('checkout', '-q', '--detach', merge)
        self.current = validation.current_context(self.root, 'owner/repo', 7,
            {'id': 9, 'run_attempt': 1, 'workflow_id': 2}, {'coverage_target': 95, 'policy': validation.POLICY})
        current, comparison, plan, report = self.coverage_inputs()
        report.write_text('SF:source.rs\nDA:1,1\nend_of_record\n')
        result = validation.evaluate_coverage(self.root, current, comparison, plan, report)
        self.assertEqual(result['status'], 'not_applicable')
        self.assertEqual(result['non_executable_files'], ['lib.rs'])

    def test_test_code_lines_cover_only_positive_test_scopes(self):
        source = (
            'pub fn production() -> u32 { 1 }\n'
            '#[cfg(test)]\nmod tests {\n'
            '    fn helper() {}\n'
            '    #[test]\n    fn one() {}\n'
            '}\n'
            '#[tokio::test]\nasync fn async_case() {}\n'
            '#[cfg(feature = "test")]\nfn feature_code() {}\n'
            '#[cfg(not(test))]\nfn non_test_code() {}\n')
        lines = validation.test_code_lines(source)
        self.assertNotIn(1, lines)
        self.assertTrue({3, 4, 5, 6, 7} <= lines)
        self.assertTrue({8, 9} <= lines)
        self.assertNotIn(11, lines)
        self.assertNotIn(13, lines)
        with patch.object(validation.subprocess, 'run', side_effect=OSError('unavailable')):
            self.assertEqual(validation.test_code_lines(source), set())

    def test_test_code_changes_are_exempt_but_production_lines_are_not(self):
        source = self.root / 'source.rs'
        source.write_text(
            'pub fn production() -> u32 { 1 }\n'
            '#[cfg(test)]\nmod tests {\n'
            '    #[test]\n    fn one() { assert!(true); }\n'
            '}\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'test(ci): rust base')
        base = self.git('rev-parse', 'HEAD')
        source.write_text(
            'pub fn production() -> u32 { 2 }\n'
            '#[cfg(test)]\nmod tests {\n'
            '    #[test]\n    fn one() { helper(); }\n'
            '    fn helper() { assert!(true); }\n'
            '}\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'test(ci): rust head')
        head = self.git('rev-parse', 'HEAD')
        merge = self.git('commit-tree', self.git('rev-parse', 'HEAD^{tree}'), '-p', base, '-p', head, '-m', 'merge')
        self.git('checkout', '-q', '--detach', merge)
        self.current = validation.current_context(self.root, 'owner/repo', 7,
            {'id': 9, 'run_attempt': 1, 'workflow_id': 2}, {'coverage_target': 95, 'policy': validation.POLICY})
        current, comparison, plan, report = self.coverage_inputs()
        comparison['base'] = plan['base'] = base
        # Changed test lines are validated by execution; only the changed
        # production line participates in the coverage denominator.
        report.write_text('SF:source.rs\nDA:1,0\nDA:5,1\nDA:6,1\nend_of_record\n')
        result = validation.evaluate_coverage(self.root, current, comparison, plan, report)
        self.assertEqual((result['covered'], result['total']), (0, 1))
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['test_only_files'], [])
        report.write_text(report.read_text().replace('DA:1,0', 'DA:1,1'))
        result = validation.evaluate_coverage(self.root, current, comparison, plan, report)
        self.assertEqual((result['covered'], result['total'], result['status']), (1, 1, 'passed'))
        previous = merge

        source.write_text(
            'pub fn production() -> u32 { 2 }\n'
            '#[cfg(test)]\nmod tests {\n'
            '    #[test]\n    fn one() { helper(); helper(); }\n'
            '    fn helper() { assert!(true); }\n'
            '}\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'test(ci): test-only head')
        head = self.git('rev-parse', 'HEAD')
        merge = self.git('commit-tree', self.git('rev-parse', 'HEAD^{tree}'), '-p', previous, '-p', head, '-m', 'merge')
        self.git('checkout', '-q', '--detach', merge)
        self.current = validation.current_context(self.root, 'owner/repo', 7,
            {'id': 9, 'run_attempt': 1, 'workflow_id': 2}, {'coverage_target': 95, 'policy': validation.POLICY})
        current, comparison, plan, report = self.coverage_inputs()
        comparison['base'] = plan['base'] = previous
        report.write_text('SF:source.rs\nDA:1,1\nDA:5,1\nDA:6,1\nend_of_record\n')
        result = validation.evaluate_coverage(self.root, current, comparison, plan, report)
        self.assertEqual((result['total'], result['status']), (0, 'not_applicable'))
        self.assertEqual(result['test_only_files'], ['source.rs'])

    def test_new_renamed_deleted_and_comment_only_sources(self):
        previous = self.current['merge']
        self.git('mv', 'source.rs', 'renamed source.rs')
        (self.root / 'comment.rs').write_text('// No executable code.\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'test(ci): rename and delete')
        head = self.git('rev-parse', 'HEAD')
        merge = self.git('commit-tree', self.git('rev-parse', 'HEAD^{tree}'), '-p', self.base, '-p', head, '-m', 'merge')
        self.git('checkout', '-q', '--detach', merge)
        self.current = validation.current_context(self.root, 'owner/repo', 7,
            {'id': 9, 'run_attempt': 1, 'workflow_id': 2}, {'coverage_target': 95, 'policy': validation.POLICY})
        current, comparison, plan, report = self.coverage_inputs()
        comparison['base'] = plan['base'] = previous
        report.write_text('SF:renamed source.rs\nDA:1,1\nend_of_record\n')
        result = validation.evaluate_coverage(self.root, current, comparison, plan, report)
        self.assertEqual((result['covered'], result['total']), (1, 1))
        self.assertEqual(result['non_executable_files'], ['comment.rs'])
        self.assertNotIn('source.rs', validation.changed_lines(self.root, previous, merge))

    def test_final_record_requires_all_jobs_title_coverage_and_exact_attempt(self):
        current, comparison, plan, report = self.coverage_inputs()
        report.write_text('SF:source.rs\nDA:1,1\nend_of_record\n')
        coverage = validation.evaluate_coverage(self.root, current, comparison, plan, report)
        needs = {name: {'result': 'success'} for name in ['lint', 'check', 'coverage']}
        run = {'id': 9, 'run_attempt': 1, 'workflow_id': 2, 'head_sha': current['head']}
        title = {'id': 3, 'head_sha': current['head'], 'conclusion': 'success', 'name': 'pr-title', 'app': {'slug': 'github-actions'}}
        record = validation.make_record(self.root, current, comparison, plan, coverage, needs, run, title)
        self.assertEqual(record['title_check']['id'], 3)
        for job in needs:
            for result in ['failure', 'cancelled', 'skipped']:
                with self.subTest(job=job, result=result), self.assertRaises(ValueError):
                    validation.make_record(self.root, current, comparison, plan, coverage, needs | {job: {'result': result}}, run, title)
        for changed_run, changed_title, changed_coverage in [
            (run | {'run_attempt': 2}, title, coverage),
            (run, title | {'conclusion': 'failure'}, coverage),
            (run, title, coverage | {'status': 'failed'}),
        ]:
            with self.assertRaises(ValueError):
                validation.make_record(self.root, current, comparison, plan, changed_coverage, needs, changed_run, changed_title)

    def test_accept_binds_flow_output_without_discarding_exact_tests(self):
        current, comparison, plan, _ = self.coverage_inputs()
        signature = current['signature']
        plan.update(tree=current['tree'], tests=[], toolchain=signature['toolchain'], build_environment={},
                    configuration={'target': None, 'all_features': True, 'features': [], 'no_default_features': False})
        output = {'plan': plan, 'validation': {'context': current, 'comparison': comparison}}
        def accept(value):
            with patch.object(validation, 'execution_signature', return_value=signature):
                return validation.accept_plan(self.root, value, 'owner/repo', 7, 9, 1, 95)
        self.assertEqual(accept(output), (current, comparison, plan))
        exact = copy.deepcopy(output)
        exact['plan'].update(packages=[], baseline_packages=['sample'],
                             tests=[{'package': 'sample', 'binary': 'sample', 'name': 'test'}])
        self.assertEqual(accept(exact)[2], exact['plan'])
        for change in ['attempt', 'source', 'exact', 'features', 'toolchain', 'environment', 'scope', 'local']:
            invalid = copy.deepcopy(output)
            if change == 'attempt': invalid['validation']['context']['run_attempt'] = 2
            elif change == 'source': invalid['plan']['tree'] = '0' * 40
            elif change == 'exact': invalid['plan']['tests'] = [{'package': 'unknown', 'binary': 'sample', 'name': 'test'}]
            elif change == 'features': invalid['plan']['configuration']['all_features'] = False
            elif change == 'toolchain': invalid['plan']['toolchain'] = 'other'
            elif change == 'environment': invalid['plan']['build_environment'] = {'RUSTFLAGS': 'other'}
            elif change == 'scope': invalid['validation']['comparison']['base'] = self.heads[0]
            else: invalid['validation'] = None
            with self.subTest(change=change), self.assertRaises((ValueError, TypeError, KeyError)):
                accept(invalid)

    def test_cli_accept_empty_evaluate_and_record_require_one_matching_flow_result(self):
        self.base = self.current['merge']
        (self.root / 'docs').mkdir()
        (self.root / 'docs/guide.md').write_text('documentation\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'test(ci): docs increment')
        head = self.git('rev-parse', 'HEAD')
        merge = self.git('commit-tree', self.git('rev-parse', 'HEAD^{tree}'), '-p', self.base, '-p', head, '-m', 'merge')
        self.git('checkout', '-q', '--detach', merge)
        signature = {'policy': validation.POLICY, 'coverage_target': 95, 'target': None, 'environment': {},
                     'toolchain': 'fixture', 'profile': 'ci', 'features': 'all'}
        run = {'id': 9, 'run_attempt': 1, 'workflow_id': 2, 'head_sha': head}
        current = validation.current_context(self.root, 'owner/repo', 7, run, signature)
        comparison = {'base': self.base, 'head': merge, 'scope': 'cumulative', 'previous': None, 'reason': 'fixture'}
        plan = {'base': self.base, 'head': merge, 'tree': current['tree'], 'comparison_mode': 'snapshot',
                'packages': [], 'tests': [], 'ignored_files': ['docs/guide.md'], 'reasons': {}, 'fallback_reasons': [],
                'toolchain': 'fixture', 'build_environment': {},
                'configuration': {'target': None, 'all_features': True, 'features': [], 'no_default_features': False}}
        directory = self.root / 'target/validation'
        directory.mkdir(parents=True)
        flow = directory / 'flow.json'
        flow.write_text(json.dumps({'plan': plan, 'validation': {'context': current, 'comparison': comparison}}))
        event = self.root / 'target/event.json'
        event.write_text(json.dumps({'number': 7}))
        env = {'GITHUB_REPOSITORY': 'owner/repo', 'GITHUB_RUN_ID': '9', 'GITHUB_RUN_ATTEMPT': '1',
               'GITHUB_EVENT_PATH': str(event), 'GITHUB_EVENT_NAME': 'pull_request', 'GH_TOKEN': 'fixture',
               'GITHUB_OUTPUT': str(self.root / 'target/output'), 'GITHUB_STEP_SUMMARY': str(self.root / 'target/summary')}
        def invoke(phase, api, extra=()):
            with patch.dict(validation.os.environ, env, clear=True), patch.object(validation.Path, 'cwd', return_value=self.root), \
                    patch('ci_monitor.GitHub', return_value=api), patch.object(validation, 'execution_signature', return_value=signature), \
                    patch.object(validation.sys, 'argv', ['pr_validation.py', phase, '--directory', str(directory), *extra]), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                return validation.main()
        api = Mock()
        self.assertEqual(invoke('accept', api, ['--flow-output', str(flow), '--coverage-target', '95']), 0)
        api.request.assert_not_called()
        self.assertIn('run=false', (self.root / 'target/output').read_text())
        self.assertEqual(invoke('evaluate', api), 0)
        self.assertEqual(json.loads((directory / 'coverage.json').read_text())['status'], 'not_applicable')
        env['VALIDATION_NEEDS'] = json.dumps({name: {'result': 'success'} for name in ['lint', 'check', 'coverage']})
        title = {'id': 11, 'head_sha': head, 'conclusion': 'success', 'name': 'pr-title', 'app': {'slug': 'github-actions'}}
        api.request.side_effect = [run, {'check_runs': [title]}]
        self.assertEqual(invoke('record', api), 0)
        self.assertTrue((directory / 'validation.json').is_file())
        flow.write_text(flow.read_text() + '\n{}')
        self.assertEqual(invoke('accept', Mock(), ['--flow-output', str(flow), '--coverage-target', '95']), 1)


if __name__ == '__main__':
    unittest.main()

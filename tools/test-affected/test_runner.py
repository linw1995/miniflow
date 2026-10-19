import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("plan_runner", Path(__file__).with_name("run-tests.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class RunnerTests(unittest.TestCase):
    def test_matchers_escape_filter_syntax_and_empty_plans_do_not_run_cargo(self):
        self.assertEqual(runner.matcher("case) | all()"), "=case\\u{29}\\u{20}\\u{7c}\\u{20}all\\u{28}\\u{29}")
        self.assertEqual(runner.filterset([], []), "none()")
        with patch.object(runner, 'verify'), patch.object(runner, 'execute') as execute, contextlib.redirect_stdout(io.StringIO()):
            runner.consume({'packages': [], 'tests': []})
        execute.assert_not_called()

    def test_rejects_partial_or_unstructured_plans(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'plan.json'
            for value in [{}, {'packages': ['sample']}, {'plan': {}}, ['cargo', 'test']]:
                path.write_text(json.dumps(value))
                with self.assertRaises(runner.PlanError):
                    runner.load_plan(path)
                with patch.object(sys, 'stdin', io.StringIO(json.dumps(value))):
                    with self.assertRaises(runner.PlanError):
                        runner.load_plan(Path('-'))

    def test_child_diagnostics_do_not_mix_with_the_machine_report(self):
        code = '''import json, pathlib, runpy, sys
execute = runpy.run_path(sys.argv[1])['execute']
execute([sys.executable, '-c', 'import sys; print("child-out"); print("child-err", file=sys.stderr)'],
        pathlib.Path.cwd(), capture=False)
print(json.dumps({'executed': True}))
'''
        result = subprocess.run([sys.executable, '-c', code, runner.__file__], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {'executed': True})
        self.assertIn('child-out', result.stderr)
        self.assertIn('child-err', result.stderr)

    @unittest.skipUnless(os.name == 'posix', 'Unix process groups')
    def test_cancellation_reaps_the_owned_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = root / 'child.py'
            pidfile = root / 'pid'
            child.write_text('''import json, os, pathlib, subprocess, sys, time
ready = pathlib.Path(sys.argv[1] + '.ready')
descendant = subprocess.Popen([sys.executable, '-c',
    "import pathlib, signal, sys, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); pathlib.Path(sys.argv[1]).touch(); time.sleep(60)", str(ready)])
while not ready.exists():
    time.sleep(0.01)
ready.write_text(json.dumps([os.getpid(), descendant.pid]))
ready.replace(sys.argv[1])
time.sleep(60)
''')
            helper = root / 'helper.py'
            helper.write_text('''import importlib.util, pathlib, signal, sys
spec = importlib.util.spec_from_file_location('runner', sys.argv[1])
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
def stop(*_):
    raise KeyboardInterrupt
signal.signal(signal.SIGTERM, stop)
try:
    r.execute([sys.executable, sys.argv[2], sys.argv[3]], pathlib.Path.cwd())
except KeyboardInterrupt:
    sys.exit(130)
''')
            process = subprocess.Popen([sys.executable, str(helper), runner.__file__, str(child), str(pidfile)], cwd=root)
            try:
                deadline = time.monotonic() + 5
                while not pidfile.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(pidfile.exists())
                process.send_signal(signal.SIGTERM)
                self.assertEqual(process.wait(timeout=5), 130)
                for pid in json.loads(pidfile.read_text()):
                    deadline = time.monotonic() + 2
                    while time.monotonic() < deadline:
                        try:
                            os.kill(pid, 0)
                        except ProcessLookupError:
                            break
                        time.sleep(0.01)
                    else:
                        self.fail(f"owned process {pid} survived cancellation")
            finally:
                if pidfile.exists():
                    with contextlib.suppress(ProcessLookupError):
                        os.killpg(json.loads(pidfile.read_text())[0], signal.SIGKILL)
                if process.poll() is None:
                    process.kill()
                    process.wait()


if __name__ == '__main__':
    unittest.main()

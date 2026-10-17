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

    @unittest.skipUnless(os.name == 'posix', 'Unix process groups')
    def test_cancellation_reaps_the_owned_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = root / 'child.py'
            pidfile = root / 'pid'
            child.write_text("import os, pathlib, sys, time\npathlib.Path(sys.argv[1]).write_text(str(os.getpid()))\ntime.sleep(60)\n")
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
                with self.assertRaises(ProcessLookupError):
                    os.kill(int(pidfile.read_text()), 0)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()


if __name__ == '__main__':
    unittest.main()

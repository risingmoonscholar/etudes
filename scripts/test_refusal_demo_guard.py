"""The refusal demo scripts refuse to start outside CI.

site/refusal/run.sh builds sweep 0.4.0 and runs apply with it; record.sh and
beats.sh run a 0.4.0 binary. That version uses the login keychain, so both must stop before
anything is built or run unless GITHUB_ACTIONS=true.

No real git, cargo or sweep runs here. Fake `git`, `cargo` and sweep binaries
sit first on PATH (or in OLD_SWEEP/NEW_SWEEP); each writes a marker file and
fails. The refusal test asserts the marker never appears. A control opens the
guard and asserts the marker DOES appear, so the fakes are proven to be the
programs the scripts would have reached.
"""
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
RUN = ROOT / "site/refusal/run.sh"
RECORD = ROOT / "site/refusal/record.sh"
BEATS = ROOT / "site/refusal/beats.sh"


class Sandbox:
    def __init__(self, directory):
        self.base = Path(directory)
        self.marker = self.base / "marker"
        self.bin = self.base / "bin"
        self.bin.mkdir()
        # Every program the scripts could reach that matters, plus the slow
        # or screen-touching ones record.sh calls, so the control stays quick.
        for name in ("git", "cargo", "sweep", "godot", "clear", "sleep"):
            self.fake(self.bin / name, fails=name in ("git", "cargo", "sweep"))
        self.fake(self.base / "old-sweep", fails=True)
        self.fake(self.base / "new-sweep", fails=True)
        for folder in ("home", "fx", "pristine"):
            (self.base / folder).mkdir()

    def fake(self, path, fails):
        if path.name in ("clear", "sleep"):
            body = "#!/bin/sh\nexit 0\n"
        else:
            body = f'#!/bin/sh\necho "$0 $*" >> "{self.marker}"\nexit {1 if fails else 0}\n'
        path.write_text(body)
        path.chmod(0o755)

    def env(self, ci):
        env = {
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "HOME": str(self.base / "home"),
            "TMPDIR": str(self.base) + "/",
            "OLD_SWEEP": str(self.base / "old-sweep"),
            "NEW_SWEEP": str(self.base / "new-sweep"),
            "FIXTURE": str(self.base / "fx"),
            "PRISTINE": str(self.base / "pristine"),
            "TERM": "dumb",
        }
        if ci is not None:
            env["GITHUB_ACTIONS"] = ci
        return env

    def run(self, script, ci, *args):
        process = subprocess.Popen(["/bin/bash", str(script), *args], env=self.env(ci),
                                   cwd=self.base, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   start_new_session=True)
        try:
            out, err = process.communicate(timeout=60)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            out, err = process.communicate()
        return process.returncode, out.decode(), err.decode()


class RefusalDemoGuardTests(unittest.TestCase):
    def check_refuses(self, script, ci, *args):
        with tempfile.TemporaryDirectory() as directory:
            box = Sandbox(directory)
            code, _, err = box.run(script, ci, *args)
            self.assertNotEqual(0, code, err)
            self.assertIn("refusing to start", err)
            self.assertIn("sweep 0.4.0", err)
            self.assertIn("login keychain", err)
            self.assertIn("runs only in CI", err)
            self.assertFalse(box.marker.exists(),
                             f"{script.name} reached a program before refusing: "
                             f"{box.marker.read_text() if box.marker.exists() else ''}")

    def test_run_refuses_outside_ci(self):
        self.check_refuses(RUN, None)
        self.check_refuses(RUN, "false")

    def test_record_refuses_outside_ci(self):
        self.check_refuses(RECORD, None)
        self.check_refuses(RECORD, "")

    def test_beats_refuses_outside_ci(self):
        for beat in ("apply", "undo", "plan"):
            self.check_refuses(BEATS, None, beat)
        self.check_refuses(BEATS, "1", "apply")

    def test_the_fakes_are_what_the_scripts_would_reach(self):
        # Control: with the guard open, each script reaches a fake (which
        # fails), so a missing marker above means the guard stopped it.
        for script, args in ((RUN, ()), (RECORD, ()), (BEATS, ("apply",))):
            with tempfile.TemporaryDirectory() as directory:
                box = Sandbox(directory)
                box.run(script, "true", *args)
                self.assertTrue(box.marker.exists(),
                                f"{script.name} reached no fake with the guard open; "
                                "the refusal test would prove nothing")


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Product experiments, separate from the unchanged stress witnesses.

Run on macOS as an ordinary user after cargo build --release:
  python3 crates/journal-dump/diagnostics/mid_apply.py target/release/sweep
All paths, keys and journals are synthetic and private to a temporary directory.
"""
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess
import sys
import tempfile
import time


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def stopped(pid):
    result = subprocess.run(['ps', '-o', 'state=', '-p', str(pid)],
                            capture_output=True, text=True, check=True)
    return 'T' in result.stdout


def snapshot(root):
    return {str(p.relative_to(root)): p.read_bytes()
            for p in root.rglob('*') if p.is_file()}


def trial(binary, mode):
    with tempfile.TemporaryDirectory(prefix='etudes-mid-apply-') as tmp:
        base = Path(tmp)
        root, state = base / 'Desktop', base / 'state'
        root.mkdir()
        state.mkdir()
        for i in range(500):
            value = i
            stem = ''
            for _ in range(5):
                stem = chr(97 + value % 26) + stem
                value //= 26
            (root / f'batch_{stem}.csv').write_bytes(f'payload {i}\n'.encode())
        original = snapshot(root)
        env = dict(os.environ, SWEEP_GRACE_SECS="0", ETUDE_STATE_DIR=str(state),
                   ETUDE_JOURNAL_KEY=secrets.token_hex(32))
        plan = subprocess.run([binary, str(root), '--json'], env=env,
                              capture_output=True, text=True, check=True)
        members = next(g['members'] for g in json.loads(plan.stdout)['details']['groups']
                       if g['name'] == 'Data')
        target = Path(members[375])
        process = subprocess.Popen([binary, 'apply', str(root), '--yes'], env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 10
            while process.poll() is None and time.monotonic() < deadline:
                if (root / 'Data').exists() and any((root / 'Data').iterdir()):
                    os.kill(process.pid, signal.SIGSTOP)
                    while not stopped(process.pid):
                        require(time.monotonic() < deadline, 'SIGSTOP was not acknowledged')
                        time.sleep(.001)
                    break
                time.sleep(.001)
            require(process.poll() is None and stopped(process.pid), 'no stopped live apply')
            held = list((root / 'Data').glob('*.csv'))
            require(0 < len(held) < 375 and target.is_file(), 'not an early partial state')
            require(len(list(state.glob('sweep-*.journal'))) == 1,
                    'missing initial journal: cannot establish accepted plan')
            before = len(held)
            if mode == 'delete':
                target.unlink()  # Non-idempotent: failure cannot pretend to be deletion.
                require(not target.exists() and not (root / 'Data' / target.name).exists(),
                        'target was relocated rather than deleted')
            elif mode == 'chmod':
                root.chmod(0o555)
                require(root.stat().st_mode & 0o777 == 0o555, 'chmod failed')
                try:
                    with (root / 'write-control').open('xb'):
                        pass
                except PermissionError:
                    pass
                else:
                    raise RuntimeError('current user can still write: invalid permission injection')
            elif mode == 'rm-race':
                require(target.is_file(), 'initial -f predicate must be true')
            os.kill(process.pid, signal.SIGCONT)
            out, err = process.communicate(timeout=30)
            if mode in ('rm-race', 'late-chmod'):
                require(process.returncode == 0, 'control apply did not complete')
                if mode == 'rm-race':
                    # Force precisely the interleaving allowed between -f and rm -f.
                    subprocess.run(['rm', '-f', str(target)], check=True)
                    require(not target.exists(), 'final ! -e predicate must be true')
                else:
                    root.chmod(0o555)
                    require(root.stat().st_mode & 0o777 == 0o555, 'late chmod failed')
                require(len(snapshot(root)) == 500, 'successful control lost a file')
            else:
                require(process.returncode != 0 and 'sweep:' in err, 'failure was swallowed')
                require(len(snapshot(root)) == (499 if mode == 'delete' else 500),
                        'unexpected loss or duplicate')
            root.chmod(0o755)
            moved = len(list((root / 'Data').glob('*.csv')))
            undo = subprocess.run([binary, 'undo'], env=env, capture_output=True, text=True)
            require(undo.returncode == 0, f'undo failed: {undo.stderr}')
            expected = dict(original)
            if mode == 'delete':
                del expected[target.name]
            require(snapshot(root) == expected, 'undo did not recover exact survivor paths and bytes')
            require('already gone' not in undo.stdout.lower(), 'phantom journal entry')
            return dict(mode=mode, stopped_after_moves=before, apply_exit=process.returncode,
                        moved=moved, files_after_undo=len(expected), exact_recovery=True)
        finally:
            root.chmod(0o755)
            if process.poll() is None:
                process.kill()
                process.wait()


if __name__ == '__main__':
    require(sys.platform == 'darwin' and os.geteuid() != 0, 'requires non-root macOS')
    binary = str(Path(sys.argv[1]).resolve())
    for mode in ('delete', 'chmod', 'rm-race', 'late-chmod'):
        print(json.dumps(trial(binary, mode)), flush=True)

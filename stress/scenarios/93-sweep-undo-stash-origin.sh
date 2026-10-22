# Sweep undo retains a created folder needed by a live stash operation.
# Selection, undo and pop preserve all original paths and bytes.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require python3 "fixture comparisons need Python" || exit 0
W=$(workdir)
trap 'rm -rf "$W"' EXIT
if python3 - "$SWEEP" "$STASH" "$UNPACK" "$W" <<'PY'
import json, os, pathlib, secrets, subprocess, sys, tarfile
sweep, stash, unpack = sys.argv[1:4]
w = pathlib.Path(sys.argv[4]); root = w/'root'; root.mkdir(); hold = w/'hold'; hold.mkdir()
env = dict(os.environ, ETUDE_JOURNAL_KEY=secrets.token_hex(32), ETUDE_STATE_DIR=str(w/'state'))
expected = {}
for n in range(3):
    name = f'Screenshot 2024-01-01 at 00.00.0{n}.png'
    data = bytes(range(256)) + str(n).encode(); expected[name] = data
    path = root/name; path.write_bytes(data); os.utime(path, (1704067200, 1704067200))
def run(binary, *args, allowed=(0,)):
    result = subprocess.run([binary, *map(str, args)], env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True)
    assert result.returncode in allowed, (args, result.returncode, result.stdout, result.stderr)
    return result
run(sweep, 'apply', root, '--yes', '--since', '0')
folder = root/'Screenshots'; assert folder.is_dir()
run(stash, 'select', *sorted(folder.glob('*.png')), '--into', hold)
result = run(sweep, 'undo', root, '--json')
assert folder.is_dir(), 'live stash origin was removed'
assert json.loads(result.stdout)['details']['directories_left_for_journals'] == 1
run(stash, 'pop', '--latest')
assert {p.name:p.read_bytes() for p in folder.iterdir()} == expected
run(sweep, 'undo', root)
assert {p.name:p.read_bytes() for p in root.iterdir()} == expected
# The shared journal decoder also remains usable during archive extraction.
source = w/'source'; source.mkdir(); (source/'member.txt').write_bytes(b'archive member')
with tarfile.open(w/'fixture.tar.gz', 'w:gz') as archive:
    archive.add(source/'member.txt', arcname='member.txt')
run(unpack, w/'fixture.tar.gz', '--into', w/'out')
assert (w/'out'/'member.txt').read_bytes() == b'archive member'
PY
then pass "live stash origin survives sweep undo and both tools restore exact paths and bytes"; else fail "journal origin retention or byte restoration failed"; fi

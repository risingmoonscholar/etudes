#!/usr/bin/env bash
# Inspection exposes opaque item ids, and restoring one leaves two held.
# A wrong item id restores nothing; restoring the rest returns every original byte.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
W=$(workdir) || exit 1; trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
CODE=0
python3 -u - "$STASH" "$W" > "$W/checks" <<'PY' || CODE=$?
import json, pathlib, subprocess, sys
binary, work = sys.argv[1], pathlib.Path(sys.argv[2])
root = work / 'PRIVATE-source'
root.mkdir()
original = {f'PRIVATE-name-{n}.txt': f'synthetic bytes {n}'.encode() for n in range(3)}
for name, data in original.items():
    (root / name).write_bytes(data)
def run(*args):
    return subprocess.run([binary, *map(str, args), '--json'], capture_output=True, text=True)
def check(condition, label):
    assert condition, label
    print(label)
def restored():
    return {p.name: p.read_bytes() for p in root.iterdir() if p.is_file() and not p.name.startswith('.')}
result = run(root)
check(result.returncode == 0 and not restored(), 'stash holds all three original files')
operation = json.loads(result.stdout)['details']['stash_id']
result = run('inspect', '--id', operation)
details = json.loads(result.stdout)['details']
items = details['items']
check(result.returncode == 0 and details['total'] == 3 and details['held'] == 3 and len(items) == 3,
      'inspection reports exactly three held items')
ids = [item['id'] for item in items]
check(len(set(ids)) == 3 and all(value and '/' not in value and '\\' not in value for value in ids)
      and 'PRIVATE' not in result.stdout + result.stderr and details['paths_shown'] is False,
      'inspection exposes unique path-free ids without source names')
result = run('pop', '--id', operation, '--item', 'item-does-not-exist')
check(result.returncode == 2 and json.loads(result.stdout)['status'] == 'refused'
      and json.loads(result.stdout)['reason']['code'] == 'unknown_item' and not restored(),
      'a wrong item id is refused as unknown and restores nothing')
result = run('pop', '--id', operation, '--item', ids[0])
first = restored()
check(result.returncode == 0 and len(first) == 1 and all(original[name] == data for name, data in first.items()),
      'one selected item returns exactly one original file with its bytes')
result = run('inspect', '--id', operation)
details = json.loads(result.stdout)['details']
check(result.returncode == 0 and details['held'] == 2 and details['total'] == 3
      and sum(item['held'] for item in details['items']) == 2
      and next(item for item in details['items'] if item['id'] == ids[0])['held'] is False,
      'inspection reports two held items after the partial restore')
result = run('pop', '--id', operation)
check(result.returncode == 0 and restored() == original, 'restoring the rest returns all three original names and bytes')
check(sorted(p.name for p in root.iterdir()) == sorted(original), 'the completed restore removes its empty holding folder')
PY
while IFS= read -r label; do pass "$label"; done < "$W/checks"
[ "$CODE" -eq 0 ] || fail "stash item contract failed (Python exit $CODE)"

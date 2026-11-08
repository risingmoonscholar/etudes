#!/usr/bin/env bash
# Apply emits versioned path-free progress on stderr while JSON stdout stays one envelope.
# The terminal counts describe all three moved and journalled files; unsupported commands refuse.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
W=$(workdir) || exit 1; trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
CODE=0
python3 -u - "$SWEEP" "$W" > "$W/checks" <<'PY' || CODE=$?
import json, pathlib, subprocess, sys
binary, work = sys.argv[1], pathlib.Path(sys.argv[2])
root = work / 'PRIVATE-root'
root.mkdir()
original = {f'report-{n}.pdf': f'synthetic {n}'.encode() for n in range(3)}
for name, data in original.items():
    (root / name).write_bytes(data)
def run(*args):
    return subprocess.run([binary, *map(str, args)], capture_output=True, text=True)
def check(condition, label):
    assert condition, label
    print(label)
result = run('apply', root, '--yes', '--progress-json', '--json')
check(result.returncode == 0, 'progress-enabled apply succeeds')
envelope = json.loads(result.stdout)
check(envelope['status'] == 'done' and envelope['details']['moved'] == 3,
      'JSON stdout is exactly one successful result envelope reporting three moves')
records = [json.loads(line) for line in result.stderr.splitlines() if line.lstrip().startswith('{')]
fields = {'schema_version', 'event', 'tool_version', 'operation_id', 'operation', 'phase', 'planned', 'completed', 'journalled'}
check(2 <= len(records) <= 12 and all(set(record) == fields and record['schema_version'] == 1
      and record['event'] == 'progress' and record['operation'] == 'apply'
      and record['operation_id'] == envelope['operation_id'] and record['tool_version'] == envelope['tool_version']
      and record['phase'] in ('planning', 'advancing', 'done', 'error') for record in records),
      'stderr progress uses schema one and the result operation id within the documented record bound')
check(all(all(type(record[key]) is int and 0 <= record[key] <= 3 for key in ('planned', 'completed', 'journalled'))
          and record['journalled'] <= record['completed'] <= record['planned'] for record in records)
      and all(a['completed'] <= b['completed'] and a['journalled'] <= b['journalled'] for a, b in zip(records, records[1:])),
      'progress counts are bounded and monotonically record completed and journalled moves')
check(records[0]['phase'] == 'planning' and (records[0]['planned'], records[0]['completed'], records[0]['journalled']) == (3, 0, 0)
      and records[-1]['phase'] == 'done' and (records[-1]['planned'], records[-1]['completed'], records[-1]['journalled']) == (3, 3, 3),
      'planning starts at zero moves and done reports exactly three durable moves')
check(all('PRIVATE' not in json.dumps(record) and not any(name in json.dumps(record) for name in original) for record in records),
      'progress records disclose no fixture paths or source names')
files = [p for p in root.rglob('*') if p.is_file()]
check(len(files) == 3 and {p.name: p.read_bytes() for p in files} == original
      and all(p.parent != root for p in files), 'all three reported moves exist on disk with their original bytes')
for args, label in (((root,), 'scan'), (('status',), 'status'), (('verify',), 'verify')):
    result = run(*args, '--progress-json', '--json')
    check(result.returncode == 2 and 'progress' in result.stdout + result.stderr,
          f'{label} refuses the unsupported progress flag')
check({p.name: p.read_bytes() for p in root.rglob('*') if p.is_file()} == original
      and all(not (root / name).exists() for name in original), 'unsupported commands move and restore nothing')
# Undo takes the flag now, through the same layer, and the envelope stays one line.
result = run('undo', root, '--progress-json', '--json')
records = [json.loads(line) for line in result.stderr.splitlines() if line.startswith('{')]
check(result.returncode == 0 and len(result.stdout.strip().splitlines()) == 1
      and records and all(r['operation'] == 'undo' for r in records) and records[-1]['phase'] == 'done'
      and records[-1]['journalled'] == 3,
      'undo reports three durable reversals on stderr and one result on stdout')
check({p.name: p.read_bytes() for p in root.iterdir() if p.is_file()} == original, 'undo put all three files back')
PY
while IFS= read -r label; do pass "$label"; done < "$W/checks"
[ "$CODE" -eq 0 ] || fail "structured progress contract failed (Python exit $CODE)"

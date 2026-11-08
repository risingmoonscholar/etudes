#!/usr/bin/env bash
# Existing-folder routes move only eligible files into their folders; a missing
# route folder is created only when files move into it and removed by undo.
# Invalid profiles and stale exported plans refuse without changing the tree.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
W=$(workdir); trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
CODE=0
python3 -u - "$SWEEP" "$W" > "$W/checks" <<'PY' || CODE=$?
import json, os, pathlib, subprocess, sys
binary, work = sys.argv[1], pathlib.Path(sys.argv[2])
profile = pathlib.Path(os.environ['ETUDE_STATE_DIR']) / 'sweep-existing-profile.toml'
root = work / 'files'
root.mkdir()
(root / 'Reading').mkdir()
original = {name: name.encode() for name in ('a.pdf', 'b.pdf', 'c.pdf', 'passport.pdf')}
for name, data in original.items():
    (root / name).write_bytes(data)
def run(*args):
    return subprocess.run([binary, *map(str, args)], capture_output=True, text=True)
def check(condition, label):
    assert condition, label
    print(label)
def tree():
    return sorted((str(p.relative_to(root)), p.read_bytes() if p.is_file() else None)
                  for p in root.rglob('*'))
def write_profile(body):
    profile.write_text('schema_version = 1\n' + body)
route = '[[routes]]\nextension = "pdf"\nfolder = "Reading"\n'
before = tree()
result = run(root, '--scheme', 'existing', '--json')
check(not profile.exists() and tree() == before, 'a scan never creates the absent profile or moves files')
write_profile(route)
result = run('apply', root, '--scheme', 'existing', '--yes', '--json')
check(result.returncode == 0, 'a valid profile applies from ETUDE_STATE_DIR')
check(all((root / 'Reading' / name).is_file() and (root / 'Reading' / name).read_bytes() == original[name] and not (root / name).exists()
          for name in ('a.pdf', 'b.pdf', 'c.pdf')), 'exactly three routed files reach the existing folder')
check((root / 'passport.pdf').read_bytes() == original['passport.pdf'], 'the personal-record safety hold wins over the route')
result = run(root, '--scheme', 'existing', '--depth', '2', '--json')
details = json.loads(result.stdout)['details']
check(result.returncode == 1 and not details['groups'] and details['left_alone']['in_place'] == 3,
      'depth two reports the three routed files in place without proposing them again')
check(run('undo', root).returncode == 0 and tree() == before, 'undo restores the original names and bytes')
shelf = root / 'Missing destination'
write_profile(route.replace('Reading', 'Missing destination'))
result = run(root, '--scheme', 'existing', '--json')
check(result.returncode == 0 and tree() == before and
      '"name":"Missing destination","signal":".pdf (saved route, new folder sweep creates)"' in result.stdout,
      'a plan lists a missing route folder as one sweep creates, and makes nothing')
result = run('apply', root, '--scheme', 'existing', '--yes', '--json')
check(result.returncode == 0 and shelf.is_dir() and
      all((shelf / name).read_bytes() == original[name] and not (root / name).exists() for name in ('a.pdf', 'b.pdf', 'c.pdf')),
      'apply creates the missing route folder and moves the routed files into it with their bytes')
check((root / 'passport.pdf').read_bytes() == original['passport.pdf'], 'the safety hold still wins over a created route folder')
check(run('undo', root).returncode == 0 and tree() == before and not shelf.exists(),
      'undo restores every file and removes the empty folder sweep created')
write_profile(route + '[[routes]]\nextension = "xyz"\nfolder = "Never Made"\n')
result = run('apply', root, '--scheme', 'existing', '--yes', '--json')
check(result.returncode == 0 and not (root / 'Never Made').exists() and 'Never Made' not in result.stdout + result.stderr,
      'a route with no files here creates no folder and is not mentioned')
check(run('undo', root).returncode == 0 and tree() == before, 'undo after a run with an unused route restores the tree')
held = root / 'Held'
held.write_bytes(b'synthetic file holding the name')
holding = tree()
write_profile(route.replace('Reading', 'Held'))
result = run('apply', root, '--scheme', 'existing', '--yes', '--json')
check(result.returncode == 2 and tree() == holding and json.loads(result.stdout)['reason']['code'] == 'profile_invalid'
      and 'which is a file, not a folder, so nothing moved' in result.stderr and '"Held"' not in result.stderr,
      'a file holding the route name refuses by name without moving files')
held.unlink()
for body, label, named in (
    (route + route, 'duplicate routes refuse without moving files', None),
    ('[[routes]]\nextension = [\n', 'malformed routes refuse without moving files', None),
):
    write_profile(body)
    result = run('apply', root, '--scheme', 'existing', '--yes', '--json')
    check(result.returncode == 2 and tree() == before and
          (named is None or named in result.stdout + result.stderr), label)
write_profile(route)
(root / 'c.pdf').unlink()
small = tree()
result = run('apply', root, '--scheme', 'existing', '--yes', '--json')
check(result.returncode == 1 and tree() == small, 'two eligible routed files do not form a group')
(root / 'c.pdf').write_bytes(original['c.pdf'])
plan = work / 'plan'
result = run(root, '--scheme', 'existing', '--export-plan', plan, '--json')
check(result.returncode == 0 and plan.is_file() and tree() == before, 'exporting a routed plan moves nothing')
digest = json.loads(result.stdout)['details']['binding']['plan_digest']
profile.write_text(profile.read_text() + '\n# changed after export\n')
result = run('apply', '--plan', plan, '--plan-digest', digest, '--yes', '--json')
check(result.returncode == 2 and tree() == before, 'a changed profile refuses exported-plan replay without moving files')
PY
while IFS= read -r label; do pass "$label"; done < "$W/checks"
[ "$CODE" -eq 0 ] || fail "existing-folder contract failed (Python exit $CODE)"

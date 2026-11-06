#!/usr/bin/env bash
# Routing advice works as printed; saved profiles are private and invalidate replay.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
W=$(workdir); trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
CODE=0
mkdir -p "$W/home"
HOME="$W/home" python3 -u - "$SWEEP" "$W" > "$W/checks" <<'PY' || CODE=$?
import json, os, pathlib, shlex, subprocess, sys
binary, work = sys.argv[1], pathlib.Path(sys.argv[2])
root = work / 'files'; root.mkdir()
(root / 'Blender Files').mkdir()
profile = pathlib.Path(os.environ['ETUDE_STATE_DIR']) / 'sweep-existing-profile.toml'
for name in ('one.bpy', 'two.bpy', 'three.bpy', 'passport.bpy'):
    (root / name).write_bytes(b'synthetic')
def run(*args):
    r = subprocess.run([binary, *map(str, args)], capture_output=True, text=True)
    assert 'panicked' not in r.stdout + r.stderr
    return r
def check(ok, label):
    assert ok, label
    print(label)
r = run(root, '--since', '0')
check('3 files had no group.' in r.stdout and 'one.bpy' not in r.stdout, 'hint counts eligible unclaimed files without names')
command = next(line.split(': ', 1)[1] for line in r.stdout.splitlines() if 'To send' in line)
args = shlex.split(command)
check(args[0] == 'sweep' and run(*args[1:]).returncode == 0, 'the exact printed hint makes a usable plan')
check('To send' not in run(root, '--since', '0', '--quiet').stdout, 'quiet suppresses routing advice')
hint = json.loads(run(root, '--since', '0', '--json').stdout)['details']['routing_hint']
check(hint['count'] == 3 and hint['extensions'] == ['bpy'] and hint['command'] == command, 'JSON advice is structured and agrees with text')
r = run('apply', root, '--since', '0', '--map', 'bpy=Blender Files', '--save-routes', '--yes')
check(r.returncode == 0 and 'Saved route .bpy → Blender Files' in r.stdout and str(profile) in r.stdout, 'successful apply names the saved route and profile')
check(profile.stat().st_mode & 0o777 == 0o600, 'saved profile is mode 0600')
check((root / 'passport.bpy').exists() and (root / 'Blender Files/one.bpy').exists(), 'personal-record holds win over saved routes')
r = run('undo', root)
check(r.returncode == 0 and (root / 'one.bpy').exists(), 'the printed folder-specific undo works')
plan = work / 'plan'
r = run(root, '--since', '0', '--scheme', 'existing', '--export-plan', plan, '--json')
check(r.returncode == 0, 'the reader accepts the saved profile')
digest = json.loads(r.stdout)['details']['binding']['plan_digest']
(root / 'Models').mkdir()
for n in range(3): (root / f'{n}.blendx').write_bytes(b'synthetic')
r = run('apply', root, '--since', '0', '--map', 'blendx=Models', '--save-routes', '--yes')
check(r.returncode == 0, 'a second route is added to the whole existing profile')
r = run('apply', '--plan', plan, '--plan-digest', digest, '--yes', '--json')
check(r.returncode == 2 and json.loads(r.stdout)['reason']['code'] == 'plan_stale', 'saving a route invalidates an older profile-bound plan')
run('undo', root)
before = profile.read_bytes()
(root / 'Other').mkdir()
r = run('apply', root, '--since', '0', '--map', 'bpy=Other', '--save-routes', '--yes')
check(r.returncode == 2 and 'Blender Files' in r.stderr and 'Other' in r.stderr and profile.read_bytes() == before and (root / 'one.bpy').exists(), 'conflicting routes name both folders and refuse before moves')
for flags in (['--save-routes'], ['--save-routes', '--map', 'bpy=Blender Files', '--scheme', 'tags'], ['--save-routes', '--map', 'bpy=Blender Files', '--no-journal'], ['--save-routes', '--map', 'bpy=Missing']):
    r = run('apply', root, '--yes', '--since', '0', *flags)
    check(r.returncode == 2 and profile.read_bytes() == before and (root / 'one.bpy').exists(), 'invalid save request refuses without changing files or profile: ' + ' '.join(flags))
(root / 'Blender Files/one.bpy').write_bytes(b'occupied')
r = run('apply', root, '--since', '0', '--map', 'bpy=Blender Files', '--save-routes', '--yes')
check(r.returncode != 0 and 'routes were not saved' in r.stderr and profile.read_bytes() == before, 'failed apply explicitly saves nothing')
check(not (profile.parent / '.sweep-existing-profile.pending').exists(), 'failed apply cleans its private temporary file')
PY
while IFS= read -r label; do pass "$label"; done < "$W/checks"
[ "$CODE" -eq 0 ] || fail "saved routes contract failed (Python exit $CODE)"

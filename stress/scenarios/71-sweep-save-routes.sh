#!/usr/bin/env bash
# Routing advice works as printed; saved profiles are private and invalidate replay.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
W=$(workdir) || exit 1; trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
CODE=0
mkdir -p "$W/home"
HOME="$W/home" python3 -u - "$SWEEP" "$W" > "$W/checks" <<'PY' || CODE=$?
import json, os, pathlib, shlex, signal, subprocess, sys, time
binary, work = sys.argv[1], pathlib.Path(sys.argv[2])
root = work / 'files'; root.mkdir()
(root / 'Blender Files').mkdir()
# Sweep prints the state folder as given; TMPDIR may end in '/', so compare raw text.
profile_text = os.path.join(os.environ['ETUDE_STATE_DIR'], 'sweep-existing-profile.toml')
profile = pathlib.Path(profile_text)
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
lines = r.stdout.splitlines()
at = lines.index('3 files had no group.')
hint_lines = lines[at:at + 3]
check(hint_lines[1] == 'To route the .bpy files, name a folder after bpy= (sweep creates it if missing):' and hint_lines[2].startswith('  sweep '), 'the hint is a short sentence, then the command on its own line')
check(all(len(line) < 90 for line in hint_lines[:2]) and len(hint_lines[2]) - len(shlex.split(hint_lines[2])[-1]) < 60, 'no line of the hint is wide beyond the folder the person typed')
printed = hint_lines[2].strip()
args = shlex.split(printed)
check(args[0] == 'sweep' and run(*args[1:]).returncode == 0, 'the exact printed hint makes a usable plan')
check(all('Blender' not in line for line in hint_lines) and "--map 'bpy=Routed Files'" in printed, 'the hint never names a folder sweep found')
# Typed as a relative path from its parent, the hint prints it that way and runs from there.
rel = subprocess.run([binary, 'files', '--since', '0'], capture_output=True, text=True, cwd=work)
assert 'panicked' not in rel.stdout + rel.stderr
rel_step = next(line.strip() for line in rel.stdout.splitlines() if line.startswith('  sweep '))
rel_run = subprocess.run([binary, *shlex.split(rel_step)[1:]], capture_output=True, text=True, cwd=work)
check(rel_step.endswith(' -- files') and rel_run.returncode == 0 and 'Routed Files' in rel_run.stdout, 'the hint shows the folder as typed and runs as printed from the same place')
check('To route' not in run(root, '--since', '0', '--quiet').stdout, 'quiet suppresses routing advice')
quiet = run(root, '--since', '0', '--quiet', '--json')
command = json.loads(quiet.stdout)['details']['routing_hint']['command']
check(shlex.split(command)[:-1] == args[:-1] and shlex.split(command)[-1] == str(root) and 'To route' not in quiet.stderr, 'quiet JSON keeps the structured hint, with the resolved folder')
hint = json.loads(run(root, '--since', '0', '--json').stdout)['details']['routing_hint']
check(hint['count'] == 3 and hint['extensions'] == ['bpy'] and hint['command'] == command, 'JSON advice is structured and agrees with text')
small = work / 'small'; small.mkdir(); (small / 'single.pdf').write_bytes(b'synthetic')
r = run(small, '--since', '0')
check('1 file had no group.' in r.stdout and 'single.pdf' not in r.stdout and 'sweep propose --help' in r.stdout, 'small built-in groups get private per-file routing advice')
check(run('propose', '--help').returncode == 0, 'the per-file routing advice works as printed')
r = run('apply', root, '--since', '0', '--map', 'bpy=Blender Files', '--save-routes', '--yes')
check(r.returncode == 0 and 'Saved route .bpy → Blender Files. Undo moves the files back but keeps this route.' in r.stdout and profile_text in r.stdout, 'successful apply names the saved route, the profile, and that undo keeps it')
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
# A route saved for this tree does not block saving in a tree without its folder.
elsewhere = work / 'elsewhere'; (elsewhere / 'Notes').mkdir(parents=True)
for n in range(3): (elsewhere / f'{n}.mdx').write_bytes(b'synthetic')
r = run('apply', elsewhere, '--since', '0', '--map', 'mdx=Notes', '--save-routes', '--yes', '--json')
saved = json.loads(r.stdout)['details']['routes_saved'] if r.returncode == 0 else {}
check(saved.get('state') == 'saved' and saved.get('added') == [{'extension': 'mdx', 'folder': 'Notes'}] and saved.get('profile') == profile_text and saved.get('written') is True, 'a saved route elsewhere does not block, and JSON reports the save')
run('undo', elsewhere)
# The person's own text survives a save, and a save with nothing new writes nothing.
profile.write_text('# routes I keep\n' + profile.read_text())
kept = profile.read_bytes(); inode = profile.stat().st_ino
r = run('apply', elsewhere, '--since', '0', '--map', 'mdx=Notes', '--save-routes', '--yes')
check(r.returncode == 0 and 'Route .mdx → Notes was already saved; the profile was not changed.' in r.stdout and profile.read_bytes() == kept and profile.stat().st_ino == inode, 'nothing new leaves the profile untouched')
run('undo', elsewhere)
(elsewhere / 'Sketches').mkdir()
for n in range(3): (elsewhere / f'{n}.skx').write_bytes(b'synthetic')
# A killed run's leftover pending file does not block the next save.
(profile.parent / '.sweep-existing-profile.pending').write_bytes(b'left by a killed run')
r = run('apply', elsewhere, '--since', '0', '--map', 'skx=Sketches', '--save-routes', '--yes')
check(r.returncode == 0 and profile.read_bytes().startswith(kept) and not (profile.parent / '.sweep-existing-profile.pending').exists(), 'comments are kept and a stale pending file is cleared')
run('undo', elsewhere)
# Save fails after every file moved: pause apply, change the profile, resume.
race = work / 'race'; (race / 'Race').mkdir(parents=True)
for n in range(300): (race / f'{n:03}.rcx').write_bytes(b'synthetic')
proc = subprocess.Popen([binary, 'apply', race, '--since', '0', '--map', 'rcx=Race', '--save-routes', '--yes', '--json', '--progress-json'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
paused = False
tried = False
err_lines = []
for err_line in proc.stderr:
    err_lines.append(err_line)
    if not paused and not tried and '"phase":"advancing"' in err_line:
        tried = True
        # Pause, confirm the process is stopped, then inject; never inject
        # into a running process, so a late stop is unproven, not a failure.
        os.kill(proc.pid, signal.SIGSTOP)
        stopped = False
        for _ in range(200):
            state = subprocess.run(['ps', '-o', 'state=', '-p', str(proc.pid)], capture_output=True, text=True).stdout
            if 'T' in state:
                stopped = True
                break
            time.sleep(0.01)
        if stopped:
            with open(profile, 'a') as handle: handle.write('# edited while sweep was moving files\n')
            paused = True
        os.kill(proc.pid, signal.SIGCONT)
out = proc.stdout.read(); proc.wait()
err = ''.join(err_lines)
assert 'panicked' not in out + err
if not paused:
    print('UNPROVEN apply finished, or was not confirmed stopped, before the pause could land')
else:
    result = json.loads(out)
    check(proc.returncode == 3 and result['status'] == 'incomplete' and result['reason'] == {'code': 'profile_write_failed', 'retry': 'after_change', 'subject': {'kind': 'profile', 'path': profile_text}}, 'a save that fails after moves names the profile, not the journal')
    check('the files moved as reported above, but the route was not saved' in err and result['details']['routes_saved']['reason'] == 'profile_changed' and (race / 'Race/000.rcx').exists(), 'the message says the files moved and the route was not saved')
    lines = err.split('add these lines to the end of ' + profile_text + ':\n\n', 1)[1].split('\n\n', 1)[0]
    with open(profile, 'a') as handle: handle.write('\n' + lines + '\n')
    run('undo', race)
    r = run('apply', race, '--since', '0', '--map', 'rcx=Race', '--save-routes', '--yes')
    check(r.returncode == 0 and 'Route .rcx → Race was already saved' in r.stdout, 'the printed lines save the route by hand')
    run('undo', race)
before = profile.read_bytes()
(root / 'Other').mkdir()
r = run('apply', root, '--since', '0', '--map', 'bpy=Other', '--save-routes', '--yes')
check(r.returncode == 2 and 'Blender Files' in r.stderr and 'Other' in r.stderr and profile.read_bytes() == before and (root / 'one.bpy').exists(), 'conflicting routes name both folders and refuse before moves')
for flags in (['--save-routes'], ['--save-routes', '--map', 'bpy=Blender Files', '--scheme', 'tags'], ['--save-routes', '--map', 'bpy=Blender Files', '--no-journal']):
    r = run('apply', root, '--yes', '--since', '0', *flags)
    check(r.returncode == 2 and profile.read_bytes() == before and (root / 'one.bpy').exists(), 'invalid save request refuses without changing files or profile: ' + ' '.join(flags))
r = run('apply', elsewhere, '--yes', '--since', '0', '--save-routes', '--map', 'skx=Missing', '--json')
result = json.loads(r.stdout)
check(r.returncode == 3 and result['reason']['code'] == 'missing_input' and result['details']['routes_saved']['state'] == 'not_saved' and profile.read_bytes() == before and (elsewhere / '0.skx').exists(), 'a missing folder is missing_input and moves nothing')
step = r.stderr.split('Create it first: ', 1)[1].rsplit('.', 1)[0]
subprocess.run(shlex.split(step), check=True)
r = run('apply', elsewhere, '--yes', '--since', '0', '--save-routes', '--map', 'skx=Missing')
check(r.returncode == 2 and 'already routes to Sketches' in r.stderr, 'after the printed mkdir, the save reaches the route check')
(root / 'Blender Files/one.bpy').write_bytes(b'occupied')
r = run('apply', root, '--since', '0', '--map', 'bpy=Blender Files', '--save-routes', '--yes')
check(r.returncode != 0 and 'routes were not saved' in r.stderr and profile.read_bytes() == before, 'failed apply explicitly saves nothing')
check(not any(p.name.endswith('.pending') for p in profile.parent.iterdir()), 'failed apply cleans its private temporary file')
PY
# The shared reason table, contract and envelope code changed with this
# feature, so drive $STASH and $UNPACK through them too: each contract still
# matches its pinned file, shares retry and subject rules with sweep for every
# common code, does not claim sweep's new profile code, and a refusal from
# each still has the envelope shape its contract promises.
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HOME="$W/home" python3 -u - "$SWEEP" "$STASH" "$UNPACK" "$REPO" >> "$W/checks" <<'PY' || CODE=$?
import json, pathlib, subprocess, sys
sweep, repo = sys.argv[1], pathlib.Path(sys.argv[4])
tools = {'stash': sys.argv[2], 'unpack': sys.argv[3]}
def run(binary, *args):
    r = subprocess.run([binary, *args], capture_output=True, text=True)
    assert 'panicked' not in r.stdout + r.stderr
    return r
def check(ok, label):
    assert ok, label
    print(label)
def canonical(value):
    value = dict(value)
    value['operation_id'] = '<per-invocation>'
    value['tool_version'] = '<manifest-version>'
    if isinstance(value.get('details'), dict) and value['details'].get('kind') == 'capability_contract':
        value['details'] = canonical(value['details'])
    return value
def rules(contract):
    return {row['code']: (row['retry'], row['subject_kind']) for row in contract['details']['refusal_codes']}
sweep_rules = rules(json.loads(run(sweep, 'contract', '--json').stdout))
check(sweep_rules.get('profile_write_failed') == ('after_change', 'profile'), 'sweep lists the new profile code with its retry and subject')
for name, binary in tools.items():
    r = run(binary, 'contract', '--json')
    contract = json.loads(r.stdout)
    pin = repo / 'docs/contracts' / f"v{contract['details']['schema_version']}" / f'{name}.json'
    check(r.returncode == 0 and canonical(contract) == json.loads(pin.read_text()), f'{name} contract --json still matches {pin.relative_to(repo)}')
    own = rules(contract)
    check('profile_write_failed' not in own and 'profile_invalid' not in own, f'{name} does not claim sweep profile codes')
    shared = sorted(set(own) & set(sweep_rules))
    check(shared and all(own[code] == sweep_rules[code] for code in shared), f'{name} and sweep agree on retry and subject for {len(shared)} shared codes')
    r = run(binary, '--bogus', '--json')
    out = r.stdout.strip().splitlines()
    envelope = json.loads(out[0]) if len(out) == 1 else {}
    check(r.returncode == 2 and envelope.get('status') == 'refused' and envelope.get('schema_version') == 4 and envelope.get('reason') == {'code': 'unknown_option', 'retry': own.get('unknown_option', ('?',))[0]} and envelope.get('details') == {} and envelope['scope']['tool'] == name, f'{name} refuses an unknown option with one envelope its contract describes')
PY
while IFS= read -r label; do
  case "$label" in UNPROVEN\ *) unproven "save failure after moves" "${label#UNPROVEN }";; *) pass "$label";; esac
done < "$W/checks"
[ "$CODE" -eq 0 ] || fail "saved routes contract failed (Python exit $CODE)"

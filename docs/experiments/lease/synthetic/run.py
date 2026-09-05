#!/usr/bin/env python3
"""Drive incumbent experiments, retain raw receipts; no acceptance assertions."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
import urllib.error

p = argparse.ArgumentParser()
p.add_argument('tool', choices=['process-compose', 'pueue'])
a = p.parse_args()
base = Path(__file__).resolve().parent
out = base.parent / 'runs' / a.tool
out.mkdir(parents=True, exist_ok=False)
records = []

def record(kind, **fields):
    row = dict(n=len(records)+1, time=datetime.datetime.now(datetime.timezone.utc).isoformat(), kind=kind, **fields)
    records.append(row)
    with (out/'transcript.jsonl').open('a') as f:
        f.write(json.dumps(row)+'\n')
    print(json.dumps(row), flush=True)
    return row

def decision(chose, expected, because):
    subprocess.run(['emit', '--chose', chose, '--because', because, '--expected', expected], check=True)

env = {'PATH': '/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin', 'TMPDIR': '/tmp', 'LANG': 'en_US.UTF-8'}

def command(argv, cwd=None, timeout=20):
    cwd = str(cwd or root)
    record('attempt', argv=argv, shell=shlex.join(argv), cwd=cwd)
    try:
        r = subprocess.run(argv, cwd=cwd, env=env, text=True, capture_output=True, timeout=timeout)
        record('command', argv=argv, shell=shlex.join(argv), cwd=cwd, returncode=r.returncode, stdout=r.stdout, stderr=r.stderr)
        return r
    except subprocess.TimeoutExpired as e:
        record('timeout', argv=argv, seconds=timeout)
        raise

# The generator is actually executed for each incumbent, never passed a real project.
r = subprocess.run([sys.executable, str(base/'create.py')], env=env, text=True, capture_output=True, check=True)
manifest = json.loads(r.stdout)
root = Path(manifest['root'])
record('generator', argv=[sys.executable,str(base/'create.py')], env=env, returncode=r.returncode, stdout=r.stdout, stderr=r.stderr)
(out/'manifest.json').write_text(r.stdout)
env['XDG_CONFIG_HOME'] = str(root/'config-home')
(root/'config-home').mkdir()
record('environment', env=env)
shutil.copy2(root/'worker.py', out/'worker.py')
for exe in [a.tool] + (['pueued'] if a.tool=='pueue' else []):
    path=Path(shutil.which(exe)).resolve()
    record('executable', name=exe, path=str(path), sha256=hashlib.sha256(path.read_bytes()).hexdigest())
    command([exe, 'version'] if exe=='process-compose' else [exe,'--version'])
for args in ([['--help'], ['up','--help'], ['process','stop','--help'], ['process','get','--help']] if a.tool=='process-compose' else [['--help'],['add','--help'],['kill','--help'],['enqueue','--help'],['shutdown','--help']]):
    command([a.tool]+args)

handles=[]
def spawn(argv, name, cwd=None):
    log=root/(name+'.log')
    f=log.open('w')
    h=subprocess.Popen(argv,cwd=str(cwd or root),env=env,stdout=f,stderr=subprocess.STDOUT,start_new_session=True)
    f.close(); handles.append((h,log,name))
    record('spawn',argv=argv,shell=shlex.join(argv),cwd=str(cwd or root),pid=h.pid,log=str(log))
    return h

def direct_signal(h, sig):
    # These handles are direct unreaped children of this driver, never fixture PIDs.
    if h.poll() is None:
        record('signal-attempt',pid=h.pid,signal=sig,authority='direct live Popen child')
        h.send_signal(sig)
        h.wait(timeout=15)
        record('reaped',pid=h.pid,returncode=h.returncode)

identities={}
def identify(project,label=None):
    path=root/project/'identity.json'
    deadline=time.monotonic()+12
    while not path.exists() and time.monotonic()<deadline:
        time.sleep(.1)
    if not path.exists():
        record('identity-missing',project=project); raise RuntimeError('worker did not launch: '+project)
    ident=json.loads(path.read_text()); identities[label or project]=ident
    record('identity-hint',project=project,label=label or project,value=ident)
    observe(label or project)
    return ident

def observe(label):
    ident=identities[label]
    url='http://127.0.0.1:'+str(ident['port'])+'/'
    try:
        with urllib.request.urlopen(url,timeout=1) as r: status,body=r.status,r.read().decode()
    except urllib.error.HTTPError as e: status,body=e.code,e.read().decode()
    except Exception as e: status,body=None,str(e)
    record('http',label=label,url=url,status=status,body=body)
    command(['/bin/ps','-p',str(ident['pid']),'-o','pid=,ppid=,pgid=,stat=,lstart=,comm='])

def gate(project):
    path=root/project/'allow-ready'
    command([sys.executable,'-c','from pathlib import Path; Path('+repr(str(path))+').touch()'])

def worker(project,mode='ordinary'):
    return [sys.executable,str(root/'worker.py'),'--mode',mode]

unrelated=spawn(worker('unrelated'),'unrelated',root/'unrelated')
identify('unrelated'); gate('unrelated')

if a.tool=='process-compose':
    cli=['process-compose','-U','-u',str(root/'pc.sock'),'-L',str(root/'pc.log')]
    configs={}
    for name,mode in [('task-a','tree'),('task-b','ordinary'),('escaped','escaped'),('resistant','ignore-term'),('expiry','ordinary')]:
        configs[name]={'command':'exec '+shlex.join(worker(name,mode)), 'working_dir':str(root/name), 'disabled':name not in ('task-a','task-b'), 'shutdown':{'signal':15,'timeout_seconds':2}}
    probe=root/'probe.py'
    probe.write_text('import json,urllib.request\nfrom pathlib import Path\np=json.loads(Path("identity.json").read_text())["port"]\nassert urllib.request.urlopen("http://127.0.0.1:%s/"%p,timeout=1).status==200\n')
    configs['task-b']['readiness_probe']={'exec':{'command':shlex.join([sys.executable,str(probe)])},'period_seconds':1,'timeout_seconds':2,'failure_threshold':100,'success_threshold':1}
    config=root/'pc.yaml'
    config.write_text(json.dumps({'version':'0.5','processes':configs},indent=2))
    shutil.copy2(config,out/'initial-config.json'); shutil.copy2(probe,out/'probe.py')
    daemon_args=cli+['-f',str(config),'--disable-dotenv','--keep-project','-t=false','up']
    daemon=spawn(daemon_args,'supervisor')
    def state(name='task-b'): return command(cli+['process','get',name,'-o','json'])
    def stop(name): return command(cli+['process','stop',name])
    def start(name): return command(cli+['process','start',name])
else:
    (root/'runtime').mkdir()
    config=root/'pueue.yml'
    config.write_text(json.dumps({'shared':{'pueue_directory':str(root/'state'),'runtime_directory':str(root/'runtime'),'unix_socket_path':str(root/'pq.sock'),'alias_file':str(root/'aliases.yml')}}))
    shutil.copy2(config,out/'initial-config.json')
    cli=['pueue','-c',str(config),'--color','never']
    daemon_args=['pueued','-c',str(config)]
    daemon=spawn(daemon_args,'supervisor')
    time.sleep(1)
    command(cli+['parallel','4'])
    ids={}
    def add(name,mode='ordinary'):
        r=command(cli+['add','-p','-w',str(root/name),'exec '+shlex.join(worker(name,mode))])
        ids[name]=r.stdout.strip(); return ids[name]
    def state(name=None): return command(cli+['status','--json'])
    def stop(name): return command(cli+['kill',ids[name]])
    def start(name): return add(name,{'escaped':'escaped','resistant':'ignore-term'}.get(name,'ordinary'))
    add('task-a','tree'); add('task-b')

identify('task-a'); identify('task-a/child'); identify('task-b')
time.sleep(2)
record('phase',name='readiness-closed'); state(); observe('task-b')
decision('open the configured readiness gate and compare status with HTTP', 'Process Compose health changes only after HTTP 200; Pueue status may have no readiness dimension', 'started and ready are distinct acceptance claims')
gate('task-b'); time.sleep(2); record('phase',name='readiness-open'); state(); observe('task-b')
decision('stop task-a while independently probing task-b and unrelated', 'ordinary parent and child stop while both other listeners keep answering', 'a command acknowledgment alone does not establish cancellation')
record('phase',name='concurrency-and-tree-stop'); stop('task-a'); time.sleep(1)
for name in ['task-a','task-a/child','task-b','unrelated']: observe(name)
state('task-a')

for name in ['escaped','resistant']:
    decision('run and cancel '+name+' fixture', 'record any surviving endpoint alongside the incumbent terminal state', 'escaped descendants and ignored SIGTERM must not be hidden by a clean task status')
    record('phase',name=name+'-cancellation')
    start(name); identify(name)
    if name=='escaped': identify(name+'/child')
    stop(name); time.sleep(2); state(name); observe(name)
    if name=='escaped': observe(name+'/child')

# Explicit expiry is a configured CLI timer recipe, not the fixture's 120s limit.
decision('configure a timer task that invokes the incumbent cancellation command after four seconds', 'the expiry listener stops before its 120-second fixture backstop while task-b and unrelated survive', 'neither inspected task schema offers a native lease deadline; a timer recipe is measurable modest configuration')
record('phase',name='explicit-expiry')
start('expiry'); identify('expiry')
if a.tool=='process-compose':
    # Launch timer through a separate isolated Process Compose project; main config is not hot-edited.
    timer=root/'timer.json'
    timer.write_text(json.dumps({'version':'0.5','processes':{'expiry-timer':{'command':'sleep 4; '+shlex.join(cli+['process','stop','expiry']),'working_dir':str(root)}}}))
    shutil.copy2(timer,out/'timer-config.json')
    timer_handle=spawn(['process-compose','-f',str(timer),'--disable-dotenv','--no-server','-L',str(root/'timer.log'),'-t=false','up'],'expiry-timer')
else:
    command(cli+['add','--delay','4','-w',str(root),' '.join(shlex.quote(v) for v in cli+['kill',ids['expiry']])])
time.sleep(7)
state('expiry')
for name in ['expiry','task-b','unrelated']: observe(name)

if a.tool=='process-compose':
    decision('replay a saved process-name stop after restarting that name', 'determine whether the stale reference can stop a new process incarnation', 'the public stop contract takes a name and no launch generation')
    record('phase',name='stale-reference')
    old=identities['task-a']['pid']; (root/'task-a'/'identity.json').unlink()
    start('task-a'); identify('task-a','replacement-incarnation')
    record('saved-reference',reference='task-a',old_pid=old,new_pid=identities['replacement-incarnation']['pid'])
    stop('task-a'); time.sleep(1); observe('replacement-incarnation'); observe('unrelated')

decision('interrupt the directly launched supervisor with SIGKILL and restart using the same isolated state', 'compare recovered status with the still-running task-b endpoint', 'graceful shutdown would not exercise lost in-memory ownership')
record('phase',name='interruption')
direct_signal(daemon,signal.SIGKILL); observe('task-b')
if a.tool=='process-compose':
    # Prevent re-launch: recovery has definitions but no live handles from the killed supervisor.
    for v in configs.values(): v['disabled']=True
    config.write_text(json.dumps({'version':'0.5','processes':configs},indent=2))
    shutil.copy2(config,out/'recovery-config.json')
daemon=spawn(daemon_args,'recovered-supervisor'); time.sleep(2)
state(); stop('task-b'); time.sleep(1); observe('task-b'); observe('unrelated')

record('phase',name='missing-ownership')
if a.tool=='pueue':
    decision('remove only the documented synthetic state.json while the daemon is stopped', 'a request for the old task id is refused and the orphan remains observable', 'missing ownership must not cause guessed PID cleanup')
    direct_signal(daemon,signal.SIGKILL)
    path=root/'state'/'state.json'
    shutil.copy2(path,out/'state-before-loss.json')
    command([sys.executable,'-c','from pathlib import Path; Path('+repr(str(path))+').rename('+repr(str(path)+'.saved')+')'])
    daemon=spawn(daemon_args,'empty-supervisor'); time.sleep(2)
    state(); stop('task-b'); observe('task-b')
    decision('enqueue a replacement after documented state loss and replay the old task-zero reference', 'observe whether task-id reuse lets the stale reference signal a replacement', 'this is task-id reuse, not simulated OS PID reuse')
    record('phase',name='stale-reference')
    add('replacement'); identify('replacement')
    command(cli+['kill',ids['task-a']]); time.sleep(1); observe('replacement'); observe('unrelated')
else:
    # The preceding restarted supervisor has already lost ownership of task-b.
    stop('task-b'); command(cli+['process','stop','unknown-synthetic-task']); observe('task-b')

decision('shut down only the isolated supervisor and let escaped or orphaned workers reach their original bounded lifetime', 'final endpoint and process observations reveal whether residual workers have exited', 'fixture PID files do not authorize cleanup signals')
record('phase',name='cleanup')
command(cli+(['down'] if a.tool=='process-compose' else ['shutdown']))
try: daemon.wait(timeout=10)
except subprocess.TimeoutExpired: record('supervisor-residual',pid=daemon.pid)
direct_signal(unrelated,signal.SIGTERM)
# Wait in short intervals, preserving visibility; this is containment, not expiry evidence.
deadline=time.monotonic()+125
while time.monotonic()<deadline:
    live=False
    for ident in identities.values():
        try:
            urllib.request.urlopen('http://127.0.0.1:%s/'%ident['port'],timeout=.2).close(); live=True
        except urllib.error.HTTPError: live=True
        except Exception: pass
    if not live: break
    record('containment-wait',remaining_seconds=round(deadline-time.monotonic(),1))
    time.sleep(10)
record('phase',name='final-observations')
for name in identities: observe(name)
for h,log,name in handles:
    record('spawn-result',name=name,pid=h.pid,returncode=h.poll(),stdout_stderr=log.read_text())
    shutil.copy2(log,out/(name+'.log'))
record('complete',root=str(root),note='Observed receipts only; independent witness decides acceptance. No actual OS PID reuse was forced.')

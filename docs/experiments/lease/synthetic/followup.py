#!/usr/bin/env python3
"""Focused incumbent configuration observations. No lane verdict or assertions."""
import datetime
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time
import urllib.request
import urllib.error

name = sys.argv[1]
if name not in ('process-compose', 'pueue'):
    raise SystemExit('choose process-compose or pueue')
base = Path(__file__).resolve().parent
out = base.parent/'runs'/('followup-'+name)
out.mkdir(exist_ok=False)
seq = 0

def rec(kind, **data):
    global seq
    seq += 1
    row = dict(n=seq, time=datetime.datetime.now(datetime.timezone.utc).isoformat(), kind=kind, **data)
    with (out/'transcript.jsonl').open('a') as f:
        f.write(json.dumps(row)+'\n')
    print(json.dumps(row), flush=True)

def decide(chose, expected):
    argv = ['emit','--chose',chose,'--expected',expected]
    rec('decision-attempt',argv=argv)
    r = subprocess.run(argv, capture_output=True, text=True)
    rec('decision-result',returncode=r.returncode,stdout=r.stdout,stderr=r.stderr)
    if r.returncode: raise RuntimeError('emit refused')

env = {'PATH':'/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin','TMPDIR':'/tmp','LANG':'en_US.UTF-8'}
root = base

def cmd(argv):
    rec('attempt', argv=argv, shell=shlex.join(argv), cwd=str(root))
    r = subprocess.run(argv, cwd=root, env=env, capture_output=True,text=True,timeout=20)
    rec('command',argv=argv,shell=shlex.join(argv),cwd=str(root),returncode=r.returncode,stdout=r.stdout,stderr=r.stderr)
    return r

r = cmd([sys.executable,str(base/'create.py')])
manifest = json.loads(r.stdout); root = Path(manifest['root'])
(out/'manifest.json').write_text(r.stdout)
shutil.copy2(root/'worker.py',out/'worker.py')
(root/'config-home').mkdir(); env['XDG_CONFIG_HOME']=str(root/'config-home')
rec('environment',env=env)
cmd([name,'version'] if name=='process-compose' else [name,'--version'])
identities = {}

def observe(project):
    value = identities[project]
    cmd(['/bin/ps','-ww','-p',str(value['pid']),'-o','pid=,ppid=,pgid=,stat=,lstart=,args='])
    url='http://127.0.0.1:%s/'%value['port']
    try:
        with urllib.request.urlopen(url,timeout=1) as r: status,body=r.status,r.read().decode()
    except urllib.error.HTTPError as e: status,body=e.code,e.read().decode()
    except Exception as e: status,body=None,str(e)
    rec('http',project=project,url=url,status=status,body=body)

def identify(project):
    p=root/project/'identity.json'; end=time.monotonic()+12
    while not p.exists() and time.monotonic()<end: time.sleep(.1)
    identities[project]=json.loads(p.read_text())
    rec('identity-hint',project=project,value=identities[project]); observe(project)

def marker_snapshot(label):
    rec('markers',label=label,files={p.name:p.read_text() for p in root.glob('*.receipt')})

def worker(project,mode='ordinary'):
    return 'exec '+shlex.join([sys.executable,str(root/'worker.py'),'--mode',mode])

if name=='process-compose':
    probe=root/'probe.py'
    probe.write_text('import json,urllib.request\nfrom pathlib import Path\np=json.loads(Path("identity.json").read_text())["port"]\nassert urllib.request.urlopen("http://127.0.0.1:%s/"%p,timeout=1).status==200\n')
    processes = {'source': {'command':worker('escaped','escaped'),'working_dir':str(root/'escaped'),'shutdown':{'signal':15,'timeout_seconds':2},'readiness_probe':{'exec':{'command':shlex.join([sys.executable,str(probe)])},'period_seconds':1,'failure_threshold':100}}}
    for condition in ('process_started','process_healthy','process_completed','process_completed_successfully'):
        marker=root/(condition+'.receipt')
        code='from pathlib import Path; import datetime; Path('+repr(str(marker))+').write_text(datetime.datetime.now(datetime.timezone.utc).isoformat())'
        processes[condition]={'command':shlex.join([sys.executable,'-c',code]),'working_dir':str(root),'depends_on':{'source':{'condition':condition}}}
    config=root/'pc.json'; config.write_text(json.dumps({'version':'0.5','processes':processes},indent=2))
    shutil.copy2(probe,out/'probe.py')
    cli=['process-compose','-U','-u',str(root/'pc.sock'),'-L',str(root/'pc.log')]
    args=cli+['-f',str(config),'--disable-dotenv','--keep-project','-t=false','up']
else:
    (root/'runtime').mkdir()
    callback=root/'callback.py'
    callback.write_text('import json,sys,datetime\nfrom pathlib import Path\np=Path('+repr(str(root/'callback.receipt'))+')\nwith p.open("a") as f: f.write(json.dumps({"time":datetime.datetime.now(datetime.timezone.utc).isoformat(),"id":sys.argv[1],"result":sys.argv[2]})+"\\n")\n')
    config=root/'pueue.json'
    config.write_text(json.dumps({'shared':{'pueue_directory':str(root/'state'),'runtime_directory':str(root/'runtime'),'unix_socket_path':str(root/'pq.sock'),'alias_file':str(root/'aliases.yml')},'daemon':{'callback':shlex.join([sys.executable,str(callback)])+' "{{ id }}" "{{ result }}"'}},indent=2))
    shutil.copy2(callback,out/'callback.py')
    cli=['pueue','-c',str(config),'--color','never']; args=['pueued','-c',str(config)]
shutil.copy2(config,out/'config.json')
rec('spawn-attempt',argv=args,shell=shlex.join(args))
with (root/'supervisor.log').open('w') as log:
    daemon=subprocess.Popen(args,cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
rec('spawn',pid=daemon.pid)
try:
    time.sleep(1)
    if name=='pueue':
        task=cmd(cli+['add','-p','-w',str(root/'escaped'),worker('escaped','escaped')]).stdout.strip()
    identify('escaped'); identify('escaped/child'); time.sleep(2)
    rec('phase',name='gate-closed')
    cmd(cli+(['process','list','-o','json'] if name=='process-compose' else ['status','--json']))
    marker_snapshot('gate-closed'); observe('escaped')
    decide('open the synthetic HTTP readiness gate','healthy dependency runs only after readiness; completion callback remains absent while Pueue task runs')
    cmd([sys.executable,'-c','from pathlib import Path; Path('+repr(str(root/'escaped'/'allow-ready'))+').touch()'])
    time.sleep(3)
    cmd(cli+(['process','list','-o','json'] if name=='process-compose' else ['status','--json']))
    marker_snapshot('gate-open'); observe('escaped')
    decide('cancel the escaped parent through the isolated incumbent CLI','completion surfaces may fire while the child still answers HTTP with matching full argv')
    cmd(cli+(['process','stop','source'] if name=='process-compose' else ['kill',task]))
    time.sleep(3)
    cmd(cli+(['process','list','-o','json'] if name=='process-compose' else ['status','--json']))
    marker_snapshot('after-cancel')
    for project in identities: observe(project)
finally:
    decide('shut down the isolated supervisor then observe the child through its unchanged 120-second backstop','full argv plus endpoint observations record residual life and eventual absence without authorizing signals from PID hints')
    cmd(cli+(['down'] if name=='process-compose' else ['shutdown']))
    try: daemon.wait(timeout=15)
    except subprocess.TimeoutExpired:
        rec('supervisor-residual',pid=daemon.pid)
    # Time-bounded observation, never signals from identity files.
    end=time.monotonic()+125
    while time.monotonic()<end and 'escaped/child' in identities:
        observe('escaped/child')
        r=cmd(['/bin/ps','-ww','-p',str(identities['escaped/child']['pid']),'-o','pid=,lstart=,args='])
        if r.returncode==1 and not r.stdout: break
        time.sleep(10)
    rec('phase',name='residual-cleanup-observation')
    for project in identities: observe(project)
    rec('spawn-result',pid=daemon.pid,returncode=daemon.poll(),stdout_stderr=(root/'supervisor.log').read_text())
    shutil.copy2(root/'supervisor.log',out/'supervisor.log')
    for p in root.glob('*.receipt'): shutil.copy2(p,out/p.name)
    rec('complete',note='observations only; no acceptance verdict; fixture expiry is not incumbent cleanup')

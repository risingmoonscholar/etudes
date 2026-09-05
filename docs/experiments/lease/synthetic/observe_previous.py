#!/usr/bin/env python3
"""Read-only current observations of historical synthetic identities; never signal."""
import datetime
import json
from pathlib import Path
import shlex
import subprocess
import urllib.request
import urllib.error
base=Path(__file__).resolve().parent.parent
out=base/'runs'/'review-followup'/'historical-residuals.jsonl'
with out.open('x') as f:
 def record(kind,**data):
  f.write(json.dumps(dict(time=datetime.datetime.now(datetime.timezone.utc).isoformat(),kind=kind,**data))+'\n'); f.flush()
 for tool in ('process-compose','pueue','pueue-setup-failure'):
  rows=[json.loads(s) for s in (base/'runs'/tool/'transcript.jsonl').read_text().splitlines()]
  identities={}
  for row in rows:
   if row['kind']=='identity-hint': identities[row['value']['pid']]=row['value']
  # Include identities generated but not reached by the original driver.
  root=Path(json.loads((base/'runs'/tool/'manifest.json').read_text())['root'])
  for path in root.glob('**/identity.json'):
   value=json.loads(path.read_text()); identities[value['pid']]=value
  for value in identities.values():
   argv=['/bin/ps','-ww','-p',str(value['pid']),'-o','pid=,ppid=,pgid=,stat=,lstart=,args=']
   record('attempt',tool=tool,argv=argv,shell=shlex.join(argv),historical_hint=value)
   r=subprocess.run(argv,capture_output=True,text=True)
   record('command',tool=tool,argv=argv,returncode=r.returncode,stdout=r.stdout,stderr=r.stderr)
   url='http://127.0.0.1:%s/'%value['port']
   try:
    with urllib.request.urlopen(url,timeout=1) as response: status,body=response.status,response.read().decode()
   except urllib.error.HTTPError as e: status,body=e.code,e.read().decode()
   except Exception as e: status,body=None,str(e)
   record('http',tool=tool,pid_hint=value['pid'],url=url,status=status,body=body)

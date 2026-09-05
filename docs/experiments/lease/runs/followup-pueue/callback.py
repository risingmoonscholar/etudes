import json,sys,datetime
from pathlib import Path
p=Path('/tmp/lease-baseline-ruvl5kyf/callback.receipt')
with p.open("a") as f: f.write(json.dumps({"time":datetime.datetime.now(datetime.timezone.utc).isoformat(),"id":sys.argv[1],"result":sys.argv[2]})+"\n")

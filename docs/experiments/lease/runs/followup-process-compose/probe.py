import json,urllib.request
from pathlib import Path
p=json.loads(Path("identity.json").read_text())["port"]
assert urllib.request.urlopen("http://127.0.0.1:%s/"%p,timeout=1).status==200

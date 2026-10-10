#!/usr/bin/env bash
# User-selected objects from different parents retain one reversible encrypted operation.
# Synthetic duplicate basenames, an opaque folder and overlap refusal bind filesystem observations to each call.
# The original bytes, unchanged sibling, count-only receipt and restored parents are independent of reported move counters.
# A broadened parent scan or overwriting duplicate basenames would fail these comparisons; source links are refused.
# The case uses three selected objects and one small supplied test key, with no real credentials or payloads.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require python3 "exact selection needs Python fixture generation" || exit 0
W=$(workdir)
trap 'rm -rf "$W"' EXIT
if python3 - "$UNPACK" "$STASH" "$W" <<'PYTHON'
import pathlib,subprocess,json,sys,os
binary=pathlib.Path(sys.argv[2]);root=pathlib.Path(sys.argv[3]);holding=root/'holding';holding.mkdir()
a=root/'alpha'/'PRIVATE_record.pdf';b=root/'beta'/'PRIVATE_record.pdf';tree=root/'tree'
for parent in (a.parent,b.parent,tree):parent.mkdir()
a.write_bytes(b'synthetic alpha');b.write_bytes(b'synthetic beta');(tree/'payload').write_bytes(b'synthetic tree')
sibling=a.parent/'unselected_PRIVATE';sibling.write_bytes(b'synthetic untouched')
env=dict(os.environ,ETUDE_STATE_DIR=str(root/'state'),ETUDE_JOURNAL_KEY='56'*32)
def run(*args):return subprocess.run([str(binary),*map(str,args)],env=env,capture_output=True,text=True)
for selected in [(tree,tree/'payload'),(a,a)]:
 result=run('select',*selected,'--into',holding,'--json');assert result.returncode==2
 assert not list(holding.iterdir()) and a.read_bytes()==b'synthetic alpha'
paths=root/'paths0';paths.write_bytes(b''.join(str(p).encode()+b'\0' for p in (a,b,tree)))
result=run('select','--from0',paths,'--into',holding,'--json');assert result.returncode==0,result.stderr
receipt=json.loads(result.stdout);assert receipt['status']=='done';assert 'PRIVATE' not in result.stdout+result.stderr
assert sibling.read_bytes()==b'synthetic untouched' and not a.exists() and not b.exists() and not tree.exists()
operation=pathlib.Path(receipt['details']['holding_root']);result=run('pop',operation,'--json');assert result.returncode==0,result.stderr
assert a.read_bytes()==b'synthetic alpha' and b.read_bytes()==b'synthetic beta' and (tree/'payload').read_bytes()==b'synthetic tree'
assert not operation.exists()
PYTHON
then pass "exact selection retains parents, isolates duplicate names and restores the opaque directory"; else fail "exact selection filesystem or receipt comparison failed"; fi

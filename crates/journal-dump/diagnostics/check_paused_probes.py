"""Pin the required witness and control channels for the remaining probes."""
from pathlib import Path
import ast
root = Path(__file__).resolve().parents[3]
for number in range(2, 6):
    path = next((root / 'stress/scenarios').glob(f'40-0{number}*.sh'))
    text = path.read_text()
    for token in ('--watch-apply', 'STOPPED=', '--control-late', '--control-ignored', 'run_controls'):
        assert token in text, (path.name, token)
    assert 'DELAY_MS' not in text, path.name
source = (root / 'scripts/check-tool-contracts.py').read_text()
for call in ast.walk(ast.parse(source)):
    if isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute) and isinstance(call.func.value, ast.Name) and call.func.value.id == 'subprocess' and call.func.attr == 'run':
        assert any(keyword.arg == 'stdin' and ast.unparse(keyword.value) == 'subprocess.DEVNULL' for keyword in call.keywords), call.lineno
# Generated negative-control wrappers are also tools started by this witness.
for node in ast.walk(ast.parse(source)):
    if isinstance(node, ast.Constant) and isinstance(node.value, str) and 'result = subprocess.run([real,' in node.value:
        assert 'stdin=subprocess.DEVNULL' in node.value
print('ok: paused pending probes, controls and closed subprocess stdin')

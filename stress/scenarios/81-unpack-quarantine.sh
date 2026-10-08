#!/usr/bin/env bash
# Downloaded archives keep their exact captured quarantine on every published payload.
# Fixtures and xattr values are synthetic; readback and bytes are independent of stdout.
# The unmarked-output control must fail the exact-mark comparison; attribute failures refuse publication.
# All ten suffixes exercise actual commands; the exFAT phase uses one disposable 512 MB image.
# Existing stress costs stay intact; this adds archive provenance rather than metadata-moving policy.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require python3 "quarantine matrix requires Python" || exit 0
if [ "$(uname -s)" != Darwin ]; then unproven "quarantine matrix" "macOS xattr APIs required"; exit 0; fi
SCRIPT="$(dirname "${BASH_SOURCE[0]}")/../../crates/unpack-cli/tests/quarantine_matrix.py"
if python3 "$SCRIPT" raw; then pass "every raw extractor and alias propagation was measured"; else fail "raw quarantine measurement failed"; fi
if python3 "$SCRIPT" unpack "$UNPACK"; then pass "APFS marked/unmarked extraction and failed preservation witnesses pass"; else fail "APFS quarantine witness failed"; fi
require hdiutil "exFAT quarantine needs a disk image" || exit 0
W=$(workdir)
MNT="$W/exfat"
mkdir -p "$MNT"
cleanup() { detach_registered_mounts; rm -rf "$W"; }
trap cleanup EXIT
if ! create_disk_image "$W/exfat.dmg" 512m ExFAT Quarantine; then unproven "exFAT quarantine" "image creation failed"; exit 0; fi
if ! hdiutil attach "$W/exfat.dmg" -nobrowse -mountpoint "$MNT" >/dev/null 2>&1; then unproven "exFAT quarantine" "image attach failed"; exit 0; fi
register_mount "$MNT"
if python3 "$SCRIPT" unpack "$UNPACK" "$MNT"; then pass "exFAT exact quarantine survives publication via AppleDouble representation"; else fail "exFAT quarantine witness failed"; fi
if python3 - "$MNT" <<'PYTHON'
from pathlib import Path
import sys
root=Path(sys.argv[1])
assert any(path.name.startswith('._') for path in root.rglob('*')), 'no AppleDouble companion was observed'
assert not any('.unpack-' in path.name and '.partial' in path.name for path in root.iterdir()), 'orphan staging companion'
PYTHON
then pass "exFAT companions observed without orphan staging"; else fail "exFAT companion witness failed"; fi

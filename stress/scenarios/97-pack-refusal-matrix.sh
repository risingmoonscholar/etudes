#!/usr/bin/env bash
# One synthetic tree per row of docs/experiments/pack/fixtures-v1.json. Each
# row asserts the exit status, the machine-readable reason (code, retry class,
# subject kind), that the source tree is byte-for-byte and identity-for-identity
# unchanged, that no private staging file is left behind, and that a refusal
# creates no destination. The in-flight row checks that the entry is named on
# stderr and no other source name is. The sensitive row checks that the count
# is reported and that no sensitive name appears anywhere in the output.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
export ETUDE_STATE_DIR="$W/state"; mkdir -p "$ETUDE_STATE_DIR"
export COPYFILE_DISABLE=1

FIXTURES="$(dirname "${BASH_SOURCE[0]}")/../../crates/fixtures/src/lib.rs"
SENSITIVE=()
while IFS= read -r name; do SENSITIVE+=("$name"); done < <(python3 - "$FIXTURES" <<'PY'
import sys
text = open(sys.argv[1]).read()
block = text.split("pub const SENSITIVE_NAMES: &[&str] = &[", 1)[1].split("];", 1)[0]
for line in block.splitlines():
    line = line.strip()
    if line.startswith('"') and line.endswith('",'):
        print(line[1:-2])
PY
)
[ "${#SENSITIVE[@]}" -ge 10 ] || { fail "could not read the shared sensitive fixture names"; exit 0; }

# identity_tree DIR OUT: device, inode, size, mode and nanosecond times of every
# entry, without following links. A byte snapshot alone would miss a rewritten
# file with the same bytes.
identity_tree() {
  python3 - "$1" > "$2" <<'PY'
import os, sys
root = sys.argv[1]
rows = []
for base, dirs, files in os.walk(root, followlinks=False):
    for name in dirs + files:
        p = os.path.join(base, name)
        s = os.lstat(p)
        rows.append((os.path.relpath(p, root), s.st_dev, s.st_ino, s.st_size, s.st_mode, s.st_mtime_ns, s.st_ctime_ns))
for row in sorted(rows):
    print("\t".join(map(str, row)))
PY
}

# field FILE JSONPATH: one value from the envelope, 'null' for a missing step.
field() {
  python3 - "$1" "$2" <<'PY'
import json, sys
value = json.load(open(sys.argv[1]))
for step in sys.argv[2].split("."):
    if isinstance(value, dict) and step in value:
        value = value[step]
    else:
        value = None
        break
print("null" if value is None else (value if isinstance(value, str) else json.dumps(value)))
PY
}

# run_row ID WANT_EXIT WANT_CODE WANT_RETRY WANT_KIND SOURCE DEST ARGS...
# Runs pack once with --json, keeps stdout and stderr, and makes the assertions
# every row shares. Row-specific checks follow in the caller.
run_row() {
  local id="$1" want_exit="$2" want_code="$3" want_retry="$4" want_kind="$5" src="$6" dest="$7"
  shift 7
  OUT="$W/$id.out"; ERR="$W/$id.err"
  identity_tree "$src" "$W/$id.id-before"
  snapshot_tree "$src" "$W/$id.snap-before"
  "$PACK" "$src" --into "$dest" --json "$@" >"$OUT" 2>"$ERR" </dev/null
  local got=$?
  assert_eq "$want_exit" "$got" "$id: exit status"
  if ! python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$OUT" 2>/dev/null; then
    fail "$id: stdout is not one JSON envelope"
    return
  fi
  assert_eq "$want_code" "$(field "$OUT" reason.code)" "$id: reason.code"
  assert_eq "$want_retry" "$(field "$OUT" reason.retry)" "$id: reason.retry"
  assert_eq "$want_kind" "$(field "$OUT" reason.subject.kind)" "$id: reason.subject.kind"
  snapshot_tree "$src" "$W/$id.snap-after"
  identity_tree "$src" "$W/$id.id-after"
  assert_snapshot_eq "$W/$id.snap-before" "$W/$id.snap-after" "$id: source bytes and names untouched"
  if cmp -s "$W/$id.id-before" "$W/$id.id-after"; then
    pass "$id: source inodes, sizes, modes and times unchanged"
  else
    fail "$id: source identity changed"
  fi
  if [ -z "$(find "$(dirname "$dest")" -maxdepth 1 -name '.*.pack-*.partial' -print -quit)" ]; then
    pass "$id: no private staging file left beside the destination"
  else
    fail "$id: a .pack-*.partial staging file was left behind"
  fi
  if [ "$want_exit" -ne 0 ]; then
    [ ! -e "$dest" ] && pass "$id: refusal created no destination" || fail "$id: refusal created a destination"
  else
    [ -f "$dest" ] && pass "$id: archive published" || fail "$id: done without a destination"
  fi
}

# 1. sensitive-parity: every shared sensitive name is omitted and counted;
#    none of them is printed or archived.
R=sensitive-parity; S="$W/$R/source"; mkdir -p "$S"
for name in "${SENSITIVE[@]}"; do printf 'synthetic private text\n' > "$S/$name"; done
printf 'ordinary payload\n' > "$S/ordinary.txt"
run_row "$R" 0 done not_needed null "$S" "$W/$R/out.zip" --for other
assert_eq "${#SENSITIVE[@]}" "$(field "$OUT" details.excluded)" "$R: exclusion count equals the sensitive fixture count"
assert_eq '[{"id": "i-000001"}]' "$(field "$OUT" details.items)" "$R: exactly one opaque item id, the ordinary file"
leaked=0
for name in "${SENSITIVE[@]}"; do
  if grep -qF -- "$name" "$OUT" "$ERR" 2>/dev/null; then leaked=1; fi
  if [ -f "$W/$R/out.zip" ] && /usr/bin/tar -tf "$W/$R/out.zip" 2>/dev/null | grep -qF -- "$name"; then leaked=1; fi
done
[ "$leaked" -eq 0 ] && pass "$R: no sensitive name on stdout, stderr or in the archive listing" \
  || fail "$R: a sensitive name was disclosed"
[ -f "$W/$R/out.zip" ] && /usr/bin/tar -tf "$W/$R/out.zip" 2>/dev/null | grep -qx 'ordinary.txt' \
  && pass "$R: the ordinary file is archived" || fail "$R: the ordinary file is missing from the archive"

# 2. in-flight-download: refused, the entry is named, nothing else is.
R=in-flight-download; S="$W/$R/source"; mkdir -p "$S"
printf 'ordinary payload\n' > "$S/ordinary-report.txt"
printf 'partial bytes\n' > "$S/movie.part"
run_row "$R" 2 download_in_flight later download_in_flight "$S" "$W/$R/out.zip" --for other
grep -qF 'movie.part' "$ERR" && pass "$R: the in-flight entry is named on stderr" \
  || fail "$R: stderr does not name the in-flight entry"
if grep -qF 'ordinary-report' "$OUT" "$ERR"; then
  fail "$R: another source path was disclosed"
else
  pass "$R: no other source path appears on stdout or stderr"
fi

# 3. recent-file: a file created this second is eligible; no grace window.
R=recent-file; S="$W/$R/source"; mkdir -p "$S"
printf 'fresh payload\n' > "$S/just-created.txt"
run_row "$R" 0 done not_needed null "$S" "$W/$R/out.tar.gz" --for self
assert_eq 0 "$(field "$OUT" details.excluded)" "$R: nothing excluded"
/usr/bin/tar -tzf "$W/$R/out.tar.gz" 2>/dev/null | grep -qx 'just-created.txt' \
  && pass "$R: the just-created file is in the archive" || fail "$R: the recent file was not archived"

# 4. opaque-package: a package is archived whole, or the run is refused.
R=opaque-package-whole; S="$W/$R/source"; mkdir -p "$S/Example.app/Contents/MacOS"
printf 'plist\n' > "$S/Example.app/Contents/Info.plist"
printf 'binary\n' > "$S/Example.app/Contents/MacOS/Example"
run_row "$R" 0 done not_needed null "$S" "$W/$R/out.zip" --for other
members=$(/usr/bin/tar -tf "$W/$R/out.zip" 2>/dev/null | sed 's#/$##' | LC_ALL=C sort | tr '\n' ' ')
assert_eq "Example.app Example.app/Contents Example.app/Contents/Info.plist Example.app/Contents/MacOS Example.app/Contents/MacOS/Example " "$members" "$R: the package tree is archived complete"
R=opaque-package-split; S="$W/$R/source"; mkdir -p "$S/Example.app/Contents"
printf 'plist\n' > "$S/Example.app/Contents/Info.plist"
printf 'synthetic private text\n' > "$S/Example.app/Contents/passport_scan.pdf"
run_row "$R" 2 unsafe_input after_change input "$S" "$W/$R/out.zip" --for other
grep -qF 'passport' "$OUT" "$ERR" && fail "$R: the excluded name inside the package was disclosed" \
  || pass "$R: the refusal names nothing inside the package"

# 5. archive-path-hazards: detected before any archive is created.
R=hazard-escaping-symlink; S="$W/$R/source"; mkdir -p "$S"
printf 'ordinary payload\n' > "$S/ordinary.txt"
ln -s ../../outside "$S/link"
run_row "$R" 2 unsafe_input after_change input "$S" "$W/$R/out.zip" --for other
R=hazard-reserved-name; S="$W/$R/source"; mkdir -p "$S"
printf 'ordinary payload\n' > "$S/CON.txt"
run_row "$R" 2 unsafe_input after_change input "$S" "$W/$R/out.zip" --for other
R=hazard-traversal-member; S="$W/$R/source"; mkdir -p "$S"
printf 'ordinary payload\n' > "$S/trailing."
run_row "$R" 2 unsafe_input after_change input "$S" "$W/$R/out.zip" --for other

# 6. metadata-size: the bound is checked before publication; zip with keep is
#    a contradiction and is refused as invalid arguments.
R=metadata-size-bound; S="$W/$R/source"; mkdir -p "$S"
dd if=/dev/zero of="$S/payload.bin" bs=1048576 count=2 status=none
run_row "$R" 2 insufficient_space after_change destination "$S" "$W/$R/out.zip" --for other --max-size 1M
R=metadata-zip-keep; S="$W/$R/source"; mkdir -p "$S"
printf 'ordinary payload\n' > "$S/ordinary.txt"
run_row "$R" 2 invalid_arguments never null "$S" "$W/$R/out.zip" --for other --metadata keep

# 7. recipient-source-preservation: the recipient class is required, and a
#    completed pack leaves the source identical.
R=recipient-missing; S="$W/$R/source"; mkdir -p "$S"
printf 'ordinary payload\n' > "$S/ordinary.txt"
run_row "$R" 2 invalid_arguments never null "$S" "$W/$R/out.zip"
R=recipient-source-preserved; S="$W/$R/source"; mkdir -p "$S/notes"
printf 'ordinary payload\n' > "$S/ordinary.txt"
printf 'nested payload\n' > "$S/notes/inner.md"
printf 'litter\n' > "$S/.DS_Store"
run_row "$R" 0 done not_needed null "$S" "$W/$R/out.tar.gz" --for mac
assert_eq mac "$(field "$OUT" details.recipient)" "$R: recipient class reported"
assert_eq keep "$(field "$OUT" details.metadata)" "$R: metadata policy reported"

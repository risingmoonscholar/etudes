#!/usr/bin/env bash
# Recomputed plan digests do not grant permission to move held files.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null
W=$(workdir); trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
D="$W/root"; mkdir -p "$D"
for name in report01.pdf report02.pdf report03.pdf passport.pdf; do
  printf 'synthetic %s\n' "$name" > "$D/$name"
done
snapshot_tree "$D" "$W/before"
"$SWEEP" "$D" --since 0 --export-plan "$W/original.plan" --json >"$W/export.json" 2>"$W/export.err"
assert_eq 0 "$?" "ordinary sweep exports a plan"
[ -f "$W/original.plan" ] || exit 1
DIGEST=$(python3 - "$W/original.plan" "$W/forged.plan" <<'PY'
import hashlib, pathlib, sys
body = pathlib.Path(sys.argv[1]).read_bytes()[:-64]
assert body.count(b'report01.pdf') == 1
body = body.replace(b'report01.pdf', b'passport.pdf')
digest = hashlib.sha256(body).hexdigest()
pathlib.Path(sys.argv[2]).write_bytes(body + digest.encode())
print(digest)
PY
)
"$SWEEP" apply --plan "$W/forged.plan" --plan-digest "$DIGEST" --yes --json >"$W/refusal.json" 2>"$W/refusal.err"
assert_eq 2 "$?" "forged personal member refuses with a recomputed digest"
python3 - "$W/refusal.json" <<'PY'
import json, sys
assert json.load(open(sys.argv[1]))['reason']['code'] == 'plan_stale'
PY
assert_eq 0 "$?" "forged member uses the existing stale-plan reason"
snapshot_tree "$D" "$W/after-refusal"
assert_snapshot_eq "$W/before" "$W/after-refusal" "forged apply changes no path or bytes"
DIGEST=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["details"]["binding"]["plan_digest"])' "$W/export.json")
"$SWEEP" apply --plan "$W/original.plan" --plan-digest "$DIGEST" --yes --json >"$W/applied.json" 2>"$W/applied.err"
assert_eq 0 "$?" "legitimate sweep plan still applies"
"$SWEEP" undo "$D" >"$W/undo.out" 2>&1
assert_eq 0 "$?" "sweep undo restores the legitimate moves"
snapshot_tree "$D" "$W/after-undo"
assert_snapshot_eq "$W/before" "$W/after-undo" "legitimate sweep undo restores every byte"
S="$W/stash"; mkdir -p "$S"
printf 'synthetic stash payload\n' > "$S/note.txt"
"$STASH" "$S" >"$W/stash.out" 2>&1
assert_eq 0 "$?" "stash still holds an ordinary file"
"$STASH" pop "$S" >"$W/pop.out" 2>&1
assert_eq 0 "$?" "stash pop restores the file"
assert_eq 'synthetic stash payload' "$(cat "$S/note.txt")" "stash preserves exact bytes"
mkdir "$W/archive"
printf 'synthetic archive payload\n' > "$W/archive/member.txt"
tar -czf "$W/fixture.tar.gz" -C "$W/archive" member.txt
"$UNPACK" "$W/fixture.tar.gz" --into "$W/out" >"$W/unpack.out" 2>&1
assert_eq 0 "$?" "unpack still extracts an ordinary archive"
assert_eq 'synthetic archive payload' "$(cat "$W/out/member.txt")" "unpack preserves exact bytes"
true

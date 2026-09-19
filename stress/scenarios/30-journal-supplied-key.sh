#!/usr/bin/env bash
# Supplied keys retain encrypted undo even when login keychain access fails.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
W=$(workdir)
trap 'rm -rf "$W" "$ETUDE_STATE_DIR"' EXIT
KEY_A=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
KEY_B=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
export ETUDE_JOURNAL_KEY="$KEY_A"

make_tree() {
  mkdir -p "$1"
  for i in 01 02 03 04 05; do
    printf 'private original content' > "$1/Screenshot 2026-01-$i at 10.00.00 AM.png"
  done
}
restored() {
  for i in 01 02 03 04 05; do
    assert_eq 'private original content' "$(cat "$1/Screenshot 2026-01-$i at 10.00.00 AM.png" 2>/dev/null)" "$2 ($i)"
  done
}
encrypted() {
  if python3 - "$ETUDE_STATE_DIR" "$1" <<'PY'
import pathlib, struct, sys
files = list(pathlib.Path(sys.argv[1]).glob(sys.argv[2] + '-*.journal'))
assert files, 'no journal written'
for path in files:
    data = path.read_bytes()
    assert b'Screenshot' not in data and b'private original content' not in data
    while data:
        assert len(data) >= 4
        n, = struct.unpack('<I', data[:4])
        frame, data = data[4:4+n], data[4+n:]
        assert len(frame) == n and frame.startswith(b'SWEEPJ1\0')
        assert n >= 8 + 24 + 4096 + 16
PY
  then pass "$1 journals contain only encrypted frames, no plaintext filenames"
  else fail "$1 journal encryption"; fi
}

# Establish the unavailable-keychain control on this host. The Rust source
# test additionally proves that supplied keys never invoke the keychain.
make_tree "$W/control"
out=$(env -u ETUDE_JOURNAL_KEY "$SWEEP" apply "$W/control" --yes 2>&1); code=$?
if [ "$code" = 2 ] && [[ "$out" == *keychain* ]]; then
  pass 'login keychain unavailable: unsupplied apply refuses'
  [[ "$out" == *'only alternative is --no-journal'* && "$out" == *'removes undo'* ]] && pass 'sweep refusal names loss of undo' || fail 'sweep refusal advice'
  out=$(env -u ETUDE_JOURNAL_KEY "$STASH" "$W/control" 2>&1); code=$?
  assert_eq 2 "$code" 'unsupplied stash refuses'
  [[ "$out" == *'only alternative is --no-journal'* && "$out" == *'removes undo'* ]] && pass 'stash refusal names loss of undo' || fail 'stash refusal advice'
else
  unproven 'host keychain unavailable control' 'this host did not refuse keychain access; deterministic source test covers bypass'
fi
rm -f "$ETUDE_STATE_DIR"/*.journal

make_tree "$W/sweep"
assert_exit 0 'supplied-key sweep apply' -- "$SWEEP" apply "$W/sweep" --yes
assert_eq 0 "$(find "$W/sweep" -maxdepth 1 -type f | wc -l | tr -d ' ')" 'sweep actually moved files'
encrypted sweep
assert_exit 0 'supplied-key sweep undo' -- "$SWEEP" undo "$W/sweep"
restored "$W/sweep" 'sweep restores original bytes'
make_tree "$W/stash"
assert_exit 0 'supplied-key stash' -- "$STASH" "$W/stash"
encrypted stash
assert_exit 0 'supplied-key stash pop' -- "$STASH" pop "$W/stash"
restored "$W/stash" 'stash restores original bytes'

# Two keys in the SAME state directory: neither named nor global undo may
# skip the newer unreadable journal to restore an older decryptable apply.
for layout in same-root different-roots; do
  rm -f "$ETUDE_STATE_DIR"/*.journal
  first="$W/$layout/first"; second="$first"
  [ "$layout" = different-roots ] && second="$W/$layout/second"
  make_tree "$first"
  assert_exit 0 "$layout older apply with A" -- "$SWEEP" apply "$first" --yes
  sleep 1
  mkdir -p "$second"
  for i in 01 02 03 04 05; do
    printf 'new private content' > "$second/Screenshot 2026-02-$i at 11.00.00 AM.png"
  done
  assert_exit 0 "$layout newer apply with B" -- env ETUDE_JOURNAL_KEY="$KEY_B" "$SWEEP" apply "$second" --yes
  before=$(find "$W/$layout" -type f -exec shasum {} \; | sort)
  journals=$(shasum "$ETUDE_STATE_DIR"/*.journal)
  assert_exit 3 "$layout global A cannot skip B" -- "$SWEEP" undo
  assert_exit 3 "$layout named A cannot skip B" -- "$SWEEP" undo "$first"
  assert_eq "$before" "$(find "$W/$layout" -type f -exec shasum {} \; | sort)" "$layout refused undo moves nothing"
  assert_eq "$journals" "$(shasum "$ETUDE_STATE_DIR"/*.journal)" "$layout refused undo leaves journal bytes intact"
  assert_exit 0 "$layout B restores newest operation" -- env ETUDE_JOURNAL_KEY="$KEY_B" "$SWEEP" undo "$second"
  assert_eq 'new private content' "$(cat "$second/Screenshot 2026-02-02 at 11.00.00 AM.png")" "$layout newest bytes restored"
  # Even a completed unreadable journal is a barrier. Isolate key histories
  # explicitly rather than silently guessing what that journal contains.
  assert_exit 3 "$layout completed B remains an A selection barrier" -- "$SWEEP" undo "$first"
done

rm -f "$ETUDE_STATE_DIR"/*.journal
make_tree "$W/stash-mixed"
assert_exit 0 'stash mixed older A' -- "$STASH" "$W/stash-mixed"
assert_exit 0 'stash mixed restore A before next stash' -- "$STASH" pop "$W/stash-mixed"
sleep 1
assert_exit 0 'stash mixed newer B' -- env ETUDE_JOURNAL_KEY="$KEY_B" "$STASH" "$W/stash-mixed"
before=$(find "$W/stash-mixed" -type f -exec shasum {} \; | sort)
journals=$(shasum "$ETUDE_STATE_DIR"/*.journal)
assert_exit 3 'stash A refuses newer B instead of selecting older A' -- "$STASH" pop "$W/stash-mixed"
assert_eq "$before" "$(find "$W/stash-mixed" -type f -exec shasum {} \; | sort)" 'stash refusal leaves all files intact'
assert_eq "$journals" "$(shasum "$ETUDE_STATE_DIR"/*.journal)" 'stash refusal leaves journal bytes intact'
newest=$(python3 - "$ETUDE_STATE_DIR" <<'PYNEW'
import pathlib, sys
print(max(pathlib.Path(sys.argv[1]).glob('stash-*.journal'), key=lambda p: p.stat().st_mtime_ns))
PYNEW
)
chmod 000 "$newest"
for selection in named current-directory; do
  if [ "$selection" = named ]; then
    out=$(env ETUDE_JOURNAL_KEY="$KEY_B" "$STASH" pop "$W/stash-mixed" 2>&1); code=$?
  else
    out=$(cd "$W/stash-mixed" && env ETUDE_JOURNAL_KEY="$KEY_B" "$STASH" pop 2>&1); code=$?
  fi
  assert_eq 3 "$code" "$selection unreadable B blocks completed A"
  [[ "$out" == *unreadable* && "$out" != *'already popped'* ]] && pass "$selection reports unreadable journal" || fail "$selection hides unreadable journal"
done
chmod 600 "$newest"
assert_eq "$before" "$(find "$W/stash-mixed" -type f -exec shasum {} \; | sort)" 'unreadable newer stash moves nothing'
assert_eq "$journals" "$(shasum "$ETUDE_STATE_DIR"/*.journal)" 'unreadable newer stash preserves journal bytes'
assert_exit 0 'stash B restores newest operation' -- env ETUDE_JOURNAL_KEY="$KEY_B" "$STASH" pop "$W/stash-mixed"
restored "$W/stash-mixed" 'stash B original bytes restored'

# Both roots still have live operations; the wrong-key barrier must prevent
# selecting the older matching stash, not merely report an already-popped one.
rm -f "$ETUDE_STATE_DIR"/*.journal
make_tree "$W/stash-live-a"; make_tree "$W/stash-live-b"
assert_exit 0 'stash live older A' -- "$STASH" "$W/stash-live-a"
sleep 1
assert_exit 0 'stash live newer B' -- env ETUDE_JOURNAL_KEY="$KEY_B" "$STASH" "$W/stash-live-b"
before=$(find "$W/stash-live-a" "$W/stash-live-b" -type f -exec shasum {} \; | sort)
journals=$(shasum "$ETUDE_STATE_DIR"/*.journal)
assert_exit 3 'stash A cannot skip live B from another root' -- "$STASH" pop "$W/stash-live-a"
assert_eq "$before" "$(find "$W/stash-live-a" "$W/stash-live-b" -type f -exec shasum {} \; | sort)" 'mixed live stash refusal moves nothing'
assert_eq "$journals" "$(shasum "$ETUDE_STATE_DIR"/*.journal)" 'mixed live stash refusal preserves journal bytes'
assert_exit 0 'stash B restores live newest root' -- env ETUDE_JOURNAL_KEY="$KEY_B" "$STASH" pop "$W/stash-live-b"
restored "$W/stash-live-b" 'newest live stash original bytes restored'

# Discovery and filesystem failures must be barriers too, including ties:
# filenames are opaque IDs, so a filename tie-break cannot prove chronology.
for tool in sweep stash; do
  for fault in unreadable-journal broken-link equal-times unreadable-state state-not-directory; do
    rm -f "$ETUDE_STATE_DIR"/*.journal
    first="$W/$tool-$fault/first"; second="$W/$tool-$fault/second"
    make_tree "$first"; make_tree "$second"
    if [ "$tool" = sweep ]; then
      assert_exit 0 "$fault older sweep apply" -- "$SWEEP" apply "$first" --yes
      sleep 1
      assert_exit 0 "$fault newer sweep apply" -- "$SWEEP" apply "$second" --yes
    else
      assert_exit 0 "$fault older stash" -- "$STASH" "$first"
      sleep 1
      assert_exit 0 "$fault newer stash" -- "$STASH" "$second"
    fi
    newest=$(python3 - "$ETUDE_STATE_DIR" <<'NEWEST'
import pathlib, sys
print(max(pathlib.Path(sys.argv[1]).glob('*.journal'), key=lambda p: p.stat().st_mtime_ns))
NEWEST
)
    before=$(find "$W/$tool-$fault" -type f -exec shasum {} \; | sort)
    journals=$(shasum "$ETUDE_STATE_DIR"/*.journal)
    case "$fault" in
      unreadable-journal) chmod 000 "$newest" ;;
      broken-link) mv "$newest" "$W/saved-journal"; ln -s "$W/absent-journal" "$newest" ;;
      equal-times) python3 - "$ETUDE_STATE_DIR" <<'TIES'
import os, pathlib, sys
for p in pathlib.Path(sys.argv[1]).glob('*.journal'):
    os.utime(p, ns=(1800000000000000000, 1800000000000000000))
TIES
        ;;
      unreadable-state) chmod 000 "$ETUDE_STATE_DIR" ;;
      state-not-directory) mv "$ETUDE_STATE_DIR" "$ETUDE_STATE_DIR.saved"; printf blocked > "$ETUDE_STATE_DIR" ;;
    esac
    if [[ "$fault" = unreadable-* ]]; then
      if python3 - "$newest" "$ETUDE_STATE_DIR" "$fault" <<'DENIED'
import pathlib, sys
try:
    if sys.argv[3] == 'unreadable-journal':
        pathlib.Path(sys.argv[1]).read_bytes()
    else:
        list(pathlib.Path(sys.argv[2]).iterdir())
except PermissionError:
    sys.exit(0)
sys.exit(1)
DENIED
      then pass "$tool $fault permission denial established"
      else fail "$tool $fault fixture did not deny reads"; fi
    fi
    if [ "$tool" = sweep ]; then
      assert_exit 3 "$fault sweep global refuses" -- "$SWEEP" undo
      assert_exit 3 "$fault sweep named refuses" -- "$SWEEP" undo "$first"
    else
      out=$("$STASH" pop "$first" 2>&1); code=$?
      assert_eq 3 "$code" "$fault stash named refuses"
      [[ "$out" != *'already popped'* && ( "$out" == *journal* || "$out" == *malformed* ) ]] && pass "$fault stash reports journal failure" || fail "$fault stash hid unreadable state"
      assert_exit 3 "$fault stash current-directory refuses" -- bash -c 'cd "$1" && "$2" pop' _ "$first" "$STASH"
    fi
    case "$fault" in
      unreadable-journal) chmod 600 "$newest" ;;
      broken-link) rm "$newest"; mv "$W/saved-journal" "$newest" ;;
      unreadable-state) chmod 700 "$ETUDE_STATE_DIR" ;;
      state-not-directory) rm "$ETUDE_STATE_DIR"; mv "$ETUDE_STATE_DIR.saved" "$ETUDE_STATE_DIR" ;;
    esac
    assert_eq "$before" "$(find "$W/$tool-$fault" -type f -exec shasum {} \; | sort)" "$tool $fault refusal leaves files intact"
    assert_eq "$journals" "$(shasum "$ETUDE_STATE_DIR"/*.journal)" "$tool $fault refusal leaves journal bytes intact"
    if [ "$fault" = equal-times ]; then
      python3 - "$ETUDE_STATE_DIR" "$newest" <<'ORDER'
import os, pathlib, sys, time
now = time.time_ns()
for p in pathlib.Path(sys.argv[1]).glob('*.journal'):
    stamp = now if str(p) == sys.argv[2] else now - 1000000000
    os.utime(p, ns=(stamp, stamp))
ORDER
    fi
    if [ "$tool" = sweep ]; then
      assert_exit 0 "$fault repaired sweep restores older root" -- "$SWEEP" undo "$first"
    else
      assert_exit 0 "$fault repaired stash restores older root" -- "$STASH" pop "$first"
    fi
    restored "$first" "$tool $fault repaired control"

  done
done

rm -f "$ETUDE_STATE_DIR"/*.journal
make_tree "$W/invalid"
for bad in '' short "$(printf 'z%.0s' {1..64})"; do
  assert_exit 2 'malformed supplied key refuses sweep' -- env ETUDE_JOURNAL_KEY="$bad" "$SWEEP" apply "$W/invalid" --yes
  assert_exit 2 'malformed supplied key refuses stash' -- env ETUDE_JOURNAL_KEY="$bad" "$STASH" "$W/invalid"
done
restored "$W/invalid" 'invalid keys move nothing'
assert_eq 0 "$(find "$ETUDE_STATE_DIR" -name '*.journal' | wc -l | tr -d ' ')" 'invalid keys write no journals'
assert_exit 0 'explicit stash no-journal bypasses invalid key' -- env ETUDE_JOURNAL_KEY=bad "$STASH" "$W/invalid" --no-journal
assert_eq 0 "$(find "$ETUDE_STATE_DIR" -name '*.journal' | wc -l | tr -d ' ')" 'no-journal writes no journal'
# The shared core also serves unpack. Exercise extraction with the same
# supplied-key environment and prove it leaves the encrypted history alone.
make_tree "$W/unpack-history"
assert_exit 0 'unpack control records an encrypted sweep' -- "$SWEEP" apply "$W/unpack-history" --yes
journals=$(shasum "$ETUDE_STATE_DIR"/*.journal)
mkdir -p "$W/unpack-input" "$W/unpack-source"
printf 'archive payload' > "$W/unpack-source/payload.txt"
tar -cf "$W/unpack-input/example.tar" -C "$W/unpack-source" payload.txt
assert_exit 0 'unpack apply with supplied journal key' -- "$UNPACK" "$W/unpack-input/example.tar"
assert_eq 'archive payload' "$(cat "$W/unpack-input/example/payload.txt" 2>/dev/null)" 'unpack extracted original bytes'
assert_eq "$journals" "$(shasum "$ETUDE_STATE_DIR"/*.journal)" 'unpack leaves encrypted journal bytes unchanged'
assert_exit 0 'sweep undo after unpack restores' -- "$SWEEP" undo "$W/unpack-history"
restored "$W/unpack-history" 'sweep restores after unpack'

[ "$FAILED" -eq 0 ]

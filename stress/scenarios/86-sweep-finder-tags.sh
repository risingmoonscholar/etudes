#!/usr/bin/env bash
# Finder tags: an agent drives sweep's tag promise end to end on folders it made.
#
# A Finder tag is the owner having already filed something. By default sweep
# leaves a tagged item alone and tells the caller how many it left, never
# which. With --include-tagged, loudly, it organises the item and the tag
# survives the move. A tag added after the plan was made is not moved either.
# Finder comments are not looked at. Everything is synthetic, in a temp dir.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
TAG='opaque-tag-bytes-Work'
TAGATTR=com.apple.metadata:_kMDItemUserTags
COMMENTATTR=com.apple.metadata:kMDItemFinderComment

fresh() {  # fresh DIR: six reports plus one tagged invoice
  mkdir -p "$1"
  for i in 1 2 3 4 5 6; do printf 'synthetic report %s\n' "$i" > "$1/report-$i.pdf"; done
  printf 'synthetic invoice text\n' > "$1/filed.pdf"
  xattr -w "$TAGATTR" "$TAG" "$1/filed.pdf"
}
find_moved() { find "$1" -mindepth 2 -name "$2" -type f | head -1; }

# --- the plan reports a count, never a name ----------------------------------
F="$W/folder"; fresh "$F"
PLAN=$("$SWEEP" "$F" --json 2>&1) || true
grep -q '"tagged_skipped":1' <<<"$PLAN" \
  && pass "the plan counts one tagged item held back" \
  || fail "the plan did not report one held tagged item: $(head -c 300 <<<"$PLAN")"
if grep -q 'filed.pdf' <<<"$PLAN" || grep -q "$TAG" <<<"$PLAN"; then
  fail "the plan named the tagged file or its tag; a count was promised"
else
  pass "and names neither the file nor the tag"
fi
HUMAN=$("$SWEEP" "$F" 2>&1) || true
grep -q 'no contents read to decide' <<<"$HUMAN" \
  && pass "the banner says no contents read to decide" \
  || fail "the banner is not the restated promise: $(head -3 <<<"$HUMAN")"
grep -q 'names, sizes and dates only' <<<"$HUMAN" \
  && fail "the banner still carries the old promise" || pass "and not the old one"
grep -q '1 Finder-tagged item was left alone' <<<"$HUMAN" \
  && pass "the human output counts the held item" \
  || fail "the human output did not count the held item: $HUMAN"

# --- apply leaves the tagged item where it is --------------------------------
OUT=$("$SWEEP" apply "$F" --yes 2>&1); CODE=$?
assert_eq 0 "$CODE" "apply exits 0 when some items were held"
[ -f "$F/filed.pdf" ] && pass "apply left the tagged file in place" || fail "apply moved a tagged file without being told to"
[ ! -f "$F/report-1.pdf" ] && pass "and organised the untagged ones" || fail "the untagged files were not organised"
xattr -p "$TAGATTR" "$F/filed.pdf" 2>/dev/null | grep -q "$TAG" \
  && pass "the held file kept its tag" || fail "the held file lost its tag"

# --- --include-tagged moves it, loudly, and the tag survives ------------------
G="$W/again"; fresh "$G"
OUT=$("$SWEEP" apply "$G" --yes --include-tagged 2>&1); CODE=$?
assert_eq 0 "$CODE" "apply with --include-tagged exits 0"
grep -q 'WARNING: --include-tagged is on' <<<"$OUT" && pass "--include-tagged is loud" || fail "--include-tagged was silent: $OUT"
JSONW=$("$SWEEP" "$G" --include-tagged --json 2>&1 >/dev/null) || true
grep -q 'WARNING: --include-tagged is on' <<<"$JSONW" \
  && pass "and still loud under --json (on stderr)" || fail "--json hid the warning"
[ ! -f "$G/filed.pdf" ] && pass "the tagged file was organised with its group" || fail "--include-tagged left the tagged file: $OUT"
MOVED=$(find_moved "$G" filed.pdf)
[ -n "$MOVED" ] && pass "it exists inside a group folder" || fail "the tagged file vanished or never moved"
if [ -n "$MOVED" ]; then
  xattr -p "$TAGATTR" "$MOVED" 2>/dev/null | grep -q "$TAG" \
    && pass "its Finder tag survived the move" || fail "the Finder tag was lost on move"
fi
"$SWEEP" undo "$G" >/dev/null 2>&1 || true
[ -f "$G/filed.pdf" ] && pass "undo restored the tagged file" || fail "undo did not restore the tagged file"
xattr -p "$TAGATTR" "$G/filed.pdf" 2>/dev/null | grep -q "$TAG" \
  && pass "with its tag" || fail "undo lost the tag"

# --- Finder comments are not looked at ---------------------------------------
C="$W/comments"; mkdir -p "$C"
for i in 1 2 3 4 5 6; do printf 'synthetic report %s\n' "$i" > "$C/report-$i.pdf"; done
xattr -w "$COMMENTATTR" 'a private comment' "$C/report-1.pdf"
CJ=$("$SWEEP" "$C" --json 2>&1) || true
grep -q '"tagged_skipped":0' <<<"$CJ" \
  && pass "a Finder comment does not hold an item" || fail "a comment changed the plan: $(head -c 300 <<<"$CJ")"
grep -q 'a private comment' <<<"$CJ" && fail "the comment text reached the output" || pass "and its text is never in the output"
"$SWEEP" apply "$C" --yes >/dev/null 2>&1 || true
CM=$(find_moved "$C" report-1.pdf)
if [ -n "$CM" ] && [ "$(xattr -p "$COMMENTATTR" "$CM" 2>/dev/null)" = 'a private comment' ]; then
  pass "the commented file moved and kept its comment untouched"
else
  fail "the commented file did not move with its comment intact"
fi

# --- a tag added after the plan was made is not moved ------------------------
S="$W/late"; mkdir -p "$S"
for i in 1 2 3 4 5 6; do printf 'synthetic report %s\n' "$i" > "$S/report-$i.pdf"; done
EXPORT=$("$SWEEP" "$S" --export-plan "$W/late.plan" --json 2>&1) || true
DIGEST=$(grep -o '"plan_digest":"[0-9a-f]*"' <<<"$EXPORT" | head -1 | cut -d'"' -f4)
if [ -z "$DIGEST" ]; then
  fail "could not export a plan: $(head -c 300 <<<"$EXPORT")"
else
  xattr -w "$TAGATTR" "$TAG" "$S/report-3.pdf"
  OUT=$("$SWEEP" apply --plan "$W/late.plan" --plan-digest "$DIGEST" --yes 2>&1); CODE=$?
  [ "$CODE" != 0 ] && pass "an exported plan whose item was tagged afterwards is refused (exit $CODE)" \
    || fail "apply carried on after a tag appeared: $OUT"
  [ -f "$S/report-3.pdf" ] && [ -f "$S/report-1.pdf" ] \
    && pass "nothing moved, the tagged item included" || fail "files moved from a stale plan"
fi

# --- a tag added after review planned, before it applies ---------------------
# A real PTY satisfies review's terminal gate. At the final approval prompt a
# planned source is tagged, then the same plan is approved. Tagging changes the
# file's change time, so the plan's binding refuses it as stale before any
# move; the move-time check behind that refusal is covered in etude-core's
# finder_tags tests, which tag between the binding and the rename.
python3 - "$SWEEP" "$W" <<'PY'
import os, pathlib, pty, select, subprocess, sys, time
binary, work = sys.argv[1:]
ATTR = 'com.apple.metadata:_kMDItemUserTags'
for include in (False, True):
    folder = pathlib.Path(work) / ('late-include' if include else 'late-hold')
    folder.mkdir()
    for i in range(6):
        (folder / f'report-{i}.pdf').write_text('synthetic report\n')
    tagged = folder / 'report-0.pdf'
    master, slave = pty.openpty()
    args = [binary, 'review', str(folder), '--since', '0']
    if include:
        args.append('--include-tagged')
    process = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave)
    os.close(slave)
    transcript = bytearray()
    accepted = tagged_after_plan = False
    deadline = time.monotonic() + 20
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                transcript.extend(chunk)
            if not accepted and b'[a]ccept' in transcript:
                os.write(master, b'a\n')
                accepted = True
            if not tagged_after_plan and b'proceed? [y/N]' in transcript:
                subprocess.run(['xattr', '-w', ATTR, 'private-late-tag', str(tagged)], check=True)
                tagged_after_plan = True
                os.write(master, b'y\n')
        code = process.wait(timeout=5)
        text = transcript.decode(errors='replace')
        assert tagged_after_plan, 'review never reached the approval point'
        assert code == 2, f'unexpected exit {code}'
        assert 'changed since planning' in text, 'the stale plan was not named'
        assert tagged.exists(), 'the late-tagged file moved'
        assert all((folder / f'report-{i}.pdf').exists() for i in range(6)), 'a file moved from a stale plan'
        assert 'private-late-tag' not in text, 'the tag text reached the output'
        print('    ok       a tag added after review planned stops the move ('
              + ('with' if include else 'without') + ' --include-tagged): exit 2, nothing moved')
    except Exception as error:
        print(f'    FAIL     late Finder tag review: {error}')
        print(transcript.decode(errors='replace'))
        raise
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
PY
if [ $? = 0 ]; then
  pass "review refuses a plan whose item was tagged after planning, with and without --include-tagged"
else
  fail "the late-tag review check failed"
fi

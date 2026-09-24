#!/usr/bin/env bash
# Document markers protect their reference surface without freezing unrelated
# work beside the project. This covers sibling, nested, linked, and unreadable
# asset layouts for both sweep and stash/pop.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
# Bounded document scopes: documents in scenes/ may reference ../textures/.
# Restore permissions before cleanup, even when an assertion fails.
trap 'chmod -R u+rwx "$W"; rm -rf "$W" "$ETUDE_STATE_DIR"' EXIT
for layout in siblings same-child external-link unreadable-folder unreadable-marker; do
  P="$W/hold-$layout/root"
  mkdir -p "$P/scenes" "$P/textures" "$W/hold-$layout/assets"
  printf 'a' > "$P/scenes/a.blend"
  printf 'b' > "$P/scenes/b.blend"
  for n in 1 2 3; do
    printf 'texture' > "$P/textures/tex_$n.png"
    printf 'external texture' > "$W/hold-$layout/assets/tex_$n.png"
    printf 'invoice' > "$P/invoice_$n.pdf"
  done
  DOCS="$P/scenes"
  HOLDER='scenes/a.blend'
  case "$layout" in
    same-child)
      mv "$P/scenes/a.blend" "$P/a.blend"
      mv "$P/scenes/b.blend" "$P/b.blend"
      rmdir "$P/scenes"
      DOCS="$P"; HOLDER='a.blend' ;;
    external-link)
      rm -r "$P/textures"
      ln -s ../assets "$P/textures" ;;
    unreadable-folder) chmod 000 "$P/scenes" ;;
    unreadable-marker) chmod 000 "$P/scenes/a.blend" ;;
  esac
  CODE=0; PREVIEW=$("$SWEEP" "$P" --depth 4 --since 0 2>&1) || CODE=$?
  assert_eq 0 "$CODE" "$layout: sweep preview succeeds"
  if [ "$layout" = siblings ] || [ "$layout" = same-child ]; then
    grep -q "because $HOLDER may reference them" <<<"$PREVIEW" \
      && pass "$layout: the deterministic holder is $HOLDER" \
      || fail "$layout: wrong document holds the textures: $PREVIEW"
  fi
  CODE=0; APPLIED=$("$SWEEP" apply "$P" --depth 4 --since 0 --yes --no-journal 2>&1) || CODE=$?
  assert_eq 0 "$CODE" "$layout: sweep apply succeeds"
  [ -f "$P/textures/tex_1.png" ] && [ ! -f "$P/invoice_1.pdf" ] \
    && pass "$layout: sweep held textures and moved invoices" \
    || fail "$layout: sweep moved textures or failed to move invoices: $APPLIED"
  if [ "$layout" = external-link ]; then
    [ -L "$P/textures" ] && [ "$(readlink "$P/textures")" = ../assets ] && [ -f "$P/textures/tex_1.png" ] \
      && pass "sweep left the external textures link untouched and resolving" \
      || fail "sweep moved or broke the external textures link"
  fi
  if [[ "$layout" = unreadable-* ]]; then
    COUNTS=$("$SWEEP" "$P" --depth 4 --since 0 --json)
    grep -qE 'WARNING: [1-9][0-9]* .*could not be read' <<<"$APPLIED" \
      && grep -qE '"unreadable"[[:space:]]*:[[:space:]]*[1-9]' <<<"$COUNTS" \
      && [ -f "$P/textures/tex_1.png" ] \
      && pass "$layout: sweep counts the held assets unreadable" \
      || fail "sweep moved unreadable-marker siblings or did not count them unreadable: $APPLIED"
  fi
  # Fresh movable files force a real journalled stash and pop, even after sweep.
  for n in 1 2 3; do printf 'invoice' > "$P/fresh_$n.pdf"; done
  CODE=0; STASHED=$("$STASH" "$P" --json 2>&1) || CODE=$?
  assert_eq 0 "$CODE" "$layout: real stash succeeds"
  chmod u+rx "$P/scenes" 2>/dev/null || true
  [ -f "$DOCS/a.blend" ] && [ -f "$DOCS/b.blend" ] && [ -f "$P/textures/tex_1.png" ] \
    && pass "$layout: stash retained documents and textures" \
    || fail "stash moved a document or sibling texture: $STASHED"
  [ ! -f "$P/fresh_1.pdf" ] && pass "$layout: stash moved invoices" || fail "$layout: stash moved nothing: $STASHED"
  if [ "$layout" = external-link ]; then
    [ -L "$P/textures" ] && [ "$(readlink "$P/textures")" = ../assets ] && [ -f "$P/textures/tex_1.png" ] \
      && pass "stash left the external textures link untouched and resolving" \
      || fail "stash moved or broke the external textures link"
  fi
  if [[ "$layout" = unreadable-* ]]; then
    grep -qE '"skipped_unreadable"[[:space:]]*:[[:space:]]*[1-9]' <<<"$STASHED" \
      && [ -f "$P/textures/tex_1.png" ] \
      && pass "$layout: stash counts the held assets unreadable" \
      || fail "stash moved unreadable-marker siblings or did not count them unreadable: $STASHED"
  fi
  assert_exit 0 "$layout: stash pop succeeds" -- "$STASH" pop "$P"
  [ -f "$P/fresh_1.pdf" ] && pass "$layout: pop restored invoices" || fail "$layout: pop lost invoices"
  chmod -R u+rwx "$P"
done

# The two documents and textures also share one CHILD of the scanned root.
CP="$W/ChildProject"; mkdir -p "$CP/project/textures"
printf 'a' > "$CP/project/a.blend"
printf 'b' > "$CP/project/b.blend"
printf 'texture' > "$CP/project/textures/wood.png"
printf 'texture' > "$CP/project/textures/stone.png"
printf 'texture' > "$CP/project/textures/metal.png"
for n in 1 2 3; do printf 'invoice' > "$CP/invoice_$n.pdf"; done
INNER=$("$SWEEP" "$CP/project" --depth 4 --since 0 2>&1 || true)
grep -q 'because a.blend may reference them' <<<"$INNER" \
  && pass 'child project: a.blend deterministically holds textures beside both documents' \
  || fail "child project: the wrong document holds textures: $INNER"
assert_exit 0 'parent apply succeeds beside a two-document child project' -- "$SWEEP" apply "$CP" --depth 4 --since 0 --yes --no-journal
[ -f "$CP/project/a.blend" ] && [ -f "$CP/project/b.blend" ] && [ -f "$CP/project/textures/wood.png" ] && [ ! -f "$CP/invoice_1.pdf" ] \
  && pass 'parent apply held the child project and moved invoices' \
  || fail 'parent apply moved a child document or texture, or did not move invoices'
printf 'fresh invoice' > "$CP/fresh.pdf"
assert_exit 0 'parent stash succeeds beside a two-document child project' -- "$STASH" "$CP"
[ -f "$CP/project/a.blend" ] && [ -f "$CP/project/b.blend" ] && [ -f "$CP/project/textures/wood.png" ] && [ ! -f "$CP/fresh.pdf" ] \
  && pass 'parent stash held the child project and moved invoices' \
  || fail 'parent stash moved a child document or texture, or did not move invoices'
assert_exit 0 'parent stash pop restores the invoices' -- "$STASH" pop "$CP"
[ -f "$CP/fresh.pdf" ] && pass 'parent pop restored the fresh invoice' || fail 'parent pop lost the fresh invoice'

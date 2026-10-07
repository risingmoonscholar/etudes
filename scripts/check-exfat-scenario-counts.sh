#!/usr/bin/env bash
# Exercise scenario 60's actual assertions without mounting an image or
# building sweep. These stand-ins prove the count oracle, not exFAT moves.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

W=$(mktemp -d "${TMPDIR:-/tmp}/etudes-exfat-count-check-XXXXXX")
trap 'rm -rf "$W"' EXIT
mkdir -p "$W/bin"

cat > "$W/bin/diskutil" <<'SH'
#!/usr/bin/env bash
# Blank-image creation succeeds; no disk is created.
[ "$1" = image ] && [ "$2" = create ] && [ "$3" = blank ]
SH

cat > "$W/bin/hdiutil" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  attach)
    # The scenario writes the three matching JPGs immediately after attach.
    mkdir -p "$4/inbox"
    for n in 1 2 3; do
      printf 'AppleDouble %s' "$n" > "$4/inbox/._IMG_104$n.jpg"
    done
    ;;
  detach) ;;
  *) exit 1 ;;
esac
SH

cat > "$W/bin/sweep" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[ "$1" = apply ]
mkdir -p "$2/photos"
for n in 1 2 3; do
  mv "$2/inbox/IMG_104$n.jpg" "$2/photos/"
  mv "$2/inbox/._IMG_104$n.jpg" "$2/photos/"
done
if [ "$EXFAT_COUNT_DROP" = 1 ]; then
  # Leave its sidecar behind: it must not conceal the missing real file.
  rm "$2/photos/IMG_1042.jpg"
fi
SH
chmod +x "$W/bin/"*

# Use the real direct-run wrapper so a failed assertion must also exit 1.
# Clear inherited wrapper state; this check owns its two scenario executions.
unset STRESS_WRAP_DEPTH STRESS_WRAPPER_PID STRESS_RESULT_FD STRESS_STATUS_FD
unset ETUDE_STATE_DIR_OVERRIDE STRESS_FAILURE_ARTIFACTS
export PATH="$W/bin:$PATH" BIN="$W/bin" TMPDIR="$W"
export SCENARIO=60-exfat-same-device-rename-fallback
scenario="stress/scenarios/$SCENARIO.sh"

status=0
EXFAT_COUNT_DROP=0 bash "$scenario" > "$W/control.log" 2>&1 || status=$?
cat "$W/control.log"
if [ "$status" -ne 0 ] ||
   [ "$(grep -c '^    ok ' "$W/control.log" || true)" -ne 4 ] ||
   grep -Eq '^    (FAIL|unproven) ' "$W/control.log"; then
  echo "FAIL exFAT count control: three JPGs plus three sidecars must pass" >&2
  exit 1
fi

status=0
EXFAT_COUNT_DROP=1 bash "$scenario" > "$W/drop.log" 2>&1 || status=$?
if [ "$status" -ne 1 ] ||
   ! grep -Fxq "    FAIL     apply lost none of the 3 real files: expected '3', got '2'" "$W/drop.log"; then
  cat "$W/drop.log"
  echo "FAIL exFAT count mutation: a dropped JPG must fail the loss assertion" >&2
  exit 1
fi
echo "ok exFAT count mutation: dropping IMG_1042.jpg failed the loss assertion (exit 1)"

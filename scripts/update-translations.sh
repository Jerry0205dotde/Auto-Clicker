#!/usr/bin/env bash
# Collect the translatable texts of qml/ and src/messages.rs into
# translations/*.ts. With --check, change nothing and fail if a .ts file is
# outdated or still has an unfinished translation.
set -euo pipefail

cd "$(dirname "$0")/.."
check=false
if (($# == 1)) && [[ $1 == --check ]]; then
  check=true
elif (($# != 0)); then
  printf 'usage: %s [--check]\n' "$0" >&2
  exit 2
fi
lupdate="$("${QMAKE:-qmake6}" -query QT_HOST_BINS)/lupdate"
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
# lupdate reads the Rust file with its C++ parser, which finds the literal
# translate("Klickmeister", "...") calls but drops a call that ends with a
# trailing comma, as rustfmt writes wrapped calls.
sed -z -E 's/,([[:space:]]*\))/\1/g' src/messages.rs > "$work/messages.rs"
sources=(qml "$work/messages.rs")

status=0
for ts in translations/*.ts; do
  if ! $check; then
    "$lupdate" -silent -locations none -no-obsolete "${sources[@]}" -ts "$ts"
    continue
  fi
  copy="$work/$(basename -- "$ts")"
  cp -- "$ts" "$copy"
  "$lupdate" -silent -locations none -no-obsolete "${sources[@]}" -ts "$copy"
  if ! cmp -s -- "$ts" "$copy"; then
    printf '%s is outdated. Run scripts/update-translations.sh and translate the new texts:\n' "$ts" >&2
    diff -u -- "$ts" "$copy" >&2 || true
    status=1
  fi
  if grep -q 'type="unfinished"' -- "$copy"; then
    printf '%s has unfinished translations.\n' "$ts" >&2
    status=1
  fi
done
exit "$status"

#!/usr/bin/env bash
# Archive the Git work tree of the current directory as OUTPUT, with every
# path below PREFIX/. Includes local edits and new files, honours unstaged
# deletions and leaves out .git and ignored files.
set -euo pipefail

if (($# != 2)); then
  printf 'usage: %s OUTPUT PREFIX\n' "$0" >&2
  exit 2
fi
output="$1"
prefix="$2"
mtime="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"

files="$(mktemp -t klickmeister-files.XXXXXXXX)"
trap 'rm -f -- "$files"' EXIT
LC_ALL=C comm -z -23 \
  <(git ls-files --cached --others --exclude-standard -z | LC_ALL=C sort -zu) \
  <(git ls-files --deleted -z | LC_ALL=C sort -zu) > "$files"
tar --no-recursion --null \
  --mtime="@$mtime" --owner=0 --group=0 --numeric-owner \
  --transform="s|^|$prefix/|S" \
  -czf "$output" --files-from="$files"

#!/usr/bin/env bash
set -euo pipefail

if (($# > 1)); then
  printf 'usage: %s [vVERSION]\n' "$0" >&2
  exit 2
fi
if ((EUID == 0)); then
  printf 'Run this script as a regular user; makepkg cannot run as root.\n' >&2
  exit 1
fi
for tool in makepkg python3 git tar b2sum sha256sum; do
  if ! command -v "$tool" >/dev/null; then
    printf 'Missing build tool: %s (requires Arch Linux and the build dependencies in CONTRIBUTING.md).\n' "$tool" >&2
    exit 1
  fi
done

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_dir"
# Reuse the maintained package metadata and dependency list.
source ./PKGBUILD
manifest_version="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')"
if [[ "$pkgver" != "$manifest_version" || "${1:-v$pkgver}" != "v$pkgver" ]]; then
  printf 'Cargo.toml, PKGBUILD and the release tag must have the same version.\n' >&2
  exit 1
fi
if [[ "$(uname -m)" != x86_64 ]]; then
  printf 'Release packages currently require an x86_64 build machine.\n' >&2
  exit 1
fi

dist_dir="$repo_dir/dist"
work_dir="$(mktemp -d -t klickmeister-release.XXXXXXXX)"
trap 'rm -rf -- "$work_dir"' EXIT
mkdir -p "$dist_dir" "$work_dir/source" "$work_dir/bin"
source_archive="klickmeister-$pkgver.tar.gz"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"

# Include the current checkout, including local source edits, without .git or
# ignored build outputs. Freeze that exact snapshot before starting makepkg.
bash scripts/source-archive.sh "$dist_dir/$source_archive" "Auto-Clicker-release-$pkgver"
source_sum="$(b2sum "$dist_dir/$source_archive" | cut -d' ' -f1)"

python3 - "$work_dir/source/PKGBUILD" "$source_sum" <<'PY'
import pathlib
import re
import sys

recipe = pathlib.Path('PKGBUILD').read_text()
replacements = {
    '_commit': '"release-$pkgver"',
    'source': '("klickmeister-$pkgver.tar.gz::$url/releases/download/v$pkgver/klickmeister-$pkgver.tar.gz")',
    'b2sums': f"('{sys.argv[2]}')",
    'options': "('!lto' '!debug')",
}
for key, value in replacements.items():
    recipe, count = re.subn(rf'^{key}=.*$', lambda _: f'{key}={value}', recipe, flags=re.MULTILINE)
    if count != 1:
        raise SystemExit(f'Expected one {key} assignment in PKGBUILD, found {count}')
pathlib.Path(sys.argv[1]).write_text(recipe)
PY
cp "$dist_dir/$source_archive" "$work_dir/source/"
(
  cd "$work_dir/source"
  # Dependencies must already be installed. Never change the build host here.
  PKGDEST="$dist_dir" SRCDEST="$PWD" BUILDDIR="$PWD" PKGEXT='.pkg.tar.zst' \
    makepkg --cleanbuild --noconfirm --force
  makepkg --printsrcinfo > .SRCINFO
)

binary_archive="klickmeister-$pkgver-$pkgrel-x86_64.pkg.tar.zst"
binary_sum="$(b2sum "$dist_dir/$binary_archive" | cut -d' ' -f1)"
python3 - "$pkgver" "$pkgrel" "$binary_sum" "$work_dir/bin/PKGBUILD" <<'PY'
import pathlib
import re
import sys

recipe = pathlib.Path('packaging/aur/PKGBUILD-bin.in').read_text()
depends = re.search(r'^depends=.*$', pathlib.Path('PKGBUILD').read_text(), re.MULTILINE)
if depends is None:
    raise SystemExit('Missing depends assignment in PKGBUILD')
for key, value in zip(('PKGVER', 'PKGREL', 'B2SUM', 'DEPENDS'), (*sys.argv[1:4], depends[0])):
    recipe = recipe.replace(f'@{key}@', value)
pathlib.Path(sys.argv[4]).write_text(recipe)
PY
(
  cd "$work_dir/bin"
  makepkg --printsrcinfo > .SRCINFO
  # Test the binary recipe against the real package, with checksum verification.
  cp "$dist_dir/$binary_archive" .
  PKGDEST="$work_dir/bin" SRCDEST="$PWD" BUILDDIR="$PWD" PKGEXT='.pkg.tar.zst' \
    makepkg --cleanbuild --noconfirm --force
)

for variant in source bin; do
  aur_name=klickmeister
  if [[ "$variant" == bin ]]; then
    aur_name=klickmeister-bin
  fi
  mkdir -p "$dist_dir/aur/$aur_name"
  cp "$work_dir/$variant/PKGBUILD" "$work_dir/$variant/.SRCINFO" "$dist_dir/aur/$aur_name/"
  tar -czf "$dist_dir/$aur_name-$pkgver-$pkgrel-aur.tar.gz" -C "$dist_dir/aur" "$aur_name"
done
(
  cd "$dist_dir"
  sha256sum "$binary_archive" "$source_archive" \
    "klickmeister-$pkgver-$pkgrel-aur.tar.gz" \
    "klickmeister-bin-$pkgver-$pkgrel-aur.tar.gz" > SHA256SUMS
)
printf '\nRelease files: %s\nInstall: sudo pacman -U %s/%s\n' "$dist_dir" "$dist_dir" "$binary_archive"

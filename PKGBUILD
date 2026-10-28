# Maintainer: Klickmeister contributors
pkgname=klickmeister
pkgver=0.1.3
pkgrel=1
pkgdesc='Auto clicker for KDE Plasma on Wayland'
arch=('x86_64')
url='https://github.com/jerry0205/Auto-Clicker'
license=('MIT')
depends=('gcc-libs' 'glibc' 'kirigami' 'kwindowsystem' 'qqc2-desktop-style' 'qt6-base' 'qt6-declarative' 'qt6-wayland' 'xdg-desktop-portal' 'xdg-desktop-portal-kde')
makedepends=('cargo' 'clang' 'lld' 'pkgconf' 'qt6-tools' 'rust')
options=('!lto')
# Pinned source for standalone builds; releases use their own source snapshot.
_commit=947a56dfe9b5fcd5938406552267030c181c86e2
source=("Auto-Clicker-${_commit}.tar.gz::https://github.com/Jerry0205/Auto-Clicker/archive/${_commit}.tar.gz")
b2sums=('568c6578396336a6d5a5ebe844225478a14054db66df3fc2ffa23816e41a133db8583ea7ce3799dac4508c6d615776fbef3e6504157e24cdf86ab2b489aabf40')

_cargo_jobs() {
  local threads
  threads="$(nproc 2>/dev/null)"
  if [[ ! "$threads" =~ ^[1-9][0-9]*$ ]]; then
    threads=2
  fi

  local jobs=$((threads / 2))
  if ((jobs < 1)); then
    jobs=1
  fi
  printf '%s' "$jobs"
}

prepare() {
  cd "$srcdir/Auto-Clicker-$_commit"
  cargo fetch --locked
}

build() {
  cd "$srcdir/Auto-Clicker-$_commit"
  CARGO_BUILD_JOBS="$(_cargo_jobs)" \
    CARGO_TARGET_DIR=target cargo build --frozen --release
}

check() {
  cd "$srcdir/Auto-Clicker-$_commit"
  CARGO_BUILD_JOBS="$(_cargo_jobs)" \
    CARGO_TARGET_DIR=target cargo test --frozen --release --all-targets
  bash tests/qml-smoke.sh target/release/klickmeister
}

package() {
  cd "$srcdir/Auto-Clicker-$_commit"
  install -Dm755 target/release/klickmeister "$pkgdir/usr/bin/klickmeister"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm644 packaging/io.github.jerry0205.klickmeister.desktop \
    "$pkgdir/usr/share/applications/io.github.jerry0205.klickmeister.desktop"
  install -Dm644 packaging/io.github.jerry0205.klickmeister.metainfo.xml \
    "$pkgdir/usr/share/metainfo/io.github.jerry0205.klickmeister.metainfo.xml"
  install -Dm644 packaging/icons/io.github.jerry0205.klickmeister.svg \
    "$pkgdir/usr/share/icons/hicolor/scalable/apps/io.github.jerry0205.klickmeister.svg"
}

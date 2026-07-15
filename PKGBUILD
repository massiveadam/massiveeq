pkgname=massiveeq-git
pkgver=0.2.1
pkgrel=1
pkgdesc='Device-aware systemwide equalizer for PipeWire and Wayland desktops'
arch=('x86_64')
url='https://github.com/massiveadam/massiveeq'
license=('MIT')
depends=('pipewire>=1:1.4' 'wireplumber>=0.5' 'gtk4>=4.18' 'libadwaita>=1.8' 'libsndfile' 'libsamplerate')
makedepends=('cargo' 'clang' 'git')
provides=('massiveeq')
conflicts=('massiveeq')
source=("$pkgname::git+https://github.com/massiveadam/massiveeq.git")
sha256sums=('SKIP')

pkgver() {
  cd "$pkgname"
  git describe --long --tags 2>/dev/null | sed 's/^v//;s/\([^-]*-g\)/r\1/;s/-/./g' || printf '0.2.1.r%s.%s' "$(git rev-list --count HEAD)" "$(git rev-parse --short HEAD)"
}

build() {
  cd "$pkgname"
  cargo build --release --locked
}

check() {
  cd "$pkgname"
  cargo test --workspace --locked
}

package() {
  cd "$pkgname"
  install -Dm755 target/release/massiveeq "$pkgdir/usr/bin/massiveeq"
  install -Dm755 target/release/massiveeqd "$pkgdir/usr/bin/massiveeqd"
  install -Dm644 packaging/massiveeq.service "$pkgdir/usr/lib/systemd/user/massiveeq.service"
  install -Dm644 packaging/org.massiveeq.Service1.service "$pkgdir/usr/share/dbus-1/services/org.massiveeq.Service1.service"
  install -Dm644 packaging/org.massiveeq.MassiveEQ.desktop "$pkgdir/usr/share/applications/org.massiveeq.MassiveEQ.desktop"
  install -Dm644 packaging/org.massiveeq.MassiveEQ.metainfo.xml "$pkgdir/usr/share/metainfo/org.massiveeq.MassiveEQ.metainfo.xml"
  install -Dm644 packaging/org.massiveeq.MassiveEQ.svg "$pkgdir/usr/share/icons/hicolor/scalable/apps/org.massiveeq.MassiveEQ.svg"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/massiveeq/LICENSE"
}

# Maintainer: dvano <dvano@britzu.com>
#
# Local/testing package: builds directly from this working directory rather
# than a downloaded tarball, since the project has no published releases or
# remote yet. Once it does, switch `source`/`sha256sums` to a real tagged
# tarball/git URL for a proper (e.g. AUR) submission.
pkgname=packet-radio
pkgver=0.1.0
pkgrel=2
pkgdesc="Linux-native AGWPE/AX.25/KISS packet radio client"
arch=('x86_64')
url="https://example.invalid/packet-radio" # TODO: replace once a remote exists
license=('MIT')
depends=('gtk4' 'libadwaita' 'systemd-libs')
makedepends=('cargo' 'pkgconf')
options=('!lto')
source=()
sha256sums=()

prepare() {
	cd "$startdir"
	cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
	cd "$startdir"
	export RUSTUP_TOOLCHAIN=stable
	export CARGO_TARGET_DIR="$srcdir/target"
	cargo build --frozen --release --workspace
}

package() {
	cd "$startdir"
	install -Dm755 "$srcdir/target/release/pr-app" "$pkgdir/usr/bin/packet-radio"
	install -Dm644 packaging/net.packetradio.PacketRadio.desktop \
		"$pkgdir/usr/share/applications/net.packetradio.PacketRadio.desktop"
	install -Dm644 packaging/net.packetradio.PacketRadio.svg \
		"$pkgdir/usr/share/icons/hicolor/scalable/apps/net.packetradio.PacketRadio.svg"
	install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}

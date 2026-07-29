# Maintainer: dvano <dvano@britzu.com>
#
# Local/testing package: builds directly from this working directory rather
# than a downloaded tarball, since the project has no published releases or
# remote yet. Once it does, switch `source`/`sha256sums` to a real tagged
# tarball/git URL for a proper (e.g. AUR) submission.
pkgname=pgprc
pkgver=0.1.0
pkgrel=3
pkgdesc="Pretty Good Packet Radio Client — a Linux-native AGWPE/AX.25/KISS packet radio client"
arch=('x86_64')
url="https://example.invalid/packet-radio" # TODO: replace once a remote exists
license=('MIT')
depends=('gtk4' 'libadwaita' 'systemd-libs' 'hicolor-icon-theme' 'desktop-file-utils')
optdepends=('direwolf: sound modem'
            'linux-lts: native Linux AX.25 modem support'
            'libax25: native Linux AX.25 modem support'
            'ax25-tools: native Linux AX.25 modem support'
            'ax25-apps: native Linux AX.25 modem support')
# Renamed from packet-radio (same project) — replaces/conflicts so
# installing this package cleanly migrates off the old one instead of
# leaving both installed side by side under different names.
replaces=('packet-radio')
conflicts=('packet-radio')
makedepends=('cargo' 'pkgconf')
options=('!lto')
install=pgprc.install
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
	# Always start from a clean target dir. This is a local/testing package
	# rebuilt straight from a working directory that changes between builds,
	# and cargo's incremental rebuild detection is mtime-based — on at least
	# one dev machine used for this project, source files ended up with
	# mtimes that didn't reliably compare as "newer" than a stale cached
	# build artifact, so an incremental `cargo build` silently repackaged an
	# old binary instead of rebuilding. A full rebuild is slower but never
	# silently stale, which matters far more for a package meant to be
	# installed and run, not iterated on.
	rm -rf "$CARGO_TARGET_DIR"
	cargo build --frozen --release --workspace
}

package() {
	cd "$startdir"
	install -Dm755 "$srcdir/target/release/pgprc" "$pkgdir/usr/bin/pgprc"
	install -Dm644 packaging/net.packetradio.PGPRC.desktop \
		"$pkgdir/usr/share/applications/net.packetradio.PGPRC.desktop"
	install -Dm644 packaging/net.packetradio.PGPRC.svg \
		"$pkgdir/usr/share/icons/hicolor/scalable/apps/net.packetradio.PGPRC.svg"
	install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}

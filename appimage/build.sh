#!/usr/bin/env bash
# Builds a portable AppImage for pgprc (Pretty Good Packet Radio Client).
#
# Requires linuxdeploy, linuxdeploy-plugin-gtk.sh, and appimagetool. Use ones
# already on PATH, or drop them (executable) into appimage/tools/ next to
# this script:
#   https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
#   https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh
#   https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
# (chmod +x all three after downloading)
#
# Output: appimage/pgprc-<version>-x86_64.AppImage

set -euo pipefail

cd "$(dirname "$0")"
ROOT="$(cd .. && pwd)"
TOOLS_DIR="$PWD/tools"
APPDIR="$PWD/AppDir"

find_tool() {
    local name="$1"
    if [ -x "$TOOLS_DIR/$name" ]; then
        echo "$TOOLS_DIR/$name"
    elif command -v "$name" >/dev/null 2>&1; then
        command -v "$name"
    else
        echo "Missing $name — see the header of this script for download URLs." >&2
        exit 1
    fi
}

LINUXDEPLOY="$(find_tool linuxdeploy-x86_64.AppImage)"
PLUGIN_GTK="$(find_tool linuxdeploy-plugin-gtk.sh)"
APPIMAGETOOL="$(find_tool appimagetool-x86_64.AppImage)"

echo "==> Building release binary"
(cd "$ROOT" && cargo build --release --workspace)

echo "==> Assembling AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/scalable/apps"
install -Dm755 "$ROOT/target/release/pgprc" "$APPDIR/usr/bin/pgprc"
install -Dm644 "$ROOT/packaging/net.packetradio.PGPRC.desktop" \
    "$APPDIR/usr/share/applications/net.packetradio.PGPRC.desktop"
install -Dm644 "$ROOT/packaging/net.packetradio.PGPRC.svg" \
    "$APPDIR/usr/share/icons/hicolor/scalable/apps/net.packetradio.PGPRC.svg"

echo "==> Running linuxdeploy (shared libs + GTK runtime via the gtk plugin)"
export PATH="$(dirname "$PLUGIN_GTK"):$PATH"
export DEPLOY_GTK_VERSION=4
# The strip binary bundled inside linuxdeploy's own AppImage is too old to
# parse RELR relocations (DT_RELR) that this system's newer glibc/binutils
# emit by default, so it fails on nearly every bundled library. Skip
# stripping entirely rather than fight the toolchain mismatch.
export NO_STRIP=1
# Deploy only (no --output appimage here — see the patchelf workaround below
# for why appimagetool is invoked separately, after AppDir is patched up).
"$LINUXDEPLOY" \
    --appdir "$APPDIR" \
    --executable "$APPDIR/usr/bin/pgprc" \
    --desktop-file "$ROOT/packaging/net.packetradio.PGPRC.desktop" \
    --icon-file "$ROOT/packaging/net.packetradio.PGPRC.svg" \
    --plugin gtk

echo "==> Working around a patchelf/RELR incompatibility"
# linuxdeploy uses patchelf to rewrite every bundled top-level usr/lib/*.so*
# to an $ORIGIN runpath. On this system's toolchain (libraries built with
# RELR relocations, i.e. DT_RELR/.relr.dyn — new enough that even the strip
# bundled inside linuxdeploy's own AppImage can't parse it, see NO_STRIP
# above), patchelf 0.19.1 silently corrupts the rewritten library's layout:
# ld.so segfaults deep inside its own relocation/symbol-lookup code the
# first time it needs to resolve a lazily-bound symbol through one of these
# files. Confirmed by bisection: swapping a patched library for a byte-
# identical pristine copy of the same file (same version, same machine)
# fixes it, so the fault is in patchelf's rewrite, not the library content
# or pgprc itself. Work around it by restoring pristine copies of every
# such library (the GTK plugin's own subdirectory copies — gdk-pixbuf
# loaders, GIO modules, GTK modules, typelibs — are plain `cp`, not
# patchelf'd, so they're untouched and don't need this) and pointing the
# dynamic linker at them via LD_LIBRARY_PATH instead of runpath.
restored=0
for lib in "$APPDIR"/usr/lib/*.so*; do
    [ -f "$lib" ] || continue
    name="$(basename "$lib")"
    if [ -f "/usr/lib/$name" ] && ! cmp -s "/usr/lib/$name" "$lib"; then
        cp "/usr/lib/$name" "$lib"
        restored=$((restored + 1))
    fi
done
echo "Restored $restored pristine librar$([ "$restored" = 1 ] && echo y || echo ies)"

# AppRun only sources apprun-hooks/linuxdeploy-plugin-gtk.sh by name (it's
# not a glob over the whole directory), so append here rather than drop in
# a second hook file that would never actually run.
echo 'export LD_LIBRARY_PATH="$APPDIR/usr/lib:${LD_LIBRARY_PATH:-}"' \
    >> "$APPDIR/apprun-hooks/linuxdeploy-plugin-gtk.sh"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
OUT="pgprc-${VERSION:-0.0.0}-x86_64.AppImage"

echo "==> Running appimagetool"
rm -f "$OUT"
"$APPIMAGETOOL" "$APPDIR" "$PWD/$OUT"

echo "==> Done: appimage/$OUT"

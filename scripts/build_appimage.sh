#!/usr/bin/env bash
# Build axross-files-matrix's AppImage from a release binary.
#
# Usage: scripts/build_appimage.sh BINARY VERSION OUTDIR [TOOLS]
#   BINARY   target/release/axross-files-matrix (built on an old enough glibc:
#            the release workflow builds on Ubuntu 22.04, glibc 2.35)
#   VERSION  0.14.0
#   OUTDIR   where axross-files-matrix-VERSION-x86_64.AppImage goes
#   TOOLS    where the pinned appimagetool and runtime are (fetched there,
#            verified by SHA-256, by scripts/fetch_appimagetool.sh)
#
# The AppDir holds the binary, the desktop entry and the icon from
# packaging/, LICENSE and THIRD_PARTY_LICENSES.md (usr/share/doc). No
# library is bundled: the binary needs glibc 2.34 or later
# (built in AlmaLinux 9: glibc 2.34) and opens X11 / Wayland
# and Vulkan / GL at run time from the system, as any desktop program.
# AppDirOnly=1 builds the AppDir alone (no download: for a local check).
set -euo pipefail

BINARY="${1:?usage: build_appimage.sh BINARY VERSION OUTDIR [TOOLS]}"
VERSION="${2:?version}"
OUTDIR="${3:?outdir}"
TOOLS="${4:-$OUTDIR/tools}"
HERE="$(cd -- "$(dirname -- "$0")/.." && pwd)"

APPDIR="$OUTDIR/axross-files-matrix.AppDir"
rm -rf -- "$APPDIR"
mkdir -p -- "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/scalable/apps"
install -m 0755 -- "$BINARY" "$APPDIR/usr/bin/axross-files-matrix"
install -m 0644 -- "$HERE/packaging/axross-files-matrix.desktop" "$APPDIR/usr/share/applications/axross-files-matrix.desktop"
install -m 0644 -- "$HERE/packaging/axross-files-matrix.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/axross-files-matrix.svg"
# (the licences: the program's and those of what it is built from)
mkdir -p -- "$APPDIR/usr/share/doc/axross-files-matrix"
install -m 0644 -- "$HERE/LICENSE" "$APPDIR/usr/share/doc/axross-files-matrix/LICENSE"
install -m 0644 -- "$HERE/THIRD_PARTY_LICENSES.md" "$APPDIR/usr/share/doc/axross-files-matrix/THIRD_PARTY_LICENSES.md"
# (appimagetool wants the entry and the icon at the AppDir's top as well)
cp -- "$APPDIR/usr/share/applications/axross-files-matrix.desktop" "$APPDIR/axross-files-matrix.desktop"
cp -- "$APPDIR/usr/share/icons/hicolor/scalable/apps/axross-files-matrix.svg" "$APPDIR/axross-files-matrix.svg"
cat > "$APPDIR/AppRun" <<'RUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/axross-files-matrix" "$@"
RUN
chmod 0755 -- "$APPDIR/AppRun"
echo ">>> AppDir: $APPDIR"
if [ "${AppDirOnly:-0}" = 1 ]; then
    exit 0
fi

"$HERE/scripts/fetch_appimagetool.sh" "$TOOLS/appimagetool-x86_64.AppImage"
"$HERE/scripts/fetch_appimagetool.sh" "$TOOLS/runtime-x86_64" --runtime
OUT="$OUTDIR/axross-files-matrix-$VERSION-x86_64.AppImage"
# (extracted, not mounted: no FUSE needed on the build machine)
( cd -- "$TOOLS" && rm -rf squashfs-root && ./appimagetool-x86_64.AppImage --appimage-extract >/dev/null )
ARCH=x86_64 "$TOOLS/squashfs-root/AppRun" --no-appstream \
    --runtime-file "$TOOLS/runtime-x86_64" "$APPDIR" "$OUT"
echo ">>> $OUT"

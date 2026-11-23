#!/usr/bin/env bash
# Build axross-files-matrix.app and a DMG holding it (and the licences).
#
# Usage: scripts/build_macos_app.sh BINARY VERSION OUTDIR THIRD_PARTY [NAME]
#   BINARY       the macOS program (arm64, x86_64 or a universal one)
#   VERSION      0.15.0
#   OUTDIR       where axross-files-matrix.app and NAME.dmg go
#   THIRD_PARTY  THIRD_PARTY_LICENSES.md made for macOS
#   NAME         the DMG's name (default axross-files-matrix-VERSION-macos)
# APP_ONLY=1 builds the .app alone (no hdiutil, no sips: for a check on any
# system). The icon is made from packaging/axross-files-matrix.svg by QuickLook
# (qlmanage) and iconutil where they are; else the app has none.
set -euo pipefail

BINARY="${1:?usage: build_macos_app.sh BINARY VERSION OUTDIR THIRD_PARTY [NAME]}"
VERSION="${2:?version}"
OUTDIR="${3:?outdir}"
THIRD="${4:?third-party licences}"
NAME="${5:-axross-files-matrix-$VERSION-macos}"
HERE="$(cd -- "$(dirname -- "$0")/.." && pwd)"

APP="$OUTDIR/axross-files-matrix.app"
rm -rf -- "$APP"
mkdir -p -- "$APP/Contents/MacOS" "$APP/Contents/Resources"
install -m 0755 -- "$BINARY" "$APP/Contents/MacOS/axross-files-matrix"
install -m 0644 -- "$HERE/LICENSE" "$APP/Contents/Resources/LICENSE"
install -m 0644 -- "$THIRD" "$APP/Contents/Resources/THIRD_PARTY_LICENSES.md"
sed -e "s/@VERSION@/$VERSION/g" -- "$HERE/packaging/macos/Info.plist" > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
if [ "${APP_ONLY:-0}" != 1 ] && command -v qlmanage >/dev/null && command -v iconutil >/dev/null; then
    ICONS="$(mktemp -d)"
    qlmanage -t -s 1024 -o "$ICONS" "$HERE/packaging/axross-files-matrix.svg" >/dev/null 2>&1 || true
    if [ -f "$ICONS/axross-files-matrix.svg.png" ]; then
        mkdir -p "$ICONS/axross-files-matrix.iconset"
        for s in 16 32 128 256 512; do
            sips -z $s $s "$ICONS/axross-files-matrix.svg.png" --out "$ICONS/axross-files-matrix.iconset/icon_${s}x${s}.png" >/dev/null
            sips -z $((s * 2)) $((s * 2)) "$ICONS/axross-files-matrix.svg.png" --out "$ICONS/axross-files-matrix.iconset/icon_${s}x${s}@2x.png" >/dev/null
        done
        iconutil -c icns -o "$APP/Contents/Resources/axross-files-matrix.icns" "$ICONS/axross-files-matrix.iconset"
    else
        echo ">>> no icon: QuickLook could not render the SVG"
    fi
    rm -rf -- "$ICONS"
fi
echo ">>> $APP"
if [ "${APP_ONLY:-0}" = 1 ]; then
    exit 0
fi
STAGE="$(mktemp -d)"
cp -R -- "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
cp -- "$HERE/LICENSE" "$STAGE/LICENSE"
cp -- "$THIRD" "$STAGE/THIRD_PARTY_LICENSES.md"
hdiutil create -volname "axross-files-matrix $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$OUTDIR/$NAME.dmg"
rm -rf -- "$STAGE"
echo ">>> $OUTDIR/$NAME.dmg"

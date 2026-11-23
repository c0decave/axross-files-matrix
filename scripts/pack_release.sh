#!/usr/bin/env bash
# Pack a release archive: the program with its licence, the third-party
# licences and (Linux) its desktop entry and icon, in a folder of its own.
#
# Usage: scripts/pack_release.sh BINARY NAME OUTDIR THIRD_PARTY [linux]
#   BINARY       the built program
#   NAME         the archive's name without .tar.gz (and its folder's)
#   OUTDIR       where NAME.tar.gz goes
#   THIRD_PARTY  THIRD_PARTY_LICENSES.md made for the binary's target
#   linux        with packaging/axross-files-matrix.desktop and axross-files-matrix.svg
set -euo pipefail

BINARY="${1:?usage: pack_release.sh BINARY NAME OUTDIR THIRD_PARTY [linux]}"
NAME="${2:?name}"
OUTDIR="${3:?outdir}"
THIRD="${4:?third-party licences}"
KIND="${5:-}"
HERE="$(cd -- "$(dirname -- "$0")/.." && pwd)"

STAGE="$(mktemp -d)"
trap 'rm -rf -- "$STAGE"' EXIT
DIR="$STAGE/$NAME"
mkdir -p -- "$DIR" "$OUTDIR"
install -m 0755 -- "$BINARY" "$DIR/$(basename -- "$BINARY")"
install -m 0644 -- "$HERE/LICENSE" "$DIR/LICENSE"
install -m 0644 -- "$THIRD" "$DIR/THIRD_PARTY_LICENSES.md"
install -m 0644 -- "$HERE/README.md" "$DIR/README.md"
if [ "$KIND" = linux ]; then
    install -m 0644 -- "$HERE/packaging/axross-files-matrix.desktop" "$DIR/axross-files-matrix.desktop"
    install -m 0644 -- "$HERE/packaging/axross-files-matrix.svg" "$DIR/axross-files-matrix.svg"
fi
tar -C "$STAGE" -czf "$OUTDIR/$NAME.tar.gz" "$NAME"
echo ">>> $OUTDIR/$NAME.tar.gz"
tar -tzf "$OUTDIR/$NAME.tar.gz"

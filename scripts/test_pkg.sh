#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Proves scripts/make_pkg.sh: builds the SDK's example plugin into a package
# (CLAP, LV2, VST3, Audio Unit, AAX as the SDK paths allow), then expands the
# package into the work folder and checks that every built bundle is in its
# component package at its install path. Nothing is installed; the package is
# never opened or run through `installer`.
#
#   scripts/test_pkg.sh --work-dir DIR [--formats LIST]
#   VST3_SDK, AU_SDK, AAX_SDK as for build_plugin.sh (needed per format).
set -euo pipefail
SDK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
die() { echo "test_pkg: $*" >&2; exit 1; }

WORK=""; FORMATS="clap,lv2,vst3,au,aax"
while [ $# -gt 0 ]; do
  case "$1" in
    --work-dir) [ $# -ge 2 ] || die "--work-dir needs a value"; WORK="$2"; shift 2 ;;
    --formats) [ $# -ge 2 ] || die "--formats needs a value"; FORMATS="$2"; shift 2 ;;
    -h|--help) sed -n '2,10p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) die "unknown option $1" ;;
  esac
done
[ -n "$WORK" ] || die "--work-dir is required"
case "$WORK" in /*) ;; *) WORK="$PWD/$WORK" ;; esac
case "$WORK" in /|/tmp|/Users|/Volumes|"$HOME") die "refusing work folder $WORK" ;; esac

OUT="$WORK/out"; BUILD="$WORK/work"; EXP="$WORK/expanded"
NAME="Example Gain"; VERSION="0.3.1"; STEM=ExampleGain
PKG="$OUT/$NAME-$VERSION.pkg"

# pkgutil --expand needs a folder that does not exist yet. Delete only that
# folder, and only if it is directly inside the work folder.
[ "$EXP" = "$WORK/expanded" ] || die "bad expand path"
rm -rf "$EXP"
mkdir -p "$WORK"

"$SDK_ROOT/scripts/build_plugin.sh" --pkg \
  --manifest-path "$SDK_ROOT/examples/editor/Cargo.toml" --version "$VERSION" \
  --name "$NAME" --bundle-id com.opengrid.example --manufacturer OpenGrid \
  --au-subtype ExGn --au-manufacturer OpGd --ttl-bin lv2_ttl --editor --tail --aax-pages \
  --formats "$FORMATS" --out "$OUT" --work-dir "$BUILD"

[ -f "$PKG" ] || die "$PKG was not produced"
pkgutil --check-signature "$PKG" || true
pkgutil --expand-full "$PKG" "$EXP"
xmllint --noout "$EXP/Distribution" || die "Distribution is not well-formed XML"

fail=0
for f in $(echo "$FORMATS" | tr ',' ' '); do
  case "$f" in
    clap) b="$NAME.clap"; loc="/Library/Audio/Plug-Ins/CLAP" ;;
    lv2) b="$STEM.lv2"; loc="/Library/Audio/Plug-Ins/LV2" ;;
    vst3) b="$NAME.vst3"; loc="/Library/Audio/Plug-Ins/VST3" ;;
    au) b="$NAME.component"; loc="/Library/Audio/Plug-Ins/Components" ;;
    aax) b="$NAME.aaxplugin"; loc="/Library/Application Support/Avid/Audio/Plug-Ins" ;;
    *) continue ;;
  esac
  comp="$EXP/$NAME-$f.pkg"
  [ -d "$comp" ] || { echo "FAIL $f: no component package"; fail=1; continue; }
  got="$(sed -n 's/.*install-location="\([^"]*\)".*/\1/p' "$comp/PackageInfo" | head -1)"
  if [ "$got" != "$loc" ]; then echo "FAIL $f: install-location '$got', expected '$loc'"; fail=1; continue; fi
  grep -q "id=\"choice.$f\"" "$EXP/Distribution" || { echo "FAIL $f: no choice in Distribution"; fail=1; continue; }
  if grep -q 'relocatable="true"' "$comp/PackageInfo"; then echo "FAIL $f: bundle is relocatable"; fail=1; continue; fi
  # --expand-full unpacks each Payload to a folder, laid out under the install location.
  if [ -e "$comp/Payload/$b" ]; then
    echo "ok   $f: $loc/$b ($(find "$comp/Payload" | wc -l | tr -d ' ') payload entries)"
  else
    echo "FAIL $f: $b not in the payload"; fail=1
  fi
done
echo "--- payload of every component (top level)"
for c in "$EXP"/*.pkg; do echo "[$(basename "$c")] installs into $(sed -n 's/.*install-location="\([^"]*\)".*/\1/p' "$c/PackageInfo" | head -1)"; (cd "$c/Payload" && find . -maxdepth 1 -mindepth 1); done
[ "$fail" = 0 ] || die "failed"
echo "test_pkg: all formats present at their install paths"

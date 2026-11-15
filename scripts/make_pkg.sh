#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Wraps built plugin bundles in a macOS installer package: one component package
# per format (pkgbuild) and one distribution package (productbuild) in which the
# user chooses the formats. build_plugin.sh calls this with --pkg; it also runs
# on its own over a folder of bundles. It builds and signs packages. It never
# runs `installer`, never installs anything and never uses sudo.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: make_pkg.sh --bundles DIR --name NAME --bundle-id ID --version X.Y.Z
                   --work-dir DIR --out FILE [--lv2-stem STEM] [--formats LIST]
                   [--dry-run]

  --bundles DIR     folder holding the built bundles (NAME.clap, STEM.lv2,
                    NAME.vst3, NAME.component, NAME.aaxplugin, NAME.app)
  --name NAME       product name (bundle file names, installer title)
  --bundle-id ID    bundle identifier prefix; package ids are ID.pkg.<format>
  --version X.Y.Z   package version
  --lv2-stem STEM   LV2 folder stem (default: NAME with only A-Za-z0-9_)
  --formats LIST    comma list of clap,lv2,vst3,au,aax,standalone to include
                    (default: every format whose bundle is in --bundles)
  --work-dir DIR    staging folder; only paths inside it are ever deleted
  --out FILE        the distribution package to write (NAME-VERSION.pkg)

Signing, optional (environment only):
  INSTALLER_IDENTITY  "Developer ID Installer: ..." identity. Set: the component
                      and distribution packages are signed with a secure
                      timestamp. Unset: unsigned, with a note.
  NOTARY_PROFILE      notarytool keychain profile. Set (needs INSTALLER_IDENTITY):
                      the distribution package is submitted, waited for and stapled.
USAGE
}

BUNDLES=""; NAME=""; BUNDLE_ID=""; VERSION=""; WORK=""; OUTPKG=""; STEM=""; FORMATS=""; DRY=0
die() { echo "make_pkg: $*" >&2; exit 1; }
need() { [ $# -ge 2 ] || die "$1 needs a value"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --bundles) need "$@"; BUNDLES="$2"; shift 2 ;;
    --name) need "$@"; NAME="$2"; shift 2 ;;
    --bundle-id) need "$@"; BUNDLE_ID="$2"; shift 2 ;;
    --version) need "$@"; VERSION="$2"; shift 2 ;;
    --lv2-stem) need "$@"; STEM="$2"; shift 2 ;;
    --formats) need "$@"; FORMATS="$2"; shift 2 ;;
    --work-dir) need "$@"; WORK="$2"; shift 2 ;;
    --out) need "$@"; OUTPKG="$2"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
done

[ -n "$BUNDLES" ] || die "--bundles is required"
[ -n "$NAME" ] || die "--name is required"
[ -n "$BUNDLE_ID" ] || die "--bundle-id is required"
[ -n "$VERSION" ] || die "--version is required"
[ -n "$WORK" ] || die "--work-dir is required"
[ -n "$OUTPKG" ] || die "--out is required"
abspath() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$PWD" "$1" ;; esac; }
BUNDLES="$(abspath "$BUNDLES")"; WORK="$(abspath "$WORK")"; OUTPKG="$(abspath "$OUTPKG")"
case "$WORK" in /|/Library*|/System*|/Users|/Volumes) die "refusing work folder $WORK" ;; esac
if [ -z "$STEM" ]; then STEM="$(echo "$NAME" | tr -cd 'A-Za-z0-9_')"; fi
[ -n "$STEM" ] || STEM=plugin

run() {
  printf '+'; printf ' %q' "$@"; printf '\n'
  if [ "$DRY" = 1 ]; then return 0; fi
  "$@"
}
note() { echo "== $*"; }
put() { if [ "$DRY" = 1 ]; then echo "+ write $1"; cat >/dev/null; else cat > "$1"; fi; }

# Delete a path only if it is strictly inside the work folder.
safe_rm() {
  local p="$1"
  [ -n "$p" ] || die "refusing to delete an empty path"
  case "$p" in "$WORK"/?*) ;; *) die "refusing to delete $p: not inside $WORK" ;; esac
  case "$p" in *"/../"*|*"/..") die "refusing to delete $p: contains .." ;; esac
  run rm -rf "$p"
}

IDENT="${INSTALLER_IDENTITY:-}"
if [ -n "${NOTARY_PROFILE:-}" ] && [ -z "$IDENT" ]; then
  die "NOTARY_PROFILE needs INSTALLER_IDENTITY: Apple only notarises Developer ID signed packages"
fi

# format -> bundle file name, install location, choice title, description
fmt_bundle() { case "$1" in
  clap) echo "$NAME.clap" ;; lv2) echo "$STEM.lv2" ;; vst3) echo "$NAME.vst3" ;;
  au) echo "$NAME.component" ;; aax) echo "$NAME.aaxplugin" ;; standalone) echo "$NAME.app" ;; esac; }
fmt_location() { case "$1" in
  clap) echo "/Library/Audio/Plug-Ins/CLAP" ;; lv2) echo "/Library/Audio/Plug-Ins/LV2" ;;
  vst3) echo "/Library/Audio/Plug-Ins/VST3" ;; au) echo "/Library/Audio/Plug-Ins/Components" ;;
  aax) echo "/Library/Application Support/Avid/Audio/Plug-Ins" ;; standalone) echo "/Applications" ;; esac; }
fmt_title() { case "$1" in
  clap) echo "CLAP" ;; lv2) echo "LV2" ;; vst3) echo "VST3" ;; au) echo "Audio Unit" ;;
  aax) echo "AAX (Pro Tools)" ;; standalone) echo "Standalone app" ;; esac; }

ALL="clap lv2 vst3 au aax standalone"
if [ -z "$FORMATS" ]; then
  LIST=""
  for f in $ALL; do
    if [ -e "$BUNDLES/$(fmt_bundle "$f")" ]; then LIST="$LIST $f"; fi
  done
else
  LIST="$(echo "$FORMATS" | tr ',' ' ')"
  for f in $LIST; do
    case " $ALL " in *" $f "*) ;; *) die "unknown format $f" ;; esac
    if [ "$DRY" = 0 ] && [ ! -e "$BUNDLES/$(fmt_bundle "$f")" ]; then
      die "format $f was asked for but $BUNDLES/$(fmt_bundle "$f") does not exist"
    fi
  done
fi
[ -n "${LIST// /}" ] || die "no bundles found in $BUNDLES"

if [ -n "$IDENT" ]; then
  note "packages: signed with INSTALLER_IDENTITY, secure timestamp"
else
  note "packages: UNSIGNED (INSTALLER_IDENTITY is not set). macOS will warn on opening an unsigned package; set a Developer ID Installer identity to ship it"
fi
SIGN=()
if [ -n "$IDENT" ]; then SIGN=(--sign "$IDENT" --timestamp); fi

STAGE="$WORK/stage"; COMP="$WORK/components"; PLISTS="$WORK/plists"
safe_rm "$STAGE"; safe_rm "$COMP"; safe_rm "$PLISTS"
run mkdir -p "$STAGE" "$COMP" "$PLISTS" "$(dirname "$OUTPKG")"

CHOICES=""; REFS=""; LINES=""
for f in $LIST; do
  b="$(fmt_bundle "$f")"; loc="$(fmt_location "$f")"; title="$(fmt_title "$f")"
  pid="$BUNDLE_ID.pkg.$f"
  note "component package: $f -> $loc/$b"
  run mkdir -p "$STAGE/$f"
  # ditto keeps symlinks, resource forks and signatures intact.
  run ditto "$BUNDLES/$b" "$STAGE/$f/$b"
  # A plug-in must land in its folder, not "relocate" to another copy on disk.
  cp_plist="$PLISTS/$f.plist"
  run pkgbuild --analyze --root "$STAGE/$f" "$cp_plist"
  if [ "$DRY" = 0 ]; then
    i=0
    while /usr/libexec/PlistBuddy -c "Print :$i:BundleIsRelocatable" "$cp_plist" >/dev/null 2>&1; do
      /usr/libexec/PlistBuddy -c "Set :$i:BundleIsRelocatable false" "$cp_plist"
      i=$((i + 1))
    done
  fi
  run pkgbuild --root "$STAGE/$f" --component-plist "$cp_plist" \
    --identifier "$pid" --version "$VERSION" --install-location "$loc" \
    --ownership recommended ${SIGN[@]+"${SIGN[@]}"} "$COMP/$NAME-$f.pkg"
  REFS="$REFS
  <pkg-ref id=\"$pid\" version=\"$VERSION\" onConclusion=\"none\">$NAME-$f.pkg</pkg-ref>"
  CHOICES="$CHOICES
  <choice id=\"choice.$f\" title=\"$title\" description=\"Installs $b in $loc\" start_selected=\"true\">
    <pkg-ref id=\"$pid\"/>
  </choice>"
  LINES="$LINES
    <line choice=\"choice.$f\"/>"
done

# Distribution XML: the user picks the formats.
xml_escape() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' -e 's/"/\&quot;/g'; }
DIST="$WORK/distribution.xml"
note "distribution"
put "$DIST" <<DISTXML
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
  <title>$(xml_escape "$NAME")</title>
  <options customize="always" require-scripts="false" rootVolumeOnly="true"/>
  <domains enable_anywhere="false" enable_currentUserHome="false" enable_localSystem="true"/>
  <choices-outline>$LINES
  </choices-outline>$CHOICES$REFS
</installer-gui-script>
DISTXML

run productbuild --distribution "$DIST" --package-path "$COMP" \
  --version "$VERSION" ${SIGN[@]+"${SIGN[@]}"} "$OUTPKG"

if [ -n "${NOTARY_PROFILE:-}" ]; then
  note "notarisation of the package (notarytool profile from NOTARY_PROFILE)"
  run xcrun notarytool submit "$OUTPKG" --keychain-profile "$NOTARY_PROFILE" --wait
  run xcrun stapler staple "$OUTPKG"
  run xcrun stapler validate "$OUTPKG"
fi

if [ "$DRY" = 0 ]; then
  note "package check"
  pkgutil --check-signature "$OUTPKG" || true
fi
note "package: $OUTPKG"

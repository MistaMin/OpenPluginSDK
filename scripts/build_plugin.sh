#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Builds a plugin written with OpenPluginSDK into distributable macOS bundles:
# CLAP, LV2, VST3, Audio Unit, AAX and a standalone app. Signing, notarisation
# and PACE signing are optional. See docs/BUILD.md. Run with --help.
set -euo pipefail

SDK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage: build_plugin.sh [options]

Builds a plugin crate into signed (ad-hoc unless told otherwise) macOS bundles
and collects them in one folder. Every option can also be set by the
environment variable in brackets.

Plugin
  --manifest-path FILE   Cargo.toml of the plugin crate               [OP_MANIFEST]
  --package NAME         cargo package to build (default: from manifest) [OP_PACKAGE]
  --lib-name NAME        library file stem, if not the package name with
                         '-' as '_'                                    [OP_LIB_NAME]
  --name NAME            product name: bundle file names, display name [OP_NAME]
  --bundle-id ID         bundle identifier prefix, e.g. com.acme.verb  [OP_BUNDLE_ID]
  --version X.Y.Z        bundle version (default: from the manifest)   [OP_VERSION]
  --manufacturer NAME    manufacturer display name (default: OpenPlugin) [OP_MANUFACTURER]
  --au-subtype CODE      Audio Unit subtype, four characters           [OP_AU_SUBTYPE]
  --au-manufacturer CODE Audio Unit manufacturer code, four characters [OP_AU_MANUFACTURER]
                         Both may be left out when the plugin pins its codes
                         (Plugin::au_ids); codes that differ from the pinned ones stop the build.
  --au-type TYPE         Audio Unit component type: auto, aufx, aumu, aumf [OP_AU_TYPE=auto]
  --au-version N|release Audio Unit component version: a number, or `release` to derive
                         it from --version as major*65536 + minor*256 + patch. A host takes a
                         higher number for an update        [OP_AU_VERSION=release]
  --ttl-bin NAME         the plugin's cargo bin that writes the LV2 files;
                         called as `NAME <folder> <library stem>`      [OP_TTL_BIN]
  --standalone-bin NAME  the plugin's cargo bin that runs standalone   [OP_STANDALONE_BIN]
  --features LIST        cargo features for the plugin crate           [OP_FEATURES]

Formats and options
  --formats LIST         comma list of clap,lv2,vst3,au,aax,standalone,
                         or "all" (default: clap,lv2,vst3,au,aax)      [OP_FORMATS]
  --editor / --no-editor shells host the plugin's editor (default: off) [OP_EDITOR=1]
  --tail / --no-tail     shells report the plugin's tail (default: off) [OP_TAIL=1]
  --aax-pages            write an AAX page table into the AAX bundle   [OP_AAX_PAGES=1]
  --pkg                  also wrap the bundles in an installer package,
                         OUT/NAME-VERSION.pkg, by scripts/make_pkg.sh  [OP_PKG=1]
  --archs LIST           arm64, x86_64 or universal (default: the host's) [OP_ARCHS]
  --deployment-target V  macOS deployment target (default: 11.0)       [OP_DEPLOYMENT_TARGET]

Paths
  --vst3-sdk DIR         VST3 SDK checkout                             [VST3_SDK]
  --au-sdk DIR           AudioUnitSDK checkout                         [AU_SDK]
  --aax-sdk DIR          AAX SDK checkout                              [AAX_SDK]
  --ara-sdk DIR          ARA SDK checkout: the VST3 is an ARA 2 plug-in; the
                         plugin must export op_ara_analyze               [OP_ARA_SDK]
  --out DIR              folder that receives the bundles (default: ./dist) [OP_OUT]
  --work-dir DIR         build folder for cargo and cmake
                         (default: ./build/openplugin)                 [OP_WORK_DIR]
  --jobs N               parallel jobs for cargo and cmake             [OP_JOBS]

Signing, all optional (environment only; nothing here is printed)
  CODESIGN_IDENTITY      "Developer ID Application: ..." identity. Set: sign with
                         hardened runtime and a secure timestamp. Unset: ad-hoc.
  NOTARY_PROFILE         notarytool keychain profile. Set (needs CODESIGN_IDENTITY):
                         zip, submit, wait and staple each bundle.
  INSTALLER_IDENTITY     "Developer ID Installer: ..." identity, for --pkg. Set:
                         sign the installer packages. Unset: unsigned package.
                         With NOTARY_PROFILE the package is notarised and stapled too.
  WRAPTOOL               path to PACE wraptool. Set, with PACE_ACCOUNT and
  PACE_WCGUID            (the iLok account and the wrap configuration GUID):
                         PACE-sign the AAX bundle (needs CODESIGN_IDENTITY).
                         wraptool asks for the account password itself.

Other
  --dry-run              print every command, run none, create nothing
  --no-verify            skip the codesign and build-path checks at the end
  -h, --help             this text
USAGE
}

# ---------- settings ----------
MANIFEST="${OP_MANIFEST:-}"; PACKAGE="${OP_PACKAGE:-}"; LIB_NAME="${OP_LIB_NAME:-}"
NAME="${OP_NAME:-}"; BUNDLE_ID="${OP_BUNDLE_ID:-}"; VERSION="${OP_VERSION:-}"
MANUFACTURER="${OP_MANUFACTURER:-OpenPlugin}"
AU_SUBTYPE="${OP_AU_SUBTYPE:-}"; AU_MANUFACTURER="${OP_AU_MANUFACTURER:-}"; AU_TYPE="${OP_AU_TYPE:-auto}"; AU_VERSION="${OP_AU_VERSION:-release}"
TTL_BIN="${OP_TTL_BIN:-}"; STANDALONE_BIN="${OP_STANDALONE_BIN:-}"; FEATURES="${OP_FEATURES:-}"
FORMATS="${OP_FORMATS:-clap,lv2,vst3,au,aax}"
EDITOR="${OP_EDITOR:-0}"; TAIL="${OP_TAIL:-0}"; AAX_PAGES="${OP_AAX_PAGES:-0}"
PKG="${OP_PKG:-0}"
ARCHS="${OP_ARCHS:-}"; DEPLOY="${OP_DEPLOYMENT_TARGET:-11.0}"
VST3_SDK="${VST3_SDK:-}"; AU_SDK="${AU_SDK:-}"; AAX_SDK="${AAX_SDK:-}"; ARA_SDK="${OP_ARA_SDK:-}"
OUT="${OP_OUT:-$PWD/dist}"; WORK="${OP_WORK_DIR:-$PWD/build/openplugin}"
JOBS="${OP_JOBS:-$(sysctl -n hw.ncpu 2>/dev/null || echo 2)}"
DRY=0; VERIFY=1

die() { echo "build_plugin: $*" >&2; exit 1; }
need() { [ $# -ge 2 ] || die "$1 needs a value"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest-path) need "$@"; MANIFEST="$2"; shift 2 ;;
    --package) need "$@"; PACKAGE="$2"; shift 2 ;;
    --lib-name) need "$@"; LIB_NAME="$2"; shift 2 ;;
    --name) need "$@"; NAME="$2"; shift 2 ;;
    --bundle-id) need "$@"; BUNDLE_ID="$2"; shift 2 ;;
    --version) need "$@"; VERSION="$2"; shift 2 ;;
    --manufacturer) need "$@"; MANUFACTURER="$2"; shift 2 ;;
    --au-subtype) need "$@"; AU_SUBTYPE="$2"; shift 2 ;;
    --au-manufacturer) need "$@"; AU_MANUFACTURER="$2"; shift 2 ;;
    --au-type) need "$@"; AU_TYPE="$2"; shift 2 ;;
    --au-version) need "$@"; AU_VERSION="$2"; shift 2 ;;
    --ttl-bin) need "$@"; TTL_BIN="$2"; shift 2 ;;
    --standalone-bin) need "$@"; STANDALONE_BIN="$2"; shift 2 ;;
    --features) need "$@"; FEATURES="$2"; shift 2 ;;
    --formats) need "$@"; FORMATS="$2"; shift 2 ;;
    --editor) EDITOR=1; shift ;;
    --no-editor) EDITOR=0; shift ;;
    --tail) TAIL=1; shift ;;
    --no-tail) TAIL=0; shift ;;
    --aax-pages) AAX_PAGES=1; shift ;;
    --pkg) PKG=1; shift ;;
    --archs) need "$@"; ARCHS="$2"; shift 2 ;;
    --deployment-target) need "$@"; DEPLOY="$2"; shift 2 ;;
    --vst3-sdk) need "$@"; VST3_SDK="$2"; shift 2 ;;
    --au-sdk) need "$@"; AU_SDK="$2"; shift 2 ;;
    --aax-sdk) need "$@"; AAX_SDK="$2"; shift 2 ;;
    --ara-sdk) need "$@"; ARA_SDK="$2"; shift 2 ;;
    --out) need "$@"; OUT="$2"; shift 2 ;;
    --work-dir) need "$@"; WORK="$2"; shift 2 ;;
    --jobs) need "$@"; JOBS="$2"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    --no-verify) VERIFY=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
done

# ---------- helpers ----------
# Print a command shell-quoted. In a real run, also run it.
run() {
  printf '+'; printf ' %q' "$@"; printf '\n'
  if [ "$DRY" = 1 ]; then return 0; fi
  "$@"
}
note() { echo "== $*"; }
# Write stdin to a file (or show that it would be).
put() {
  if [ "$DRY" = 1 ]; then echo "+ write $1"; cat >/dev/null; else cat > "$1"; fi
}
abspath() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$PWD" "$1" ;; esac; }
has_format() { case ",$FORMATS," in *",$1,"*) return 0 ;; esac; return 1; }
triple() { case "$1" in arm64) echo aarch64-apple-darwin ;; x86_64) echo x86_64-apple-darwin ;; *) die "unknown architecture $1" ;; esac; }

# ---------- validate ----------
[ -n "$MANIFEST" ] || die "--manifest-path is required"
[ -f "$MANIFEST" ] || die "$MANIFEST not found"
MANIFEST="$(abspath "$MANIFEST")"
if [ "$FORMATS" = all ]; then FORMATS="clap,lv2,vst3,au,aax,standalone"; fi
for f in $(echo "$FORMATS" | tr ',' ' '); do
  case "$f" in clap|lv2|vst3|au|aax|standalone) ;; *) die "unknown format $f" ;; esac
done
manifest_field() { sed -n "s/^$1 *= *\"\\(.*\\)\".*/\\1/p" "$MANIFEST" | head -1; }
if [ -z "$PACKAGE" ]; then PACKAGE="$(manifest_field name)"; fi
[ -n "$PACKAGE" ] || die "package name not found in the manifest; give --package"
if [ -z "$LIB_NAME" ]; then LIB_NAME="$(echo "$PACKAGE" | tr '-' '_')"; fi
if [ -z "$NAME" ]; then NAME="$PACKAGE"; fi
if [ -z "$VERSION" ]; then
  VERSION="$(manifest_field version)"
  [ -n "$VERSION" ] || die "version not found in the manifest (workspace-inherited?); give --version"
fi
VERSION="${VERSION%%-*}"   # bundle versions are numbers only
if [ "$AU_VERSION" = release ]; then
  IFS=. read -r v_major v_minor v_patch <<<"$VERSION"
  AU_VERSION=$(( ${v_major:-0} * 65536 + ${v_minor:-0} * 256 + ${v_patch:-0} ))
fi
case "$AU_VERSION" in (*[!0-9]*|"") die "--au-version is a number or release, not $AU_VERSION" ;; esac
if [ -z "$BUNDLE_ID" ]; then
  BUNDLE_ID="com.example.$(echo "$NAME" | tr -cd 'A-Za-z0-9' | tr 'A-Z' 'a-z')"
  echo "build_plugin: no --bundle-id; using $BUNDLE_ID" >&2
fi
if [ -z "$ARCHS" ]; then
  case "$(uname -m)" in arm64) ARCHS=arm64 ;; *) ARCHS=x86_64 ;; esac
fi
if [ "$ARCHS" = universal ]; then ARCHS="arm64 x86_64"; fi
ARCHS="$(echo "$ARCHS" | tr ',' ' ')"
for a in $ARCHS; do triple "$a" >/dev/null; done
case "$(uname -m)" in arm64) HOST_ARCH=arm64 ;; *) HOST_ARCH=x86_64 ;; esac

SHELLS=""
if has_format vst3; then SHELLS="$SHELLS OpenPluginVST3"; fi
if has_format au; then SHELLS="$SHELLS OpenPluginAU"; fi
if has_format aax; then SHELLS="$SHELLS OpenPluginAAX"; fi
check_sdk() { # variable, value, probe file, what
  [ -n "$2" ] || die "$1 is not set; the $4 shell needs it"
  if [ "$DRY" = 1 ] || [ -e "$2/$3" ]; then return 0; fi
  die "$1=$2 does not look like $4 ($3 missing)"
}
if has_format vst3; then check_sdk VST3_SDK "$VST3_SDK" public.sdk/source/main/macmain.cpp "a VST3 SDK"; fi
if has_format au; then check_sdk AU_SDK "$AU_SDK" include/AudioUnitSDK/AUBase.h "an AudioUnitSDK"; fi
if has_format aax; then check_sdk AAX_SDK "$AAX_SDK" Interfaces/AAX.h "an AAX SDK"; fi
if has_format au || [ "$AAX_PAGES" = 1 ]; then
  # Both left out: the plugin must pin them (Plugin::au_ids); the shell build asks it and
  # stops when it pins none.
  if [ -n "$AU_SUBTYPE$AU_MANUFACTURER" ] && { [ "${#AU_SUBTYPE}" != 4 ] || [ "${#AU_MANUFACTURER}" != 4 ]; }; then
    die "--au-subtype and --au-manufacturer must be four characters each (or both left out, for a plugin that pins its codes)"
  fi
fi
case "$AU_TYPE" in auto|aufx|aumu|aumf) ;; *) die "--au-type must be auto, aufx, aumu or aumf" ;; esac
if has_format lv2 && [ -z "$TTL_BIN" ]; then die "lv2 needs --ttl-bin, the plugin's bin that writes the description files"; fi
if has_format standalone && [ -z "$STANDALONE_BIN" ]; then die "standalone needs --standalone-bin, the plugin's bin that runs standalone"; fi

if [ -n "${NOTARY_PROFILE:-}" ] && [ -z "${CODESIGN_IDENTITY:-}" ]; then
  die "NOTARY_PROFILE needs CODESIGN_IDENTITY: Apple only notarises Developer ID signed code"
fi
if [ "$PKG" = 1 ] && [ -n "${NOTARY_PROFILE:-}" ] && [ -z "${INSTALLER_IDENTITY:-}" ]; then
  die "--pkg with NOTARY_PROFILE needs INSTALLER_IDENTITY: Apple only notarises Developer ID signed packages"
fi
if [ -n "${WRAPTOOL:-}" ]; then
  if ! has_format aax; then echo "build_plugin: WRAPTOOL is set but aax is not built; ignoring it" >&2; fi
  [ -n "${CODESIGN_IDENTITY:-}" ] || die "WRAPTOOL needs CODESIGN_IDENTITY"
  if [ -z "${PACE_ACCOUNT:-}" ] || [ -z "${PACE_WCGUID:-}" ]; then die "WRAPTOOL needs PACE_ACCOUNT and PACE_WCGUID"; fi
fi

OUT="$(abspath "$OUT")"; WORK="$(abspath "$WORK")"
TARGET_DIR="$WORK/target"
STEM="$(echo "$NAME" | tr -cd 'A-Za-z0-9_')"   # file stem safe inside an LV2 IRI
if [ -z "$STEM" ]; then STEM=plugin; fi

# ---------- environment for the builds ----------
export CARGO_BUILD_JOBS="$JOBS"
export CARGO_TARGET_DIR="$TARGET_DIR"
export MACOSX_DEPLOYMENT_TARGET="$DEPLOY"
# No build path in a binary. The unit separator is how cargo splits flags.
SEP="$(printf '\037')"
MANIFEST_DIR="$(dirname "$MANIFEST")"
# Path dependencies usually sit next to the workspace, so its parent folder is
# remapped too.
WS_MANIFEST="$(cargo locate-project --workspace --message-format plain --manifest-path "$MANIFEST" 2>/dev/null || true)"
if [ -n "$WS_MANIFEST" ]; then WS_ROOT="$(dirname "$WS_MANIFEST")"; else WS_ROOT="$MANIFEST_DIR"; fi
WS_PARENT="$(dirname "$WS_ROOT")"
if [ "$WS_PARENT" = / ]; then WS_PARENT="$WS_ROOT"; fi
CARGO_DIR="${CARGO_HOME:-$HOME/.cargo}"
# The last matching prefix wins, so the general ones come first.
REMAPS=("$HOME=home" "$WS_PARENT=deps" "$SDK_ROOT=openplugin-sdk" "$CARGO_DIR=cargo" "$WS_ROOT=workspace"
        "$MANIFEST_DIR=plugin" "$WORK=build")
for sdk in "$VST3_SDK=vst3sdk" "$AU_SDK=ausdk" "$AAX_SDK=aaxsdk"; do
  if [ -n "${sdk%%=*}" ]; then REMAPS+=("$sdk"); fi
done
RF=""; FMAP=""
for r in "${REMAPS[@]}"; do
  RF="${RF:+$RF$SEP}--remap-path-prefix=$r"
  FMAP="$FMAP -ffile-prefix-map='${r%%=*}'=${r#*=}"
done
export CARGO_ENCODED_RUSTFLAGS="$RF"

CARGO_FEATURES=()
if [ -n "$FEATURES" ]; then CARGO_FEATURES=(--features "$FEATURES"); fi

note "plugin $NAME ($PACKAGE) $VERSION, formats: $FORMATS, archs: $ARCHS"
if [ "$DRY" = 1 ]; then note "dry run: nothing below is executed"; fi
run mkdir -p "$WORK" "$OUT"

# ---------- Rust ----------
cargo_build() { # triple, extra args...
  local t="$1"; shift
  run cargo build --release --manifest-path "$MANIFEST" -p "$PACKAGE" --target "$t" ${CARGO_FEATURES[@]+"${CARGO_FEATURES[@]}"} "$@"
}
rel() { echo "$TARGET_DIR/$(triple "$1")/release"; }
universal() { # output, inputs... (one input is just copied)
  local out="$1"; shift
  run mkdir -p "$(dirname "$out")"
  if [ $# = 1 ]; then run cp "$1" "$out"; else run lipo -create "$@" -output "$out"; fi
}
NEED_DYLIB=0
if has_format clap || has_format lv2; then NEED_DYLIB=1; fi

note "Rust libraries"
for a in $ARCHS; do cargo_build "$(triple "$a")" --lib; done
UNI="$WORK/universal"
DYLIB="$UNI/lib$LIB_NAME.dylib"; STATICLIB="$UNI/lib$LIB_NAME.a"
if [ "$NEED_DYLIB" = 1 ]; then
  ins=(); for a in $ARCHS; do ins+=("$(rel "$a")/lib$LIB_NAME.dylib"); done
  universal "$DYLIB" "${ins[@]}"
fi
if [ -n "$SHELLS" ]; then
  ins=(); for a in $ARCHS; do ins+=("$(rel "$a")/lib$LIB_NAME.a"); done
  universal "$STATICLIB" "${ins[@]}"
fi
if has_format standalone; then
  for a in $ARCHS; do cargo_build "$(triple "$a")" --bin "$STANDALONE_BIN"; done
  ins=(); for a in $ARCHS; do ins+=("$(rel "$a")/$STANDALONE_BIN"); done
  universal "$UNI/$STANDALONE_BIN" "${ins[@]}"
fi
if has_format lv2; then
  # Runs on this machine, so it is built for the host architecture.
  cargo_build "$(triple "$HOST_ARCH")" --bin "$TTL_BIN"
fi

# ---------- bundles ----------
plist() { # executable, package type, id suffix
  cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>$1</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID$3</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundlePackageType</key><string>$2</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>$DEPLOY</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
}
BUNDLES=()   # everything that gets signed, in order

if has_format clap; then
  note "CLAP"
  B="$OUT/$NAME.clap"
  run rm -rf "$B"
  run mkdir -p "$B/Contents/MacOS"
  run cp "$DYLIB" "$B/Contents/MacOS/$NAME"
  run install_name_tool -id "@loader_path/$NAME" "$B/Contents/MacOS/$NAME"
  plist "$NAME" BNDL "" | put "$B/Contents/Info.plist"
  printf 'BNDL????' | put "$B/Contents/PkgInfo"
  BUNDLES+=("$B")
fi

if has_format lv2; then
  note "LV2"
  B="$OUT/$STEM.lv2"
  run rm -rf "$B"
  run mkdir -p "$B"
  run cp "$DYLIB" "$B/$STEM.dylib"
  run install_name_tool -id "@loader_path/$STEM.dylib" "$B/$STEM.dylib"
  run "$(rel "$HOST_ARCH")/$TTL_BIN" "$B" "$STEM"
  BUNDLES+=("$B/$STEM.dylib")
fi

if [ -n "$SHELLS" ]; then
  note "native shells (cmake)"
  CM="$WORK/cmake"
  CMA="$(echo "$ARCHS" | tr ' ' ';')"
  onoff() { if has_format "$1"; then echo ON; else echo OFF; fi; }
  tf() { if [ "$1" = 1 ]; then echo ON; else echo OFF; fi; }
  run cmake -S "$SDK_ROOT/native" -B "$CM" -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_OSX_DEPLOYMENT_TARGET="$DEPLOY" "-DCMAKE_OSX_ARCHITECTURES=$CMA" \
    "-DCMAKE_C_FLAGS=$FMAP" "-DCMAKE_CXX_FLAGS=$FMAP" "-DCMAKE_OBJCXX_FLAGS=$FMAP" \
    "-DOP_RUST_LIB=$STATICLIB" \
    "-DOP_VST3_SDK=$VST3_SDK" "-DOP_AU_SDK=$AU_SDK" "-DOP_AAX_SDK=$AAX_SDK" "-DOP_ARA_SDK=$ARA_SDK" \
    "-DOP_BUILD_VST3=$(onoff vst3)" "-DOP_BUILD_AU=$(onoff au)" "-DOP_BUILD_AAX=$(onoff aax)" \
    "-DOP_PRODUCT=$NAME" "-DOP_BUNDLE_ID=$BUNDLE_ID" "-DOP_VERSION=$VERSION" \
    "-DOP_AU_VERSION=$AU_VERSION" "-DOP_MANUFACTURER=$MANUFACTURER" "-DOP_AU_SUBTYPE=$AU_SUBTYPE" "-DOP_AU_MANUFACTURER=$AU_MANUFACTURER" \
    "-DOP_AU_TYPE=$AU_TYPE" \
    "-DOP_EDITOR=$(tf "$EDITOR")" "-DOP_TAIL=$(tf "$TAIL")" "-DOP_AAX_PAGES=$(tf "$AAX_PAGES")"
  # Only the bundles: the SDK's own proof programs test its example, not this plugin.
  run cmake --build "$CM" --config Release -j "$JOBS" --target $SHELLS
  for pair in vst3:vst3 au:component aax:aaxplugin; do
    f="${pair%%:*}"; ext="${pair#*:}"
    if has_format "$f"; then
      run rm -rf "$OUT/$NAME.$ext"
      run cp -R "$CM/$NAME.$ext" "$OUT/$NAME.$ext"
      BUNDLES+=("$OUT/$NAME.$ext")
    fi
  done
fi

if has_format standalone; then
  note "standalone"
  B="$OUT/$NAME.app"
  run rm -rf "$B"
  run mkdir -p "$B/Contents/MacOS"
  run cp "$UNI/$STANDALONE_BIN" "$B/Contents/MacOS/$NAME"
  plist "$NAME" APPL ".app" | put "$B/Contents/Info.plist"
  printf 'APPL????' | put "$B/Contents/PkgInfo"
  BUNDLES+=("$B")
fi

# ---------- signing ----------
IDENT="${CODESIGN_IDENTITY:-}"
sign() { # path
  if [ -n "$IDENT" ]; then
    run codesign --force --deep --timestamp --options runtime --sign "$IDENT" "$1"
  else
    run codesign --force --deep --sign - "$1"
  fi
}

if [ -n "$IDENT" ]; then note "signing: CODESIGN_IDENTITY, hardened runtime, timestamp"; else note "signing: ad-hoc"; fi
for b in ${BUNDLES[@]+"${BUNDLES[@]}"}; do sign "$b"; done

if [ -n "${WRAPTOOL:-}" ] && has_format aax; then
  note "PACE signing (wraptool)"
  A="$OUT/$NAME.aaxplugin"; S="$WORK/pace"
  run rm -rf "$S"
  run mkdir -p "$S"
  run "$WRAPTOOL" sign --verbose --account "$PACE_ACCOUNT" --wcguid "$PACE_WCGUID" \
    --signid "$IDENT" --in "$A" --out "$S/$NAME.aaxplugin" --autoinstall on
  run rm -rf "$A"
  run cp -R "$S/$NAME.aaxplugin" "$A"
  # Seal the PACE metadata with the Apple signature, then check both.
  sign "$A"
  run "$WRAPTOOL" verify --verbose --in "$A"
fi

if [ -n "${NOTARY_PROFILE:-}" ]; then
  note "notarisation (notarytool profile from NOTARY_PROFILE)"
  run mkdir -p "$WORK/notary"
  for b in ${BUNDLES[@]+"${BUNDLES[@]}"}; do
    # The LV2 library is submitted inside its folder, with the description files.
    case "$b" in *.dylib) b="$(dirname "$b")" ;; esac
    z="$WORK/notary/$(basename "$b").zip"
    run rm -f "$z"
    run ditto -c -k --keepParent "$b" "$z"
    run xcrun notarytool submit "$z" --keychain-profile "$NOTARY_PROFILE" --wait
    case "$b" in
      *.lv2) echo "(an LV2 folder cannot hold a ticket; Gatekeeper checks it online)" ;;
      *) run xcrun stapler staple "$b" ;;
    esac
  done
fi

# ---------- installer package ----------
if [ "$PKG" = 1 ]; then
  note "installer package"
  PKG_FORMATS="$FORMATS"
  PKG_DRY=(); if [ "$DRY" = 1 ]; then PKG_DRY=(--dry-run); fi
  run "$SDK_ROOT/scripts/make_pkg.sh" --bundles "$OUT" --name "$NAME" --lv2-stem "$STEM" \
    --bundle-id "$BUNDLE_ID" --version "$VERSION" --formats "$PKG_FORMATS" \
    --work-dir "$WORK/pkg" --out "$OUT/$NAME-$VERSION.pkg" ${PKG_DRY[@]+"${PKG_DRY[@]}"}
fi

# ---------- checks ----------
leaks=0
if [ "$VERIFY" = 1 ] && [ "$DRY" = 0 ]; then
  note "checks"
  for b in ${BUNDLES[@]+"${BUNDLES[@]}"}; do
    codesign --verify --deep --strict "$b"
    echo "codesign ok: $b"
  done
  # No build path in a shipped binary.
  for b in ${BUNDLES[@]+"${BUNDLES[@]}"}; do
    while IFS= read -r f; do
      for p in "$WORK" "$WS_PARENT" "$MANIFEST_DIR" "$SDK_ROOT" "$HOME"; do
        if strings -a "$f" | grep -qF "$p"; then echo "path leak: $f contains $p"; leaks=1; fi
      done
    done < <(find "$b" -type f \( -perm -u+x -o -name '*.dylib' \))
    case "$b" in *.dylib) echo "install name: $(otool -D "$b" | tail -1)" ;; esac
  done
  if [ "$leaks" = 0 ]; then echo "no build path found in any binary"; fi
  # Every binary carries exactly the requested slices, so "universal" is two
  # slices in each bundle and not one slice with a universal name.
  want="$(echo "$ARCHS" | tr ' ' '\n' | sort | tr '\n' ' ')"
  for b in ${BUNDLES[@]+"${BUNDLES[@]}"}; do
    while IFS= read -r f; do
      # PACE's wraptool adds its own universal __Pace_Eden bundle to the AAX; that
      # binary is PACE's, not built here, so only the plugin's own binaries are checked.
      case "$f" in *"/__Pace_Eden.bundle/"*) echo "architectures (PACE, not checked): $f"; continue ;; esac
      have="$(lipo -archs "$f" 2>/dev/null || true)"
      if [ -z "$have" ]; then continue; fi   # not a Mach-O file
      sorted="$(echo "$have" | tr ' ' '\n' | sort | tr '\n' ' ')"
      if [ "$sorted" = "$want" ]; then
        echo "architectures ($have): $f"
      else
        echo "architecture mismatch: $f has ($have), wanted ($ARCHS)"; leaks=1
      fi
    done < <(find "$b" -type f \( -perm -u+x -o -name '*.dylib' \))
  done
fi

note "done: $OUT"
if [ "$leaks" != 0 ]; then exit 1; fi

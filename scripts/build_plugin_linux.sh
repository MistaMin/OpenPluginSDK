#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Builds a plugin written with OpenPluginSDK into Linux plug-ins: a CLAP file,
# an LV2 bundle, a VST3 bundle and a standalone executable with its desktop entry. The macOS packager is scripts/build_plugin.sh; this script
# takes the same option names where they mean the same thing. See
# docs/BUILD.md. Run with --help.
set -euo pipefail

SDK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage: build_plugin_linux.sh [options]

Builds a plugin crate into Linux plug-ins and collects them in one folder.
Every option can also be set by the environment variable in brackets.

  CLAP  OUT/NAME.clap        the shared library itself (one ELF file)
  LV2   OUT/STEM.lv2/        STEM.so, manifest.ttl and STEM.ttl
  VST3  OUT/NAME.vst3/       Contents/ARCH-linux/NAME.so (the shell of
                             native/vst3/linux with the Rust library inside),
                             Contents/Resources/VST3-SDK-LICENSE.txt
  standalone OUT/NAME        the executable (one ELF file)
             OUT/ID.desktop  its freedesktop desktop entry

STEM is NAME reduced to letters, digits and '_'. ID is NAME in lower case
with every character other than letters, digits, '_', '.' and '-' as '-'.

Plugin
  --manifest-path FILE   Cargo.toml of the plugin crate               [OP_MANIFEST]
  --package NAME         cargo package to build (default: from manifest) [OP_PACKAGE]
  --lib-name NAME        library file stem, if not the package name with
                         '-' as '_'                                    [OP_LIB_NAME]
  --name NAME            product name: file names (default: package)   [OP_NAME]
  --version X.Y.Z        version, printed in the log (default: from the
                         manifest; a host reads the plugin's own
                         descriptor, not this)                         [OP_VERSION]
  --ttl-bin NAME         the plugin's cargo bin that writes the LV2 files;
                         called as `NAME <folder> <stem>`              [OP_TTL_BIN]
  --standalone-bin NAME  the plugin's cargo bin that runs standalone   [OP_STANDALONE_BIN]
  --features LIST        cargo features for the plugin crate           [OP_FEATURES]

Formats and options
  --formats LIST         comma list of clap,lv2,vst3,standalone, or
                         "all" (clap,lv2,standalone; default: clap,lv2).
                         au and aax are not built on Linux             [OP_FORMATS]
  --vst3-sdk DIR         VST3 SDK checkout, for vst3                   [VST3_SDK]
  --manufacturer NAME    the vendor, for the VST3 shell                [OP_MANUFACTURER]
  --editor               the VST3 shell hosts the plugin's editor (an
                         X11 view run by the host's loop)              [OP_EDITOR=1]
  --tail                 the VST3 shell reports the plugin's tail      [OP_TAIL=1]
  --strip                strip the symbol table from the libraries
                         (the dynamic exports stay)                    [OP_STRIP=1]
  --allow-lib SONAME     a shared library the plug-in may need besides
                         libc, libm, libgcc_s and the loader; repeatable
                         (comma list in the variable)                  [OP_ALLOW_LIBS]

Paths
  --out DIR              folder that receives the plug-ins (default: ./dist) [OP_OUT]
  --work-dir DIR         build folder for cargo
                         (default: ./build/openplugin-linux)           [OP_WORK_DIR]
  --jobs N               parallel jobs for cargo                       [OP_JOBS]

Other
  --dry-run              print every command, run none, create nothing
  --no-verify            skip the checks at the end (build paths, linked
                         libraries, exported entry points, RPATH)
  -h, --help             this text
USAGE
}

# ---------- settings ----------
MANIFEST="${OP_MANIFEST:-}"; PACKAGE="${OP_PACKAGE:-}"; LIB_NAME="${OP_LIB_NAME:-}"
NAME="${OP_NAME:-}"; VERSION="${OP_VERSION:-}"
TTL_BIN="${OP_TTL_BIN:-}"; STANDALONE_BIN="${OP_STANDALONE_BIN:-}"; FEATURES="${OP_FEATURES:-}"
FORMATS="${OP_FORMATS:-clap,lv2}"
STRIP="${OP_STRIP:-0}"; ALLOW_LIBS="${OP_ALLOW_LIBS:-}"
VST3_SDK="${VST3_SDK:-}"; MANUFACTURER="${OP_MANUFACTURER:-OpenPlugin}"
EDITOR="${OP_EDITOR:-0}"; TAIL="${OP_TAIL:-0}"
OUT="${OP_OUT:-$PWD/dist}"; WORK="${OP_WORK_DIR:-$PWD/build/openplugin-linux}"
JOBS="${OP_JOBS:-$(nproc 2>/dev/null || echo 2)}"
DRY=0; VERIFY=1

die() { echo "build_plugin_linux: $*" >&2; exit 1; }
need() { [ $# -ge 2 ] || die "$1 needs a value"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest-path) need "$@"; MANIFEST="$2"; shift 2 ;;
    --package) need "$@"; PACKAGE="$2"; shift 2 ;;
    --lib-name) need "$@"; LIB_NAME="$2"; shift 2 ;;
    --name) need "$@"; NAME="$2"; shift 2 ;;
    --version) need "$@"; VERSION="$2"; shift 2 ;;
    --ttl-bin) need "$@"; TTL_BIN="$2"; shift 2 ;;
    --standalone-bin) need "$@"; STANDALONE_BIN="$2"; shift 2 ;;
    --features) need "$@"; FEATURES="$2"; shift 2 ;;
    --formats) need "$@"; FORMATS="$2"; shift 2 ;;
    --strip) STRIP=1; shift ;;
    --vst3-sdk) need "$@"; VST3_SDK="$2"; shift 2 ;;
    --manufacturer) need "$@"; MANUFACTURER="$2"; shift 2 ;;
    --editor) EDITOR=1; shift ;;
    --tail) TAIL=1; shift ;;
    --allow-lib) need "$@"; ALLOW_LIBS="${ALLOW_LIBS:+$ALLOW_LIBS,}$2"; shift 2 ;;
    --out) need "$@"; OUT="$2"; shift 2 ;;
    --work-dir) need "$@"; WORK="$2"; shift 2 ;;
    --jobs) need "$@"; JOBS="$2"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    --no-verify) VERIFY=0; shift ;;
    -h|--help) usage; exit 0 ;;
    --bundle-id|--au-subtype|--au-manufacturer|--archs|--deployment-target|--au-sdk|--aax-sdk)
      die "$1 is a macOS option of scripts/build_plugin.sh; it has no meaning for the Linux formats" ;;
    --no-editor|--no-tail|--aax-pages|--pkg)
      die "$1 is a macOS option of scripts/build_plugin.sh; it has no meaning for the Linux formats" ;;
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
abspath() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$PWD" "$1" ;; esac; }
has_format() { case ",$FORMATS," in *",$1,"*) return 0 ;; esac; return 1; }

# ---------- validate ----------
[ "$(uname -s)" = Linux ] || die "this script builds Linux plug-ins on Linux; on macOS use scripts/build_plugin.sh"
[ -n "$MANIFEST" ] || die "--manifest-path is required"
[ -f "$MANIFEST" ] || die "$MANIFEST not found"
MANIFEST="$(abspath "$MANIFEST")"
if [ "$FORMATS" = all ]; then FORMATS="clap,lv2,standalone"; fi
for f in $(echo "$FORMATS" | tr ',' ' '); do
  case "$f" in
    clap|lv2|vst3|standalone) ;;
    au|aax) die "format $f is not built on Linux (Linux builds clap, lv2, vst3 and standalone); use scripts/build_plugin.sh on macOS for it" ;;
    *) die "unknown format $f" ;;
  esac
done
manifest_field() { sed -n "s/^$1 *= *\"\\(.*\\)\".*/\\1/p" "$MANIFEST" | head -1; }
if [ -z "$PACKAGE" ]; then PACKAGE="$(manifest_field name)"; fi
[ -n "$PACKAGE" ] || die "package name not found in the manifest; give --package"
if [ -z "$LIB_NAME" ]; then LIB_NAME="$(echo "$PACKAGE" | tr '-' '_')"; fi
if [ -z "$NAME" ]; then NAME="$PACKAGE"; fi
case "$NAME" in */*) die "--name must not contain '/'" ;; esac
if [ -z "$VERSION" ]; then
  VERSION="$(manifest_field version)"
  [ -n "$VERSION" ] || die "version not found in the manifest (workspace-inherited?); give --version"
fi
VERSION="${VERSION%%-*}"   # numbers only, as on macOS
if has_format lv2 && [ -z "$TTL_BIN" ]; then die "lv2 needs --ttl-bin, the plugin's bin that writes the description files"; fi
if has_format standalone && [ -z "$STANDALONE_BIN" ]; then die "standalone needs --standalone-bin, the plugin's bin that runs standalone"; fi
case "$STANDALONE_BIN" in */*) die "--standalone-bin must not contain '/'" ;; esac
if has_format vst3; then
  [ -n "$VST3_SDK" ] || die "vst3 needs --vst3-sdk (or VST3_SDK), a VST3 SDK checkout"
  [ -f "$VST3_SDK/public.sdk/source/main/linuxmain.cpp" ] || die "--vst3-sdk $VST3_SDK is not a VST3 SDK checkout"
  VST3_SDK="$(abspath "$VST3_SDK")"
  if [ "$DRY" = 0 ]; then command -v cmake >/dev/null || die "vst3 needs cmake and a C++ compiler"; fi
fi
NEED_LIB=0
if has_format clap || has_format lv2 || has_format vst3; then NEED_LIB=1; fi
for t in strings nm readelf ldd file; do
  if [ "$VERIFY" = 1 ] && [ "$DRY" = 0 ]; then command -v "$t" >/dev/null || die "$t is needed for the checks (binutils, glibc); or give --no-verify"; fi
done
if [ "$STRIP" = 1 ] && [ "$DRY" = 0 ]; then command -v strip >/dev/null || die "--strip needs strip (binutils)"; fi

OUT="$(abspath "$OUT")"; WORK="$(abspath "$WORK")"
TARGET_DIR="$WORK/target"
REL="$TARGET_DIR/release"
STEM="$(echo "$NAME" | tr -cd 'A-Za-z0-9_')"   # file stem safe inside an LV2 IRI
if [ -z "$STEM" ]; then STEM=plugin; fi
# Desktop file ID: the characters the Desktop Entry spec allows in one.
DESKTOP_ID="$(printf '%s' "$NAME" | tr 'A-Z' 'a-z' | tr -c 'a-z0-9_.-' '-')"
case "$DESKTOP_ID" in ''|[0-9]*|.*|-*) DESKTOP_ID="app-$DESKTOP_ID" ;; esac

# ---------- environment for the build ----------
export CARGO_BUILD_JOBS="$JOBS"
export CARGO_TARGET_DIR="$TARGET_DIR"
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
RF=""
for r in "${REMAPS[@]}"; do RF="${RF:+$RF$SEP}--remap-path-prefix=$r"; done
export CARGO_ENCODED_RUSTFLAGS="$RF"

CARGO_FEATURES=()
if [ -n "$FEATURES" ]; then CARGO_FEATURES=(--features "$FEATURES"); fi

note "plugin $NAME ($PACKAGE) $VERSION, formats: $FORMATS, $(uname -m) Linux"
if [ "$DRY" = 1 ]; then note "dry run: nothing below is executed"; fi
run mkdir -p "$WORK" "$OUT"

# ---------- Rust ----------
cargo_build() {
  run cargo build --release --manifest-path "$MANIFEST" -p "$PACKAGE" ${CARGO_FEATURES[@]+"${CARGO_FEATURES[@]}"} "$@"
}
SO="$REL/lib$LIB_NAME.so"
if [ "$NEED_LIB" = 1 ]; then
  note "Rust library"
  cargo_build --lib
fi
if has_format lv2; then cargo_build --bin "$TTL_BIN"; fi
if has_format standalone; then
  note "Rust standalone"
  cargo_build --bin "$STANDALONE_BIN"
fi
if [ "$NEED_LIB" = 1 ] && [ "$DRY" = 0 ] && [ ! -f "$SO" ]; then die "$SO was not built: is the crate a cdylib, and is --lib-name right?"; fi
STATIC="$REL/lib$LIB_NAME.a"
if has_format vst3 && [ "$DRY" = 0 ] && [ ! -f "$STATIC" ]; then die "$STATIC was not built: vst3 needs \"staticlib\" in the crate's crate-type"; fi

# ---------- plug-ins ----------
LIBS=()     # every shared library shipped, for the checks
KINDS=()    # its format, in the same order
place() { # destination
  run cp "$SO" "$1"
  if [ "$STRIP" = 1 ]; then run strip --strip-all "$1"; fi
}

if has_format clap; then
  note "CLAP"
  B="$OUT/$NAME.clap"
  run rm -rf "$B"
  place "$B"
  LIBS+=("$B"); KINDS+=(clap)
fi

if has_format lv2; then
  note "LV2"
  B="$OUT/$STEM.lv2"
  run rm -rf "$B"
  run mkdir -p "$B"
  place "$B/$STEM.so"
  run "$REL/$TTL_BIN" "$B" "$STEM"
  LIBS+=("$B/$STEM.so"); KINDS+=(lv2)
fi

if has_format vst3; then
  note "VST3"
  NATIVE="$WORK/native"
  onoff() { if [ "$1" = 1 ]; then echo ON; else echo OFF; fi; }
  run cmake -S "$SDK_ROOT/native" -B "$NATIVE" -DCMAKE_BUILD_TYPE=Release \
    "-DOP_RUST_LIB=$STATIC" "-DOP_VST3_SDK=$VST3_SDK" "-DOP_PRODUCT=$NAME" "-DOP_VERSION=$VERSION" \
    "-DOP_MANUFACTURER=$MANUFACTURER" "-DOP_EDITOR=$(onoff "$EDITOR")" "-DOP_TAIL=$(onoff "$TAIL")" \
    "-DOP_TRIM_PATHS=$WORK;$WS_PARENT;$SDK_ROOT;$VST3_SDK;$HOME"
  run cmake --build "$NATIVE" --target OpenPluginVST3 -j "$JOBS"
  B="$OUT/$NAME.vst3"
  run rm -rf "$B"
  run cp -R "$NATIVE/$NAME.vst3" "$B"
  case "$(uname -m)" in aarch64|arm64) VST3_ARCH=aarch64-linux ;; *) VST3_ARCH=x86_64-linux ;; esac
  M="$B/Contents/$VST3_ARCH/$NAME.so"
  if [ "$STRIP" = 1 ]; then run strip --strip-all "$M"; fi
  LIBS+=("$M"); KINDS+=(vst3)
fi

# A Desktop Entry string: backslash, newline, tab and carriage return escaped.
desktop_string() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/\t/\\t/g' -e 's/\r/\\r/g' | sed -e ':a;N;$!ba;s/\n/\\n/g'; }
# An Exec argument: double-quoted, with ", `, $ and \ backslash-escaped, then
# escaped again as a string; '%' is doubled (field codes).
desktop_exec() { desktop_string "\"$(printf '%s' "$1" | sed -e 's/[\\"`$]/\\&/g' -e 's/%/%%/g')\""; }
if has_format standalone; then
  note "standalone"
  B="$OUT/$NAME"
  D="$OUT/$DESKTOP_ID.desktop"
  run rm -rf "$B"
  run cp "$REL/$STANDALONE_BIN" "$B"
  if [ "$STRIP" = 1 ]; then run strip --strip-all "$B"; fi
  run chmod 755 "$B"
  # Exec names the executable as installed on PATH; its folder is not known here.
  ENTRY="[Desktop Entry]
Type=Application
Version=1.5
Name=$(desktop_string "$NAME")
Exec=$(desktop_exec "$NAME")
Terminal=false
Categories=AudioVideo;Audio;
StartupWMClass=OpenPlugin"
  printf '+ write %q\n' "$D"
  if [ "$DRY" = 0 ]; then printf '%s\n' "$ENTRY" > "$D"; fi
  LIBS+=("$B"); KINDS+=(standalone)
fi

# ---------- checks ----------
bad=0
if [ "$VERIFY" = 1 ] && [ "$DRY" = 0 ]; then
  note "checks"
  # glibc's own libraries (libdl, libpthread and librt are part of libc since
  # glibc 2.34, but older toolchains still link them by name), the GCC unwinder
  # Rust uses, and the loader.
  expected=" libc.so.6 libm.so.6 libgcc_s.so.1 libdl.so.2 libpthread.so.0 librt.so.1 ld-linux-x86-64.so.2 ld-linux-aarch64.so.1 ld-linux-armhf.so.3 ld-linux-riscv64-lp64d.so.1 "
  for a in $(echo "$ALLOW_LIBS" | tr ',' ' '); do expected="$expected$a "; done
  for i in "${!LIBS[@]}"; do
    f="${LIBS[$i]}"; kind="${KINDS[$i]}"
    echo "-- $f"
    file -b "$f"
    if [ "$kind" = standalone ]; then
      if ! file -b "$f" | grep -q 'ELF .* executable'; then echo "not an ELF executable: $f"; bad=1; fi
      if [ ! -x "$f" ]; then echo "not executable: $f"; bad=1; fi
    elif ! file -b "$f" | grep -q 'ELF .* shared object'; then echo "not an ELF shared object: $f"; bad=1; fi
    # No build path in a shipped binary.
    for p in "$WORK" "$WS_PARENT" "$MANIFEST_DIR" "$SDK_ROOT" "$HOME" ${VST3_SDK:+"$VST3_SDK"}; do
      if strings -a "$f" | grep -qF "$p"; then echo "path leak: $f contains $p"; bad=1; fi
    done
    # Linked libraries: only the expected system ones, and all of them found.
    while IFS= read -r lib; do
      case "$expected" in
        *" $lib "*) echo "needs $lib" ;;
        *) echo "unexpected library: $f needs $lib (allow it with --allow-lib $lib)"; bad=1 ;;
      esac
    done < <(readelf -d "$f" | sed -n 's/.*(NEEDED).*\[\(.*\)\].*/\1/p')
    if ldd "$f" | grep -q 'not found'; then ldd "$f" | grep 'not found'; echo "missing library for $f"; bad=1; fi
    # No RPATH or RUNPATH into the build tree. $ORIGIN-relative ones are fine.
    while IFS= read -r rp; do
      echo "run path: $rp"
      for p in "$WORK" "$WS_PARENT" "$WS_ROOT" "$HOME"; do
        case "$rp" in *"$p"*) echo "run path into the build tree: $f has $rp"; bad=1 ;; esac
      done
    done < <(readelf -d "$f" | sed -n 's/.*(\(RPATH\|RUNPATH\)).*\[\(.*\)\].*/\2/p')
    # The format's entry point is exported (an executable has none).
    case "$kind" in clap) syms=clap_entry ;; lv2) syms=lv2_descriptor ;; vst3) syms="GetPluginFactory ModuleEntry ModuleExit" ;; standalone) continue ;; esac
    for sym in $syms; do
      if nm -D --defined-only "$f" | awk '{print $NF}' | grep -qx "$sym"; then
        echo "exports $sym"
      else
        echo "missing export: $f does not define $sym"; bad=1
      fi
    done
    # The VST3 module keeps the Rust library to itself (native/vst3/linux/exports.map):
    # two plug-ins of this SDK in one host must not bind to each other's op_shell_*.
    if [ "$kind" = vst3 ]; then
      extra="$(nm -D --defined-only "$f" | awk '{print $NF}' | grep -vxE 'GetPluginFactory|ModuleEntry|ModuleExit' | grep -vE '^(_init|_fini)$' || true)"
      if [ -n "$extra" ]; then echo "unexpected exports in $f: $(echo "$extra" | head -5 | tr '\n' ' ')"; bad=1; else echo "exports nothing else"; fi
    fi
  done
  if has_format lv2; then
    B="$OUT/$STEM.lv2"
    for t in manifest.ttl "$STEM.ttl"; do
      if [ -s "$B/$t" ]; then echo "lv2 file: $t"; else echo "missing or empty: $B/$t"; bad=1; fi
    done
    if ! grep -qF "lv2:binary <$STEM.so>" "$B/manifest.ttl"; then echo "manifest.ttl does not name $STEM.so as lv2:binary"; bad=1; fi
  fi
  if has_format standalone; then
    D="$OUT/$DESKTOP_ID.desktop"
    if [ -s "$D" ]; then echo "desktop entry: $DESKTOP_ID.desktop"; else echo "missing or empty: $D"; bad=1; fi
    # desktop-file-utils is optional; without it the entry is not validated.
    if command -v desktop-file-validate >/dev/null; then
      if desktop-file-validate "$D"; then echo "desktop-file-validate: no errors"; else echo "desktop-file-validate failed on $D"; bad=1; fi
    else
      echo "desktop-file-validate not found: desktop entry not validated"
    fi
  fi
  if [ "$bad" = 0 ]; then echo "all checks passed"; fi
fi

note "done: $OUT"
if [ "$bad" != 0 ]; then exit 1; fi

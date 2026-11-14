#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Run Avid's AAX Validator (DigiShell "aaxval" dish) against the example AAX
# bundle, locally. The validator and the AAX SDK are not redistributable, so
# this cannot run in public CI. See docs/AAX_VALIDATOR.md.
#
# Usage:
#   AAX_VALIDATOR_DIR=/path/to/aax-validator-dsh-... \
#   AAX_SDK=/path/to/aax-sdk-2-8-1 \
#   scripts/aax_validate.sh [--bundle /path/to/X.aaxplugin] [--out DIR]
#
# Without --bundle the script builds examples/editor as a staticlib and the AAX
# shell (AAX only, with page tables) under OUT. Nothing is installed, signed or
# run with sudo; the bundle is validated in place, by path.
#
# Environment:
#   AAX_VALIDATOR_DIR      folder holding CommandLineTools/dsh (required)
#   AAX_SDK                AAX SDK checkout (required unless --bundle is given)
#   AAX_VALIDATOR_OUT      build and log folder (default: build-aax-validator in the repo)
#   AAX_VALIDATOR_TESTS    space separated test ids to run (default: every test)
#   OP_EDITOR              ON (default) or OFF, whether the shell hosts the editor
#   AAX_VALIDATOR_DSP      1 to count test.cycle_counts (needs HDX); default skips it
#   AAX_VALIDATOR_TIMEOUT  seconds for the whole dsh run (default 1800)
set -u

root=$(cd "$(dirname "$0")/.." && pwd)
bundle=""
out="${AAX_VALIDATOR_OUT:-$root/build-aax-validator}"

while [ $# -gt 0 ]; do
  case "$1" in
    --bundle) bundle=${2:-}; shift 2 ;;
    --out) out=${2:-}; shift 2 ;;
    -h|--help) sed -n 2,26p "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ "$(uname -s)" != Darwin ]; then
  echo "aax_validate: macOS only" >&2; exit 2
fi

dsh="${AAX_VALIDATOR_DIR:-}/CommandLineTools/dsh"
if [ -z "${AAX_VALIDATOR_DIR:-}" ] || [ ! -x "$dsh" ]; then
  cat >&2 <<MSG
aax_validate: AAX Validator not found.
Set AAX_VALIDATOR_DIR to the unpacked "AAX Validator / DigiShell" folder, the
one that contains CommandLineTools/dsh. For example:
  export AAX_VALIDATOR_DIR="/path/to/aax-validator-dsh-2024-6-0-138bab0d-mac-arm64"
It comes from the Avid developer site and is not redistributable.
MSG
  exit 2
fi

case "$AAX_VALIDATOR_DIR" in
  *[[:space:]]*)
    cat >&2 <<MSG
aax_validate: AAX_VALIDATOR_DIR contains a space: $AAX_VALIDATOR_DIR
dsh starts each test in a child process and passes it the validator's path
unquoted, so every test aborts ("ruby: Is a directory -- /Volumes"). Copy or
move the validator to a path without spaces.
MSG
    exit 2 ;;
esac

mkdir -p "$out" || exit 2
out=$(cd "$out" && pwd)
log="$out/aax_validate.log"
: > "$log"

if [ -z "$bundle" ]; then
  if [ -z "${AAX_SDK:-}" ] || [ ! -f "$AAX_SDK/Interfaces/AAX.h" ]; then
    echo "aax_validate: set AAX_SDK to the AAX SDK checkout (needs Interfaces/AAX.h), or pass --bundle" >&2
    exit 2
  fi
  echo "building example-editor and the AAX shell under $out" | tee -a "$log"
  export CARGO_TARGET_DIR="$out/cargo"
  ( cd "$root" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build --release --lib -p example-editor ) >>"$log" 2>&1 \
    || { echo "aax_validate: cargo build failed, see $log" >&2; exit 2; }
  cmake -S "$root/native" -B "$out/cmake" -DCMAKE_BUILD_TYPE=Release \
    -DOP_RUST_LIB="$out/cargo/release/libexample_editor.a" \
    -DOP_AAX_SDK="$AAX_SDK" -DOP_AAX_PAGES=ON \
    -DOP_BUILD_VST3=OFF -DOP_BUILD_AU=OFF -DOP_BUILD_AAX=ON \
    -DOP_EDITOR="${OP_EDITOR:-ON}" \
    -DOP_PRODUCT="Example Gain" -DOP_BUNDLE_ID=com.opengrid.example \
    -DOP_VERSION=0.3.1 -DOP_MANUFACTURER=OpenGrid >>"$log" 2>&1 \
    || { echo "aax_validate: cmake configure failed, see $log" >&2; exit 2; }
  cmake --build "$out/cmake" --target OpenPluginAAX -j 2 >>"$log" 2>&1 \
    || { echo "aax_validate: cmake build failed, see $log" >&2; exit 2; }
  bundle=$(find "$out/cmake" -maxdepth 2 -name '*.aaxplugin' -type d | head -n 1)
fi

case "$bundle" in /*) ;; *) bundle="$PWD/$bundle" ;; esac
if [ ! -d "$bundle" ] || [ "${bundle%.aaxplugin}" = "$bundle" ]; then
  echo "aax_validate: not an .aaxplugin bundle: $bundle" >&2; exit 2
fi
case "$bundle" in *\"*) echo "aax_validate: bundle path must not contain a double quote" >&2; exit 2 ;; esac
echo "bundle: $bundle" | tee -a "$log"

# dsh reads commands from stdin. The path is quoted; dsh has no escaping.
cmds="$out/aax_validate.dsh"
{
  echo "load_dish aaxval"
  if [ -n "${AAX_VALIDATOR_TESTS:-}" ]; then
    for t in $AAX_VALIDATOR_TESTS; do
      echo "runtest {test: $t, path: \"$bundle\", stringformat: yaml}"
    done
  else
    echo "runtests {path: \"$bundle\", stringformat: yaml}"
  fi
  echo "quit"
} > "$cmds"

raw="$out/aax_validate.raw.log"
timeout="${AAX_VALIDATOR_TIMEOUT:-1800}"
echo "running dsh (timeout ${timeout}s), raw output in $raw" | tee -a "$log"
"$dsh" < "$cmds" > "$raw" 2>&1 &
pid=$!
( sleep "$timeout"; kill "$pid" 2>/dev/null ) &
killer=$!
wait "$pid"; rc=$?
kill "$killer" 2>/dev/null
wait "$killer" 2>/dev/null
cat "$raw" >> "$log"

# Summarize. Each result carries an id and a result_status (E_COMPLETED_PASS,
# E_COMPLETED_FAIL, E_CANCELED, E_LOST, E_TIMEOUT, ...).
summary="$out/aax_validate.summary.txt"
awk '
  /^[[:space:]-]*id:[[:space:]]/ { id=$0; sub(/^[^:]*:[[:space:]]*/, "", id); gsub(/"/, "", id) }
  /result_status:/ { s=$0; sub(/^.*result_status:[[:space:]]*/, "", s); gsub(/"/, "", s); print id "\t" s }
' "$raw" > "$summary"

# test.cycle_counts measures DSP plug-ins: it loads the DAE dish and acquires an
# HDX deck, so on a machine without HDX it ends E_LOST for any native plug-in.
# It counts as skipped unless AAX_VALIDATOR_DSP=1 says the plug-in has a DSP build.
if [ "${AAX_VALIDATOR_DSP:-0}" != 1 ]; then
  sed -i '' -E 's/^(test\.cycle_counts)	(E_LOST|E_ABORTED|E_CANCELED)$/\1	SKIPPED_NEEDS_DSP_HARDWARE/' "$summary"
fi
pass=$(grep -c 'E_COMPLETED_PASS$' "$summary")
fail=$(grep -cE 'E_COMPLETED_FAIL$|E_LOST$|E_TIMEOUT$|E_ABORTED$|E_ERROR$' "$summary")
total=$(wc -l < "$summary" | tr -d ' ')
skip=$((total - pass - fail))
echo
echo "AAX Validator: $total results, $pass passed, $fail failed, $skip skipped"
awk -F'\t' '{ printf "  %-45s %s\n", $1, $2 }' "$summary"
echo "full log: $log"

if [ "$total" -eq 0 ]; then
  echo "aax_validate: no test results found (dsh exit $rc); treating as failure" >&2
  exit 1
fi
[ "$fail" -eq 0 ] || exit 1
exit 0

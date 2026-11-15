#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Format parity (docs/FORMAT_PARITY.md): one input signal and one parameter
# automation, rendered by the Rust plugin directly and through the built CLAP,
# LV2, VST3, Audio Unit and AAX entries of examples/editor. Every output must
# match the reference sample for sample within TOLERANCE. The VST3, Audio Unit
# and AAX negative controls (shells that deliberately add a gain error) and a
# timing control (the automation one frame late) must FAIL the same
# comparison, or the run fails.
#
# Usage:
#   VST3_SDK=/path/to/vst3sdk AU_SDK=/path/to/AudioUnitSDK \
#   [AAX_SDK=/path/to/aax-sdk] scripts/format_parity/run.sh [options]
#
# Options:
#   --work DIR        build and output folder (default: build-format-parity in the repo)
#   --tolerance T     maximum absolute sample difference (default 1e-6)
#   --skip-build      reuse what --work already holds
#   --jobs N          parallel jobs for cmake (default 2)
#   -h, --help
#
# A format whose SDK variable is unset is skipped and listed as skipped. CLAP
# and LV2 need only Rust. AAX runs only when AAX_SDK is set: its SDK is not
# redistributable, so CI never has it. Nothing is installed anywhere: bundles
# are loaded from --work by path, and the Audio Unit is registered in-process.
# The script deletes nothing but the output files it wrote under --work/out.
set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
work="$root/build-format-parity"
tolerance=1e-6
build=1
jobs=2

while [ $# -gt 0 ]; do
  case "$1" in
    --work) work=${2:-}; shift 2 ;;
    --tolerance) tolerance=${2:-}; shift 2 ;;
    --skip-build) build=0; shift ;;
    --jobs) jobs=${2:-}; shift 2 ;;
    -h|--help) sed -n 2,24p "$0"; exit 0 ;;
    *) echo "format_parity: unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ "$(uname -s)" != Darwin ]; then
  echo "format_parity: macOS only" >&2; exit 2
fi
case "$work" in
  /?*) ;;
  *) echo "format_parity: --work must be an absolute path, got: '$work'" >&2; exit 2 ;;
esac
if [ "$work" = / ]; then
  echo "format_parity: --work must not be /" >&2; exit 2
fi

vst3_sdk=${VST3_SDK:-}
au_sdk=${AU_SDK:-}
aax_sdk=${AAX_SDK:-}
have_vst3=0; have_au=0; have_aax=0
[ -n "$vst3_sdk" ] && [ -f "$vst3_sdk/public.sdk/source/main/macmain.cpp" ] && have_vst3=1
[ -n "$au_sdk" ] && [ -f "$au_sdk/include/AudioUnitSDK/AUEffectBase.h" ] && have_au=1
[ -n "$aax_sdk" ] && [ -f "$aax_sdk/Interfaces/AAX.h" ] && have_aax=1
[ -n "$vst3_sdk" ] && [ "$have_vst3" = 0 ] && { echo "format_parity: VST3_SDK is not a VST3 SDK checkout: $vst3_sdk" >&2; exit 2; }
[ -n "$au_sdk" ] && [ "$have_au" = 0 ] && { echo "format_parity: AU_SDK is not an AudioUnitSDK checkout: $au_sdk" >&2; exit 2; }
[ -n "$aax_sdk" ] && [ "$have_aax" = 0 ] && { echo "format_parity: AAX_SDK is not an AAX SDK: $aax_sdk" >&2; exit 2; }

target="$work/target"
host_target="$work/target-host"
cmake_dir="$work/cmake"
out="$work/out"
bundles="$work/bundles"
mkdir -p "$work" "$out" "$bundles" || exit 2

# ---- build ------------------------------------------------------------------
if [ "$build" = 1 ]; then
  echo "== building the Rust plugin and the Rust host"
  (cd "$root" && CARGO_TARGET_DIR="$target" cargo build --release -p example-editor) || exit 2
  CARGO_TARGET_DIR="$host_target" cargo build --release --manifest-path "$here/host/Cargo.toml" || exit 2

  if [ "$have_vst3" = 1 ] || [ "$have_au" = 1 ] || [ "$have_aax" = 1 ]; then
    echo "== configuring and building the native shells and hosts"
    on_off() { if [ "$1" = 1 ]; then echo ON; else echo OFF; fi; }
    cmake -S "$root/native" -B "$cmake_dir" -DCMAKE_BUILD_TYPE=Release \
      -DOP_RUST_LIB="$target/release/libexample_editor.a" \
      -DOP_BUILD_VST3="$(on_off $have_vst3)" -DOP_BUILD_AU="$(on_off $have_au)" -DOP_BUILD_AAX="$(on_off $have_aax)" \
      -DOP_VST3_SDK="$vst3_sdk" -DOP_AU_SDK="$au_sdk" -DOP_AAX_SDK="$aax_sdk" \
      -DOP_PRODUCT="Example Gain" -DOP_BUNDLE_ID=com.opengrid.example \
      -DOP_VERSION=0.3.1 -DOP_MANUFACTURER=OpenGrid \
      -DOP_AU_SUBTYPE=ExGn -DOP_AU_MANUFACTURER=OpGd \
      -DOP_TAIL=ON -DOP_EDITOR=OFF -DOP_FORMAT_PARITY=ON || exit 2
    targets=()
    [ "$have_vst3" = 1 ] && targets+=(OpenPluginVST3 OpenPluginVST3Fault ParityVST3)
    [ "$have_au" = 1 ] && targets+=(OpenPluginAU OpenPluginAUFault ParityAU)
    [ "$have_aax" = 1 ] && targets+=(ParityAAX ParityAAXFault)
    cmake --build "$cmake_dir" -j "$jobs" --target "${targets[@]}" || exit 2
  fi
fi

host="$host_target/release/parity-host"
[ -x "$host" ] || { echo "format_parity: $host not built" >&2; exit 2; }

# CLAP and LV2 bundles: the cdylib, laid out the way build_plugin.sh lays them out.
mkdir -p "$bundles/Editor Gain.clap/Contents/MacOS" "$bundles/EditorGain.lv2" || exit 2
cp "$target/release/libexample_editor.dylib" "$bundles/Editor Gain.clap/Contents/MacOS/Editor Gain" || exit 2
cp "$target/release/libexample_editor.dylib" "$bundles/EditorGain.lv2/EditorGain.dylib" || exit 2

# ---- run --------------------------------------------------------------------
# Only files this script wrote, only inside "$out".
clean_out() {
  case "$out" in "$work"/?*) ;; *) echo "format_parity: refusing to clean '$out'" >&2; exit 2 ;; esac
  for name in scenario.bin reference.f32 clap.f32 lv2.f32 vst3.f32 au.f32 aax.f32 vst3-fault.f32 au-fault.f32 aax-fault.f32 reference-late.f32; do
    rm -f -- "$out/$name"
  done
}
clean_out

failed=0
summary=()

"$host" gen "$out/scenario.bin" || exit 2
"$host" reference "$out/scenario.bin" "$out/reference.f32" || exit 2

# parity LABEL OUTPUT-FILE COMMAND...: run the host, then compare to the reference.
parity() {
  local label=$1 file=$2; shift 2
  if ! "$@"; then
    echo "$label: host failed" >&2; failed=1; summary+=("$label: ERROR"); return
  fi
  local line
  line=$("$host" compare "$out/reference.f32" "$file" "$tolerance" "$label"); local code=$?
  echo "$line"
  if [ $code -eq 0 ]; then summary+=("$label: ok"); else failed=1; summary+=("$label: FAIL"); fi
}

# control LABEL OUTPUT-FILE COMMAND...: the comparison must report a mismatch (exit 1).
control() {
  local label=$1 file=$2; shift 2
  if ! "$@"; then
    echo "$label: host failed" >&2; failed=1; summary+=("$label control: ERROR"); return
  fi
  local line
  line=$("$host" compare "$out/reference.f32" "$file" "$tolerance" "$label"); local code=$?
  echo "$line"
  if [ $code -eq 1 ]; then
    summary+=("$label control: fails as it must")
  else
    echo "$label: the negative control was NOT caught (compare exit $code)" >&2
    failed=1; summary+=("$label control: NOT CAUGHT")
  fi
}

echo "== parity, tolerance $tolerance"
parity clap "$out/clap.f32" "$host" clap "$out/scenario.bin" "$out/clap.f32" "$bundles/Editor Gain.clap/Contents/MacOS/Editor Gain"
parity lv2 "$out/lv2.f32" "$host" lv2 "$out/scenario.bin" "$out/lv2.f32" "$bundles/EditorGain.lv2/EditorGain.dylib"
if [ "$have_vst3" = 1 ]; then
  parity vst3 "$out/vst3.f32" "$cmake_dir/ParityVST3" "$cmake_dir/Example Gain.vst3" "$out/scenario.bin" "$out/vst3.f32"
else
  summary+=("vst3: skipped (VST3_SDK unset)")
fi
if [ "$have_au" = 1 ]; then
  parity au "$out/au.f32" "$cmake_dir/ParityAU" "$cmake_dir/Example Gain.component" "$out/scenario.bin" "$out/au.f32"
else
  summary+=("au: skipped (AU_SDK unset)")
fi
if [ "$have_aax" = 1 ]; then
  parity aax "$out/aax.f32" "$cmake_dir/ParityAAX" "$out/scenario.bin" "$out/aax.f32"
else
  summary+=("aax: skipped (AAX_SDK unset; local only)")
fi

echo "== negative controls: shells with a deliberate gain error must FAIL"
[ "$have_vst3" = 1 ] && control vst3 "$out/vst3-fault.f32" "$cmake_dir/ParityVST3" "$cmake_dir/Parity Fault.vst3" "$out/scenario.bin" "$out/vst3-fault.f32"
[ "$have_au" = 1 ] && control au "$out/au-fault.f32" "$cmake_dir/ParityAU" "$cmake_dir/ParityFault.component" "$out/scenario.bin" "$out/au-fault.f32"
[ "$have_aax" = 1 ] && control aax "$out/aax-fault.f32" "$cmake_dir/ParityAAXFault" "$out/scenario.bin" "$out/aax-fault.f32"

# The automation must matter: the same reference with every event one frame late
# must differ from the real one by far more than the tolerance. Without this a
# test that ignored the automation could still pass.
echo "== timing control: automation one frame late must FAIL"
control "late-1" "$out/reference-late.f32" "$host" reference "$out/scenario.bin" "$out/reference-late.f32" 1

echo "== summary"
for line in "${summary[@]}"; do echo "  $line"; done
if [ "$failed" = 0 ]; then echo "format parity: PASS"; else echo "format parity: FAIL"; fi
exit "$failed"

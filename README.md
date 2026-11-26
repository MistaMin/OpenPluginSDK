# OpenPluginSDK

> **Universal, zero-allocation digital audio plugin development framework with GoodLookin Design Rust Interface (DRI).**  
> Targets **CLAP**, **LV2**, **VST3**, **Audio Unit (AU)** and **Avid AAX**, with a standalone editor window for development.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![CI](https://github.com/MistaMin/OpenPluginSDK/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/MistaMin/OpenPluginSDK/actions/workflows/ci.yml)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey.svg)](docs/PLATFORMS.md)
[![Version](https://img.shields.io/badge/version-1.0%20Beta-orange.svg)](#)

---

## Summary

**OpenPluginSDK** is an independent, Rust-based audio plugin framework designed to build high-performance, real-time safe audio plugins and virtual instruments that run everywhere.

It combines:
1. **Realtime-safe DSP core:** Zero-allocation processing loop (`process()`), hostile parameter sanitization, and no panics.
2. **Textbook DSP library:** Bundled high-order RBJ biquad filters, overlap-save convolution for impulses up to 1024 taps (`BlockConvolver`), log-domain soft-knee dynamics (compressor/limiter/gate), tape/tube non-linear saturation curves, IEC 60268-17 VU and PPM meters, BS.1770-4 loudness (`Loudness`), loudness-matched auto gain (`LoudnessMatch`), and Delta listening (`Delta`).
3. **GoodLookin Design Rust Interface (DRI) 0.2:** procedural hardware drawing with no bitmaps: 47 knob styles, 40 faceplate finishes, meters, switches and the spectrum. Knobs are lit by default, shaded per pixel from a light rig, and the classic and flat tiers remain. `SceneCanvas` rasterises a scene, with the static plate cached, into the pixels an editor shows. No JUCE dependency or license. See [docs/DRI_GUIDE.md](docs/DRI_GUIDE.md).
4. **Format bridges:** CLAP and LV2 are Rust entries. VST3, Audio Unit, and AAX are native C++ shells over `export_shell!`: VST3 on macOS, Windows and Linux, AAX on macOS and Windows, the Audio Unit on macOS. A standalone build opens the editor in a window of its own, without audio. The shell contract is [docs/SHELL.md](docs/SHELL.md).
5. **One editor in every format:** a plugin writes one `Editor` and passes `editor = make_editor` to the export macros. CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), VST3, Audio Unit and AAX all host it in a Rust view: an `NSView` on macOS, an HWND on Windows, an X11 window on Linux. See [docs/EDITOR.md](docs/EDITOR.md) and [`examples/editor`](examples/editor).

---

## Table of Contents

- [Architecture](#architecture)
- [Crate Hierarchy](#crate-hierarchy)
- [Getting Started](#getting-started)
- [Writing a Plugin](#writing-a-plugin)
  - [1. Define Descriptors](#1-define-descriptors)
  - [2. Implement DSP](#2-implement-dsp)
  - [3. Render with GoodLookin DRI](#3-render-with-goodlookin-dri)
- [Target Format Support](#target-format-support)
  - [CLAP](#clap)
  - [VST3](#vst3)
  - [Audio Unit (macOS)](#audio-unit-macos)
  - [Avid AAX (Pro Tools)](#avid-aax-pro-tools)
  - [Standalone](#standalone)
- [Rust and C++](#rust-and-c)
- [Quality Guarantees](#quality-guarantees)
- [Roadmap](#roadmap)
- [License](#license)

---

## Architecture

```
                               ┌─────────────────────────────────────────┐
                               │           Plugin Implementation         │
                               │   (DSP logic, Parameters, UI Layout)    │
                               └────────────────────┬────────────────────┘
                                                    │
                 ┌──────────────────────────────────┴──────────────────────────────────┐
                 ▼                                                                     ▼
   ┌───────────────────────────┐                                         ┌───────────────────────────┐
   │    openplugin-core        │                                         │      openplugin-dri       │
   │  - Realtime `Plugin` trait│                                         │ (GoodLookin DRI Interface)│
   │  - ParamInfo & Tapers     │                                         │  - 47 Procedural Knobs    │
   │  - Transport & MIDI 1.0   │                                         │  - 40 Faceplates & Finishes│
   │  - Zero-alloc audio pipe  │                                         │  - Ballistic Analog VU    │
   │  - Safe state persistence │                                         │  - Direct2D / Metal / WGPU│
   └─────────────┬─────────────┘                                         └─────────────┬─────────────┘
                 │                                                                     │
                 └───────────────────────────────┬─────────────────────────────────────┘
                                                 │
                                                 ▼
   ┌───────────────────────────────────────────────────────────────────────────────────────────┐
   │                                   Format Target Bridges                                   │
   ├───────────────┬───────────────┬───────────────────────────┬───────────────┬───────────────┤
   │     CLAP      │     VST3      │      AU (Audio Unit)      │      AAX      │  Standalone   │
   │ (Rust entry)  │ (native shell)│      (native shell)       │(native shell) │ (editor only) │
   └───────────────┴───────────────┴───────────────────────────┴───────────────┴───────────────┘
```

---

## Crate Hierarchy

| Crate | Role |
|---|---|
| [`openplugin`](crates/openplugin) | Top-level facade, and a small prelude (the plugin and editor contracts, the descriptors, the export macros) |
| [`openplugin-core`](crates/openplugin-core) | Core `Plugin` trait, `ParamInfo` (format and parse), tapers, transport, MIDI, the `Editor` contract, and text presets (`PresetBank`, `build::embed_files`) |
| [`openplugin-dsp`](crates/openplugin-dsp) | Realtime-safe DSP: RBJ biquads, `BlockConvolver`, dynamics, saturation, VU metering. See [docs/CONVOLVE.md](docs/CONVOLVE.md) |
| [`openplugin-dri`](crates/openplugin-dri) | GoodLookin DRI adapter (knob tiers, faceplates, meters), `SceneCanvas` (scene to pixels, cached plate layer), and the editor views for macOS, Windows and X11. Drawing lives in GoodLookin DRI |
| [`openplugin-targets`](crates/openplugin-targets) | CLAP and LV2 entries with their editors, the LV2 `.ttl` writer, and `export_shell!` with `op_editor_*`. See [docs/SHELL.md](docs/SHELL.md) |
| [`openplugin-host`](crates/openplugin-host) | Rust host session (`HostSession`): parameter plane, scratch render, presets, session state. Also holds the Rust side of the `op_shell_*` ABI |

---

## Getting Started

Add `openplugin` to your `Cargo.toml`:

```toml
[dependencies]
openplugin = "1.0.0-beta"
```

To build a plugin into CLAP, LV2, VST3, Audio Unit and AAX bundles, with optional signing and notarisation, see [docs/BUILD.md](docs/BUILD.md).

---

## Writing a Plugin

### 1. Define Descriptors

Parameters are described statically with ranges, tapers, units, and default values:

```rust
use openplugin::prelude::*;

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db).with_taper(Taper::Linear),
    ParamInfo::new("drive", "Drive", 1.0, 10.0, 1.0, Unit::None).with_taper(Taper::Log),
];

static INFO: PluginInfo = PluginInfo::new(
    "com.vendor.saturator",
    "Console Saturator",
    "AudioCraft",
    "1.0.0",
    Category::Harmonic,
    &PARAMS,
)
.with_url("https://example.com")
.with_email("support@example.com")
.with_editor(true);
```

### 2. Implement DSP

```rust
use openplugin::dsp::{saturate_sample, SaturationMode};

pub struct SaturatorPlugin {
    linear_gain: f32,
    drive: f32,
}

impl Plugin for SaturatorPlugin {
    fn info(&self) -> &'static PluginInfo { &INFO }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {
        self.linear_gain = 1.0;
        self.drive = 1.0;
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                let db = PARAMS[0].clamp(value);
                self.linear_gain = 10.0_f32.powf(db / 20.0);
                true
            }
            "drive" => {
                self.drive = PARAMS[1].clamp(value);
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(20.0 * self.linear_gain.log10()),
            "drive" => Some(self.drive),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for ch in io.iter_mut() {
            for sample in &mut ch[..frames] {
                *sample = saturate_sample(*sample * self.linear_gain, self.drive, SaturationMode::Tape);
            }
        }
    }
}
```

### 3. Render with GoodLookin DRI

Create authentic analogue console faceplates and hardware knobs without bitmap assets:

```rust
use openplugin::dri::{draw_faceplate, draw_knob, draw_vu_meter, Color, DrawList, MeterStyle};

impl SaturatorPlugin {
    pub fn render_gui(&self, dl: &mut DrawList) {
        // 1. Draw vintage British console chassis
        draw_faceplate(dl, 320.0, 180.0, "Navy");

        // 2. Draw fluted Bakelite drive knob
        let drive_norm = PARAMS[1].to_normalized(self.drive);
        draw_knob(dl, 80.0, 80.0, 32.0, drive_norm, "DRIVE", &PARAMS[1].format(self.drive), "Bk", Color::BAKELITE_BROWN);

        // 3. Draw output level knob
        let gain_db = 20.0 * self.linear_gain.log10();
        let gain_norm = PARAMS[0].to_normalized(gain_db);
        draw_knob(dl, 240.0, 80.0, 32.0, gain_norm, "OUTPUT", &PARAMS[0].format(gain_db), "Brit", Color::RED);

        // 4. Draw illuminated analog VU meter
        draw_vu_meter(dl, 130.0, 45.0, 60.0, 45.0, gain_db, MeterStyle::AnalogVu);
    }
}
```

---

## Target Format Support

### CLAP
Modern, open audio plugin standard with high-efficiency event dispatching and direct parameter modulation. `export_clap!(P)` exports the entry. With `export_clap!(P, editor = make_editor)` it also offers `clap.gui` (Cocoa, Win32 or X11). `text_to_value` reads choice names, On/Off and units (`ParamInfo::parse`).

### LV2
`export_lv2!` gives two stereo audio ports each way (an instrument has outputs only) and one control port per parameter, then a `freeWheeling` port (`set_offline`), a MIDI/`time:Position` atom input when the plugin takes notes, a MIDI atom output when it sends them, and an `lv2:isSideChain` input per sidechain channel. Port order is in [docs/FORMATS.md](docs/FORMATS.md#lv2-ports). `editor = make_editor` adds a `ui:CocoaUI`, `ui:WindowsUI` or `ui:X11UI`. `lv2::write_bundle` writes `manifest.ttl` and the plugin `.ttl` from the parameter table. `Plugin::lv2_uri` sets a fixed URI.

### VST3
A thin native factory in `native/vst3` calls the `op_shell_*` ABI from `export_shell!`. It implements the processor and the edit controller. With `-DOP_EDITOR=ON` and `export_shell!(P, editor = make_editor)` it hosts the plugin's editor. See [docs/SHELL.md](docs/SHELL.md).

### Audio Unit (macOS)
A thin `AUEffectBase` adapter in `native/au` calls the same ABI. The proof loads the component in-process. It does not install it.

### Avid AAX (Pro Tools)
An unsigned native adapter in `native/aax` links an external Avid SDK and calls the same ABI. The proof runs the process callback and checks the ACF exports. It is not a Pro Tools session, and the build does not sign or install the bundle. Set fixed IDs with `Plugin::aax_ids` before a release.

### Standalone
The plugin's own binary opens its editor in a window with `run_window` (`openplugin::dri::cocoa`, `win32` or `x11`), and `scripts/build_plugin.sh --standalone-bin` wraps it as an application. It opens no audio device: it is for working on the editor, not for performance.

---

## Rust and C++

A plugin is written in Rust: its DSP, parameters, state and editor. About 85% of this repository is Rust. The C and C++ (about 15%) sits in `native/` and does three jobs:

| What | Share of the C/C++ | Why it is C++ |
|---|---|---|
| Test hosts (`native/*/host*.cpp`, `events_host.cpp`, `editor_host.*`, the AAX proof stub) | about half | They load a built plugin the way a DAW does, through each format's official SDK. Written apart from the Rust they test, they catch bugs a Rust test would share. |
| Format shells (`native/vst3`, `native/au`, `native/aax`, `native/ara`) | about a quarter | Thin adapters: each turns a format's calls into calls over the `op_shell_*` C ABI to the plugin's Rust code. Nothing a plugin does runs in them. |
| Headers and helpers (`native/include`, `examples/cpp_bridge`, format parity) | the rest | The C ABI between the shells and Rust, and an example that hosts existing C++ DSP. |

Why each format is what it is:

- **CLAP and LV2** are C interfaces, so they are pure Rust here, with no C++.
- **AAX** stays C++. Avid's SDK is C++ and not redistributable, and an AAX plugin is built against it, signed with PACE and checked by Avid's tools. There is no supported way to write one in Rust.
- **VST3** uses Steinberg's C++ SDK (MIT since 3.8). The interfaces underneath are COM-style tables that Rust could implement directly; the SDK is used for Steinberg's own handling of the edge cases that pluginval and hosts check.
- **Audio Unit** uses Apple's AudioUnitSDK (Apache 2.0) for its property and parameter rules. AUv2 is a C interface underneath, so it too could move to Rust.

So C++ is required for AAX and chosen for the test hosts; for the VST3 and Audio Unit shells it was the faster way to validated shells, and moving them to Rust is a possible later step.

---

## Quality Guarantees

- **No Panics:** Production code avoids unhandled crashes and enforces safe math.
- **Hostile Input Hardened:** Parameter values and transport commands from hostile DAW hosts are sanitized.
- **Pure Vector Assets:** All controls are procedural vector code with zero external bitmap dependencies.

---

## Roadmap

What comes after 1.0 Beta, from hardening on macOS to Windows, Linux and a stable 1.0: [docs/ROADMAP.md](docs/ROADMAP.md).

Changes release by release: [CHANGELOG.md](CHANGELOG.md).

What counts as a breaking change, and how versions and deprecations work: [docs/VERSIONING.md](docs/VERSIONING.md).

The API review for the 1.0 freeze, every public item with what was decided and done: [docs/API_REVIEW.md](docs/API_REVIEW.md). What is frozen: [docs/VERSIONING.md](docs/VERSIONING.md#the-frozen-surface).

Guides for plugin developers: testing a plugin with the conformance kit and the benchmark harness, [docs/TESTING_PLUGINS.md](docs/TESTING_PLUGINS.md); making it cheap (real-time rules, denormals, vectorising, fast approximations, oversampling cost, with measured before/after examples), [docs/PERFORMANCE.md](docs/PERFORMANCE.md); and keeping it from breaking (stable filters, NaN policy, panics, threads, host behaviours, soak and fuzz, with the test that enforces each rule), [docs/STABILITY.md](docs/STABILITY.md). The measured examples are in [`examples/perf_guide`](examples/perf_guide).

---

## License

OpenPluginSDK is dual-licensed under either:

* [MIT License](LICENSE-MIT) ([http://opensource.org/licenses/MIT](http://opensource.org/licenses/MIT))
* [Apache License, Version 2.0](LICENSE-APACHE) ([http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0))

at your option. This provides maximum flexibility for both commercial/proprietary plugins and open-source audio software.

The fonts in `crates/goodlookindri/shell/fonts` are not under these licences: each keeps its own, the SIL Open Font License 1.1, in its folder. A plugin that uses one ships that licence with it. Fonts installed on a user's computer (`text::system`) are read there and never copied into the plugin. See [docs/DRI_GUIDE.md](docs/DRI_GUIDE.md#fonts).

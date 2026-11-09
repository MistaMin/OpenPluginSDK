# OpenPluginSDK

> **Universal, zero-allocation digital audio plugin development framework with GoodLookin Direct Rendering Interface (DRI).**  
> Targets **CLAP**, **VST3**, **Audio Unit (AU)**, **Avid AAX**, and **Standalone**.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)](#)
[![Version](https://img.shields.io/badge/release-v0.1.0--beta.1-orange.svg)](#)

---

## Summary

**OpenPluginSDK** is an independent, pure-Rust audio plugin framework designed to build high-performance, real-time safe audio plugins and virtual instruments that run everywhere.

It combines:
1. **Realtime-safe DSP core:** Zero-allocation processing loop (`process()`), hostile parameter sanitization, and no panics.
2. **Textbook DSP library:** Bundled high-order RBJ biquad filters, log-domain soft-knee dynamics (compressor/limiter/gate), tape/tube non-linear saturation curves, and ANSI standard ballistic VU meters.
3. **GoodLookin Direct Rendering Interface (DRI):** Procedural 3D hardware rendering (47 authentic knob styles, 40 faceplate finishes, illuminated needle VU meters) running directly on GPU/platform window contexts (`HWND`, `NSView`, `X11`) without requiring a JUCE dependency or license.
4. **Universal format bridges:** Export to **CLAP**, **VST3**, **macOS Audio Unit (AUv2/AUv3)**, **Avid AAX** (for Pro Tools parity using Avid AAX SDK), and native **Standalone** desktop executables.

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
- [Quality Guarantees](#quality-guarantees)
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
   │ (Rust entry)  │ (Rust layout) │      (Rust layout)        │ (Rust layout) │  (CPAL audio) │
   └───────────────┴───────────────┴───────────────────────────┴───────────────┴───────────────┘
```

---

## Crate Hierarchy

| Crate | Role |
|---|---|
| [`openplugin`](crates/openplugin) | Top-level facade and prelude |
| [`openplugin-core`](crates/openplugin-core) | Core `Plugin` trait, `ParamInfo`, tapers, transport, and MIDI |
| [`openplugin-dsp`](crates/openplugin-dsp) | Realtime-safe DSP modules: RBJ biquads, dynamics, saturation, VU metering |
| [`openplugin-dri`](crates/openplugin-dri) | GoodLookin procedural 3D hardware GUI engine (knobs, faceplates, meters) |
| [`openplugin-targets`](crates/openplugin-targets) | Target bridges for CLAP, VST3, AU, AAX, and Standalone |
| [`openplugin-ffi`](crates/openplugin-ffi) | Rust host session: parameter plane, scratch render, presets, OPST snapshot |

---

## Getting Started

Add `openplugin` to your `Cargo.toml`:

```toml
[dependencies]
openplugin = "0.1.0-beta.1"
```

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

static INFO: PluginInfo = PluginInfo {
    id: "com.vendor.saturator",
    name: "Console Saturator",
    vendor: "AudioCraft",
    url: "https://example.com",
    email: "support@example.com",
    version: "1.0.0",
    category: Category::Harmonic,
    params: &PARAMS,
    is_instrument: false,
    has_editor: true,
};
```

### 2. Implement DSP

```rust
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

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
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
impl SaturatorPlugin {
    pub fn render_gui(&self, dl: &mut DrawList) {
        // 1. Draw vintage British console chassis
        draw_faceplate(dl, 320.0, 180.0, FaceplateStyle::BritishConsole);

        // 2. Draw fluted Bakelite drive knob
        let drive_norm = PARAMS[1].to_normalized(self.drive);
        draw_knob(dl, 80.0, 80.0, 32.0, drive_norm, "DRIVE", &PARAMS[1].format(self.drive), KnobStyle::Bakelite, Color::BAKELITE_BROWN);

        // 3. Draw output level knob
        let gain_db = 20.0 * self.linear_gain.log10();
        let gain_norm = PARAMS[0].to_normalized(gain_db);
        draw_knob(dl, 240.0, 80.0, 32.0, gain_norm, "OUTPUT", &PARAMS[0].format(gain_db), KnobStyle::Brit, Color::BRIT_RED);

        // 4. Draw illuminated analog VU meter
        draw_vu_meter(dl, 130.0, 45.0, 60.0, 45.0, gain_db, MeterStyle::AnalogVu);
    }
}
```

---

## Target Format Support

### CLAP
Modern, open audio plugin standard with high-efficiency event dispatching and direct parameter modulation. Exposes standard C ABI via `openplugin_targets::clap`.

### VST3
Steinberg VST3 compatibility via independent COM interface implementations (`IComponent`, `IEditController`, `IPlugView`).

### Audio Unit (macOS)
Direct AUv2 `AudioComponent` wrapper and Cocoa view bridging for native Logic Pro, GarageBand, and macOS DAW compatibility.

### Avid AAX (Pro Tools)
Full Pro Tools compatibility built to link with the official Avid AAX SDK (`AAX_CEffectParameters`, `AAX_IEffectGUI`).

### Standalone
Desktop application executable powered by `cpal` for zero-friction testing and live hardware performance.

---

## Quality Guarantees

- **No Panics:** Production code avoids unhandled crashes and enforces safe math.
- **Hostile Input Hardened:** Parameter values and transport commands from hostile DAW hosts are sanitized.
- **Pure Vector Assets:** All controls are procedural vector code with zero external bitmap dependencies.

---

## License

OpenPluginSDK is dual-licensed under either:

* [MIT License](LICENSE-MIT) ([http://opensource.org/licenses/MIT](http://opensource.org/licenses/MIT))
* [Apache License, Version 2.0](LICENSE-APACHE) ([http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0))

at your option. This provides maximum flexibility for both commercial/proprietary plugins and open-source audio software.

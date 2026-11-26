# Plugin UI additions and parity — 2026-11-14

Reviewed the latest local commits in the OpenGrid plugin repositories. This is a local-source audit; remote branches were not fetched. No plugin product source was changed.

## Source updates

- DualSone `c276917`: sample-timed two-second loudness/crest readouts and caption/switch association. Extracted as reusable readout aggregation and paired layout helpers.
- RadarVU `0dc3413`: readout cards overflowed the panel with a fixed vertical step. Extracted as bounded equal-height card layout.
- HarmonicCompressor `1b1c72a` and DualSone `03f3bfa`: Autogain and Delta are processor features. Their controls can use the paired layout, but their DSP and parameters remain host responsibilities.
- ArcVu `77f7c1e`: smaller windows and exposed loudness controls reinforce fitting layouts to actual panel bounds.
- GodEQ `6073658`: analyzer/meter performance work is processor-specific; no unmeasured performance changes were imported.
- Remaining reviewed plugin heads (Water Channel, HybridEQ, Blue Channel, OpenPreamp, TubeChannel, Sp9ceVerb, LibreEQ) mainly contain credits, release/host fixes, or initial product work rather than new portable toolkit widgets.

## Matching APIs

| Feature | GoodLookin DRI | GoodLookinUI |
| --- | --- | --- |
| Readout window | `readout::ReadoutWindow` | `<goodlookinui/ReadoutWindow.h>` |
| Card layout | `layout::readout_card` | `layout::readoutCard` in `<goodlookinui/Layout.h>` |
| Caption/switch layout | `layout::caption_switch` | `layout::captionSwitch` in `<goodlookinui/Layout.h>` |

The C++ layout uses a dependency-free `layout::Rect {x,y,w,h}`; convert to the host rectangle type when painting. Rust uses `scene::Rect`. Layout calls return an optional result on invalid input. Zero-sized panels produce zero-sized cells; hosts should hide those cells.

Readout input is **already measured** LUFS and crest dB plus the sample duration of that measurement. Call on the measurement thread, independent of repaint frequency. The class is single-thread owned: publish the two results with the host's atomic transport. No locks or allocations occur in updates. Do not call it concurrently from the GUI.

Defaults: forty rounded 50 ms bins, approximately 1.95–2 seconds retained history, energy-mean loudness excluding values at/below -70 LUFS, maximum crest. Empty values are -200 LUFS and -100 dB crest. Nonfinite values are ignored; loudness above +100 is rejected. Invalid sample rates fall back to 48 kHz; positive rates are clamped to 1–768000 Hz. Reset or reconstruct on a transport/sample-rate change. These are display statistics, not BS.1770 detectors or replacements for VU/PPM ballistics. The window is quantized, not an exact sliding two-second window.

Example (Rust):

```rust
let mut display = goodlookindri::readout::ReadoutWindow::new(48000.0);
display.add(-18.0, 12.0, 512);
let loudness = display.loudness();
```

Example (C++):

```cpp
#include <goodlookinui/ReadoutWindow.h>
#include <goodlookinui/Layout.h>
goodlookinui::ReadoutWindow display(48000);
display.add(-18, 12, 512);
auto card = goodlookinui::layout::readoutCard({0, 0, 160, 300}, 0, 5, 8);
```

## Scope ledger

| ID | Item / source | Code (Rust / C++) | GUI | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| S1 | Stable energy-mean loudness / DualSone | `src/readout.rs` / `ReadoutWindow.h` | Host supplies readout | Matching mixed-energy, gate, reset, invalid-value and block-partition tests | Verified core behavior |
| S2 | Crest hold and expiry / DualSone | Same | Host supplies readout | Matching maximum and history-expiry tests at 44.1/48/96 kHz | Verified core behavior |
| S3 | Readout cards fit panel / RadarVU | `src/layout.rs` / `Layout.h` | Host paints returned rectangles | Matching five-card tests at four panel heights, including zero | Verified geometry; visual interaction unverified |
| S4 | Caption belongs to its switch / DualSone | Same | Host paints caption and switch | Matching zero/narrow/normal row bounds tests | Verified geometry; visual interaction unverified |
| S5 | Matching feature behavior / user | Both new API sets | Host-owned | Same numerical expectations and geometry cases in `tests/ui_goodies.rs` and `tests/ui_goodies.cpp` | Verified for these additions |

Validation: Rust core tests in Debug and Release; C++ core suite in Debug and Release (six suites pass in each). Debug enables assertions in the existing tests; new checks also remain active with NDEBUG. Runtime DAW, native screenshots and performance sampling were not run: these additions are dependency-free utilities with no new timer, painter, host parameter or plugin binary. No host integration or visual parity certification is claimed.

Full historical parity remains unverified: the existing Rust shell, developer inspector and C++ JUCE editor have different host integrations, and catalogue pixel comparisons are still an existing milestone. The matching additions here do not establish full toolkit parity.

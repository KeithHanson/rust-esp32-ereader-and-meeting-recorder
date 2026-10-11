# Waveshare ESP32-S3-ePaper-3.97: Rust Research and Prototyping Handoff

**Date:** 2026-10-10  
**Goal:** Build battery-powered portrait e-reader and audio-recorder firmware in Rust.  
**Status:** Research and proposed implementation; no Rust firmware has yet been compiled or validated on this particular board.

## Recommendation

Start with **Rust + ESP-IDF** (`esp-idf-template`, `esp-idf-hal`, `esp-idf-svc`, `esp-idf-sys`). ESP-IDF has established facilities for SDMMC, I²S audio, Wi-Fi, NVS, and low-power operation. Implement the e-paper controller in Rust using Waveshare's proven commands, or wrap the C/C++ vendor driver behind a small C ABI. Consider native `esp-hal` (`no_std`) after validating hardware and measuring power. Rust's `epd-waveshare` crate is useful for architecture, but exact support for this panel is **not established**.

**Key research finding:** Waveshare's tutorial identifies the controller as **SSD1677**, the matching GxEPD2 driver as **`GxEPD2_397_GDEM0397T81`**, and the display pinout below.

## Board specification

| Subsystem | Details |
|---|---|
| CPU | ESP32-S3 dual-core Xtensa LX7, up to 240 MHz |
| Memory | 512 KB internal SRAM, 8 MB PSRAM, 16 MB flash (verify actual module at runtime) |
| E-paper | 3.97-inch monochrome 800 × 480, SSD1677, SPI |
| Display refresh | Full ~3.5 s; fast ~2.8 s; partial ~0.6 s (vendor values) |
| Audio | ES8311 codec, onboard microphone, NS4150B amplifier |
| Storage | MicroSD/TF slot; inspect SDMMC reference configuration |
| Power | TG28 PMIC, rechargeable 3.7 V lithium battery connector (MX1.25 2-pin) |
| Sensors | QMI8658 IMU, SHTC3 temperature/humidity, PCF85063 RTC |
| Connectivity | 2.4 GHz Wi-Fi and BLE 5 LE |
| Controls | Up, function, down, BOOT and PWR (verify GPIOs and electrical behavior) |

Some vendor pages show inconsistent module part labeling; use the schematic and runtime flash/PSRAM report to settle it.

### Verified display pins

| E-paper signal | GPIO |
|---|---:|
| BUSY | 3 |
| DC | 9 |
| CS | 10 |
| SCLK | 11 |
| MOSI | 12 |
| RST | 46 |

These come from Waveshare's dedicated e-paper tutorial. The board has no separately controlled display power-enable GPIO. **Do not guess** SDMMC, audio, I²C, PMIC, or button pins: extract them from the board schematic and official example source. GPIO46 is a boot-strapping-related pin; preserve vendor reset behavior.

### Display memory and geometry

- Native landscape: 800 × 480. Logical portrait rendering: 480 × 800 with coordinate rotation.
- One 1-bpp framebuffer: `800 * 480 / 8 = 48,000` bytes (46.875 KiB).
- Two framebuffers: 96,000 bytes; packed 2-bpp grayscale: 96,000 bytes per frame, but confirm the actual controller transfer protocol.
- Vendor display area: 86.40 × 51.84 mm.
- Partial windows: X and width should align to multiples of 8 pixels.
- Periodically full-refresh to clear ghosting; Waveshare suggests ~5–10 partial updates as an initial rule of thumb.
- Never power down during BUSY. Deep sleep usually causes an ESP32-S3 restart on wake; save reading state.
- Vendor's <40 mW refresh figure is for the display, **not whole-board draw**.

## Authoritative references

| Resource | URL | Purpose |
|---|---|---|
| Board wiki | https://docs.waveshare.com/ESP32-S3-ePaper-3.97 | Hardware, schematics, vendor instructions |
| Vendor examples | https://github.com/waveshareteam/ESP32-S3-ePaper-3.97 | ESP-IDF/Arduino code and initialization |
| Waveshare EPD tutorial | https://docs.waveshare.net/ESP32-Peripheral-Tutorials/Display/E-Paper/ | SSD1677, exact GPIO, refresh examples |
| GxEPD2 | https://github.com/ZinggJM/GxEPD2 | Panel-specific `GxEPD2_397_GDEM0397T81` |
| Draftling hardware notes | https://github.com/clackups/draftling/blob/main/HARDWARE.md | Independent board implementation, SDMMC/buttons |
| Rust on ESP Book | https://docs.espressif.com/projects/rust/book/ | Toolchains and architecture |
| ESP-IDF Rust template | https://github.com/esp-rs/esp-idf-template | Starting project |
| ESP-IDF Rust crates | https://github.com/esp-rs/esp-idf | `esp-idf-hal`, `esp-idf-svc`, `esp-idf-sys` |
| Native HAL | https://github.com/esp-rs/esp-hal | Alternative `no_std` firmware |
| Flashing | https://github.com/esp-rs/espflash | USB flashing/monitoring |
| Generic EPD crate | https://github.com/rust-embedded-community/epd-waveshare | Driver patterns, check exact panel support |
| Graphics | https://github.com/embedded-graphics/embedded-graphics | Framebuffer drawing |
| Text layout | https://github.com/embedded-graphics/embedded-text | Wrapping and typography |

## Rust architecture choices

**A. ESP-IDF + Rust (`std`) — recommended.** Most straightforward for SDMMC, audio, Wi-Fi, storage and vendor interoperability. `esp-idf-*` Rust wrappers are community-maintained and may lag upstream ESP-IDF; pin mutually compatible versions.

**B. Native `esp-hal` (`no_std`).** More control over peripherals, allocation, and runtime. Additional effort may be required for Wi-Fi, PSRAM, SDMMC, ES8311 and TG28. Follow examples for a specific `esp-hal` release tag, not unpinned `main`.

**C. Hybrid C/C++ driver + Rust app.** Compile vendor display code as an ESP-IDF component; export `extern "C"` functions for init, framebuffer write, refresh and sleep; bind from Rust. Make one component own the SPI bus. Good fast-path if the SSD1677 waveform port is complex.

## Arch Linux toolchain setup

Check current template prerequisites before installing. Starting package/tool list:

```bash
sudo pacman -S --needed base-devel git curl wget python python-pip \
  cmake ninja clang llvm flex bison gperf ccache libffi openssl \
  libusb systemd-libs pkgconf
rustup --version
cargo --version
cargo install cargo-generate
cargo install ldproxy
cargo install espup
cargo install espflash
cargo install cargo-espflash
espup install
source "$HOME/export-esp.sh"
```

Verify `pkgconf --modversion libudev`, USB permissions and `espflash --help`. The Espressif export script must be sourced in build shells. Avoid setting `IDF_PATH` to an incompatible external ESP-IDF installation.

### Create the starter project

```bash
cargo generate esp-rs/esp-idf-template cargo
# Project name: ws397-rust
# Target: esp32s3
# Choose an ESP-IDF release supported by the template
cd ws397-rust
source "$HOME/export-esp.sh"
cargo build
espflash board-info --help
espflash flash --monitor target/xtensa-esp32s3-espidf/debug/ws397-rust
```

Before flashing: identify the correct serial port, inspect partition layout, and back up existing flash/NVS if needed. The exact binary path depends on the generated project name.

## Implementation milestones

### M0 — Vendor code audit

```bash
git clone https://github.com/waveshareteam/ESP32-S3-ePaper-3.97 vendor-waveshare
git clone https://github.com/ZinggJM/GxEPD2 vendor-gxepd2
git clone https://github.com/clackups/draftling vendor-draftling
rg -n 'SSD1677|GDEM0397T81|EPD_BUSY|EPD_DC|EPD_RST' vendor-*
rg -n 'ES8311|I2S|SDMMC|TG28|PCF85063|QMI8658|SHTC3' vendor-waveshare
rg -n 'sleep|wakeup|battery|pmic|gpio_hold' vendor-waveshare
```

**Deliverables:** complete GPIO table, I²C addresses, SPI mode/frequency, BUSY polarity, display LUT and init sequence, I²S clock/codec settings, SDMMC mode, button pullups and wake-capable pins, PMIC rails, power-off sequence, flash partitions. Validate against schematic.

### M1 — Boot and diagnostics

Flash Rust hello-world. Log reset reason, flash size, PSRAM detection, free heap, CPU revision and build ID. Verify USB serial, 20 clean boots and recovery from failed flashing.

### M2 — Display bring-up

Implement init, full frame, full refresh, partial refresh, hibernate. Begin with the vendor SSD1677/GDEM0397T81 initialization, not a generic SSD1677 example. Draw checkerboard, black/white, rotated text and a partial-update counter. Time BUSY and full operation; add BUSY timeout. Verify wake-from-sleep display recovery and ghosting.

Illustrative API, **not a compiled driver**:

```rust
pub const WIDTH: usize = 800;
pub const HEIGHT: usize = 480;
pub const FRAME_BYTES: usize = WIDTH * HEIGHT / 8;

pub enum RefreshMode {
    Full,
    FastFull,
    Partial { x: u16, y: u16, width: u16, height: u16 },
}

pub trait EpaperDisplay {
    type Error;
    fn init(&mut self) -> Result<(), Self::Error>;
    fn write_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error>;
    fn refresh(&mut self, mode: RefreshMode) -> Result<(), Self::Error>;
    fn hibernate(&mut self) -> Result<(), Self::Error>;
}
```

The real partial-refresh API may need old/new frame planes and a special refresh sequence.

### M3 — Buttons and reader

Debounce up/function/down GPIOs; rotate logical 480×800 framebuffer to panel orientation; use `embedded-graphics` and `embedded-text`. Start with plain UTF-8 text on SD, then pagination/font sizing, then EPUB if justified. Save book identifier, position, font and orientation atomically in NVS or SD. Test 100 page turns and resume after restart.

### M4 — SDMMC and filesystem

Verify vendor SDMMC wiring, pin mux and bus width; Draftling's board port uses 1-bit SDMMC. Mount FAT, stream files, handle missing/removable cards, test write throughput and interrupted writes. Avoid excessive small metadata writes.

### M5 — Audio recorder

From official code, identify ES8311 I²C address, codec register initialization, I²S pins, MCLK/BCLK/WS/data pins, microphone route and amplifier control. Prototype **16-kHz mono 16-bit PCM WAV** via I²S DMA to SD: **32 KB/s**, **1.92 MB/min**, **115.2 MB/hour** decimal, excluding overhead. Ensure WAV header finalization, sample continuity, gain/noise tests, storage-full handling and crash recovery. Consider compression only after a reliable PCM pipeline exists.

### M6 — Power characterization

Measure boot, idle, display full/partial refresh, SD writing, recording, Wi-Fi sync, light sleep, deep sleep and PMIC-off currents. Check TG28 charging behavior, rail enable and battery measurement. Confirm allowed deep-sleep wake GPIOs (not every GPIO is RTC-capable), display hibernate and state persistence. Compare sleep energy against simply remaining awake for short reading intervals.

### M7 — Network synchronization

Use ESP-IDF Wi-Fi for HTTPS transfer of books and audio recordings. Disable radio between transfers; verify TLS certificates, credential handling, retry/backoff and interrupted transfer recovery. BLE optional. Do not assume iPhone can transparently mount SD storage over BLE.

## Suggested project structure

```text
ws397-rust/
  src/
    main.rs
    board.rs                 # verified GPIOs and peripheral ownership
    display/{mod.rs,ssd1677.rs,framebuffer.rs}
    reader/{layout.rs,state.rs}
    audio/{codec.rs,capture.rs,wav.rs}
    buttons.rs
    storage.rs
    power.rs
    network.rs
  docs/{pinout.md,power-tests.md,test-results.md}
  vendor/                  # pinned source references
```

Host-test framebuffer bit order/rotation, text wrapping, WAV header generation and state serialization on Linux. Record crate versions, vendor commits and ESP-IDF revision.

## Acceptance tests

| Experiment | Evidence required |
|---|---|
| Full and partial display | Photos, timing, BUSY logs, ghosting observations |
| Portrait reader | Legible 480×800 pages, correct buttons, stable pagination |
| SD storage | Read/write rates, mount/unmount and error recovery |
| Audio | Valid WAV, playback verification, no dropped samples |
| Deep sleep | Measured microamps/milliamps, wake reason and time |
| Battery | Peak currents, voltage sag, charge behavior, real runtime |
| Wi-Fi | Upload/download reliability, radio-on energy |

### Battery modeling

Use `runtime_hours ≈ usable_capacity_mAh / average_current_mA` only as an initial estimate. More useful is measured energy per page-turn plus sleep current. A simple approximation is `I_avg = Σ(I_state × t_state) / Σ(t_state)`. The PMIC, PSRAM, SD and codec may dominate low-power consumption; the panel's refresh-power specification cannot predict system battery life.

## Critical risks / open questions

1. Exact SSD1677 **panel-specific** waveform and partial-update behavior.
2. Unverified GPIO and clock map for SDMMC, I²S, codec, buttons, and PMIC.
3. Whether TG28 board-level standby current undermines deep sleep.
4. Whether deep-sleep wake can use the preferred page-turn button or needs a different strategy.
5. Reliable audio streaming while writing SD and maintaining battery voltage.
6. 16 MB flash partition layout, PSRAM configuration, and OTA tradeoffs.
7. Mechanical orientation, button accessibility and USB/battery connector clearance in the phone-mounted enclosure.
8. License obligations for copied vendor, GxEPD2 or third-party driver code: inspect each source license before redistribution.

## Concrete next task for an engineering agent

> Create a Rust ESP-IDF starter for Waveshare ESP32-S3-ePaper-3.97. First inspect the official board schematic, ESP-IDF examples, Waveshare SSD1677 tutorial and GxEPD2 `GxEPD2_397_GDEM0397T81`. Produce a verified complete pinout and driver command map with source file/line references. Then compile a Rust hello-world targeting `xtensa-esp32s3-espidf`, and implement an e-paper test displaying text and alternating full/partial refreshes. Keep C FFI as a fallback. Do not fabricate unverified GPIOs or claim the code has been flashed unless physically tested. Document reproducible setup and known issues.

## Sources consulted

- Waveshare board specification: https://www.waveshare.com/esp32-s3-epaper-3.97.htm
- Waveshare board wiki: https://docs.waveshare.com/ESP32-S3-ePaper-3.97
- Waveshare display tutorial: https://docs.waveshare.net/ESP32-Peripheral-Tutorials/Display/E-Paper/
- Official source: https://github.com/waveshareteam/ESP32-S3-ePaper-3.97
- ESP-IDF Rust template: https://github.com/esp-rs/esp-idf-template
- ESP-IDF Rust project: https://github.com/esp-rs/esp-idf
- Native Rust HAL: https://github.com/esp-rs/esp-hal
- Draftling hardware notes: https://github.com/clackups/draftling/blob/main/HARDWARE.md

## Verified: rocker button pins (2026-10-10)

From vendor repo `waveshareteam/ESP32-S3-ePaper-3.97`, `ESP-IDF/08_ESP32-S3_e-Paper-3.97/components/button_bsp/button_bsp.c` (commit at clone time; matches schematic page-1 "KEY & LED" nets Button_Up/Function/Down):

| Control | GPIO | E-colo | Notes |
|---|---|---|---|
| Rocker UP | 4 | active-low, pull-up | multi_button events (click/dbl/long) |
| Rocker press (Function) | 5 | active-low, pull-up | "press the rocker" |
| Rocker DOWN | 6 | active-low, pull-up | |
| BOOT | 0 | active-low, pull-up | side button |

Not a rotary encoder — three independent GPIO switches. Old MicroPython app did not use them (only BOOT=GPIO0). IO expander TCA9554 exists in the mic-test codec board, but the buttons themselves are direct GPIOs 4/5/6 per vendor BSP.

# Flashing the Waveshare ESP32-S3-ePaper-3.97 — working procedure

## Board / cable / permissions
- Board: SKU 33810. Factory app (MicroPython) owns USB as TinyUSB CDC `303a:4001` "Board CDC" — **not** flashable via auto-reset.
- ROM/USJ download enumerates `303a:1001` "USB JTAG/serial debug unit".
- Device node re-appears as `root:uucp 660` on every re-enumeration; current shell lacks the `uucp` group until re-login. Quick: `sudo chmod 666 /dev/ttyACM0`; permanent rule in `scripts/fix-serial-perms.sh` notes.

## Enter download mode (physical, proven)
1. Unplug USB-C 5 s (battery off if fitted).
2. Hold **BOOT**, plug USB-C (tap PWR if USB doesn't power it), hold BOOT ~2 s past re-enumeration, release.
3. Confirm: `lsusb | grep -i espressif` → `303a:1001`.

## Flash (what worked, espflash 4.6.0)
```bash
cd ws397-rust
espflash flash --port /dev/ttyACM0 --chip esp32s3 \
  --before default-reset --after watchdog-reset --flash-size 16mb \
  target/xtensa-esp32s3-espidf/debug/ws397-rust
```
- `--flash-size 16mb` matters: `FlashEnd … Other (0x1)` / `SpiAttach … Other (0x1)` on a 16 MB module with wrong/unknown size (no-stub path).
- `--after watchdog-reset` is what finally leaves the chip into the app; `hard-reset`/RTS pulse on the 1001 CDC leaves it in `waiting for download` when GPIO0 was sampled low at that reset — see console section.
- Stub flasher works fine in ROM mode; default-reset works when node is 1001.

## Console after flashing (1001 / USJ)
- App enumerates `303a:1001` (USJ console enabled via `--cfg espidf_usb_serial_jtag_conn_support` in `.cargo/config.toml` → `CONFIG_ESP_CONSOLE_SECONDARY_USB_SERIAL_JTAG=y`).
- **RTS pulse = chip reset; the boot mode it re-latches depends on GPIO0 state** (BOOT button / strapping). A chip that came from download mode via RTS tends to stay boot:0x20 DOWNLOAD. A **watchdog** reset always goes boot:0x28 SPI_FAST_FLASH_BOOT.
- Reliable capture:
  ```bash
  timeout 20 uv run --with pyserial python3 - <<'EOF'
  import serial, time
  s=serial.Serial('/dev/ttyACM0',115200,timeout=0.3)
  s.dtr=False; s.rts=True; time.sleep(0.2); s.rts=False
  t=time.time()+12; buf=b''
  while time.time()<t:
      d=s.read(4096)
      if d: buf+=d
  print(buf.decode(errors='replace'))
  EOF
  ```
  Expect: `boot:0x28 (SPI_FAST_FLASH_BOOT)` … `ws397_rust: hello world (t=… ms)` every 1 s.
- `espflash monitor` fails non-interactive ("Failed to initialize input reader") — needs a pty; the pyserial snippet above is the scriptable path.

## Flash speed + button-free flashing (verified 2026-10-11)
**`--before default-reset` works with the Rust app running.** Once our IDF firmware owns USB (USJ console, `303a:1001`), DTR/RTS line toggling *does* reset the chip into the ROM download loader — no BOOT button needed. (The factory MicroPython TinyUSB app ignored DTR/RTS; that's why every early attempt needed the physical dance.) So the day-to-day loop is just:
```bash
cd ws397-rust && source "$HOME/export-esp.sh"
env -u RUSTUP_TOOLCHAIN cargo build && \
espflash flash --port /dev/ttyACM0 --chip esp32s3 \
  --before default-reset --after watchdog-reset \
  --flash-size 16mb --no-skip --no-verify \
  target/xtensa-esp32s3-espidf/debug/ws397-rust
```
Measured **~8.4 s end-to-end** for the 1.52 MB app (`--no-skip --no-verify` kills two on-target MD5 scans that espflash does by default: skip-check pre-write + verify post-write).
- App payload is ~1.52 MB; build.rs extracts embedded EPUBs on the host and embeds compact title/chapter/text blobs, so the device does no ZIP/XML work when opening a book.
- `espflash` progress is an indicatif bar on a TTY; piping/grepping hides ALL progress and its completion string is `Flashing has completed!` — never `Hash of data verified`/`Rebooting` (those are esptool strings). Grep-based "is it hung?" checks are guaranteed to look hung.
- NEVER run two flash/monitor processes against `/dev/ttyACM0` — the second eats the first's replies and both hang at `Connecting...`. This (plus a dead stub upload leaving the bootloader mid-command) caused every "hang" this session; a stale owner makes even raw ROM sync return zero bytes while `esptool --before default-reset` still works (its reset pulse evicts the stuck state).
- `espflash board-info/flash` while the chip sits in half-dead stub state appears to "connect" for minutes then dies — always `pgrep espflash` before retrying and prefer one clean foreground run.
- Reference: advisor report `agent://FlashDiagnose` (root-cause ranking + espflash #1012 stub-throughput context).

## Root-cause recap (was: board never in ROM mode)
- 4001/"Board CDC" = app TinyUSB port; every tool failure traced to that: DTR/RTS ioctls dead (Errno 71), sync unanswered, espflash `no-reset-no-sync` faking a chip ID (it never sends SYNC; `-c esp32s3` supplies the ID, and the "Secure Download Mode" warning is inferred after GET_SECURITY_INFO/READ_REG timeouts — false evidence).
- eFuse Secure Download Mode effectively ruled out: once truly in ROM download mode, sync+security-info+flash all succeeded.
- Full analysis: `local://flash-advisor.md` (advisor report).

## M2 e-paper display (SSD1677) — working
- Boot log: 16 MB flash confirmed; PSRAM **confirmed** = `esp_psram: Adding pool of 8192K of PSRAM` (answers the M1 open question). 8 MB octal, `CONFIG_SPIRAM_USE_MALLOC=y`, threshold `ALWAYSINTERNAL=16384` so >16 KiB allocs (the 48 KiB framebuffer) land in PSRAM.
- Display path: `src/ssd1677.rs` (blocking, `SpiBusDriver` on SPI2, SW CS/DC/RST/BUSY), `src/board.rs` (pins), `src/main.rs` (init -> `blank_fb()` -> `draw_hello` -> full refresh).
- Pins: BUSY=3 DC=9 CS=10 SCLK=11 MOSI=12 RST=46 (match vendor). SPI mode 0, 20 MHz, `Dma::Disabled` (CPU FIFO, 64-byte internal chunking) — works.
- **Task watchdog**: a 3.5 s full refresh on the main task trips the 5 s `TG0WDT` and reboots the board -> `CONFIG_ESP_TASK_WDT_EN=n` in sdkconfig.defaults for this milestone. Reader app (M3+) must re-enable and run the display on a worker task that registers + feeds the WDT.
- **Noise bug (fixed)**: first driver showed TV-static. Root cause = wrong `0x01` MUX (sent HEIGHT not HEIGHT-1) and wrong `0x45` RAM-Y window direction (data-entry mode 0x11=0x01 is X-increment / **Y-decrement**, so window must be start=479 end=0, i.e. `DF 01 00 00`). An all-`0xFF` buffer can't scramble into noise, so it had to be RAM addressing. Advisor report: `agent://EpdNoiseAdvisor-2`, brief `local://epd-noise-advisor.md`.
- Text color: `BinaryColor::Off` = black (raw bit 0). `On` = white — drawing `On` on the white-clear buffer is invisible.
- **Portrait + big text**: panel RAM is physically landscape (800 wide); portrait is a **software rotation** into the native buffer, like the vendor (`GUI_Paint.c` `Paint_SetPixel` `Rotate` case; MicroPython did the same). `ssd1677::PortraitFb` (480x800) is the drawing surface; `rotate90_ccw_into(&portrait, &mut native)` maps portrait `(x,y)`->native `(y, 479-x)` before `display()`. Verified upright when the board is turned 90° CCW.
- **Font scaling**: embedded-graphics mono fonts don't scale (max `FONT_10X20`). `draw_hello` rasterises the string at 10x20 into a scratch FB then nearest-neighbour upscales x4 (-> 40x80 px glyphs). `main.rs` has the SCALE/ORIGIN consts to retune.
- **Windowed partial refresh (verified 2026-10-11)**: `Pipeline::draw` renders a fresh portrait frame, diffs it against the displayed mirror, converts the changed rectangle with the same inverse map as the working full-frame rotation, and gathers a byte-identical native RAM crop. `display_partial_planes` hardware-resets the panel, replays the proven initialized `0x11=0x01` register state, narrows the RAM window, writes the new crop to `0x24`, then triggers Mode 2 with `0x22=0xFF`. Measured menu-selection updates: **~911 ms**, no full-screen flash; every 11th incremental update uses the working `0xF7` full refresh to limit ghosting. Critical invariant: after gathering the crop, `Pipeline::draw` must call `epd.display_partial_planes(...)` before committing its software mirror. Omitting that call produces a convincing ~65 ms software-only “partial” log while the panel remains unchanged.

## M3 reader controls
- Reader: rocker up/down changes page; hold center for 2 seconds to open the reader menu.
- Reader menu: chapter select, font-size, back to library, or close. Font-size offers small/medium/large/xlarge and repaginates the current chapter.
- EPUB punctuation outside the u8g2 glyph set is normalized at build time. Missing curly-quote glyphs previously stopped rendering the remainder of a line (for example, `I won’t…` displayed only `I won`); this was not EPUB data loss or a partial-refresh defect.
- Reader body sizes use 24/30/35/42 px fonts; title/footer chrome uses 18 px Helvetica. Landscape rendering draws through a coordinate-transform target into the fixed 480x800 storage buffer, then maps all 800x480 logical pixels directly to native panel coordinates. Portrait and landscape were hardware-verified 2026-10-11.
- Add locally owned EPUBs under `ws397-rust/books/`; `build.rs` extracts every `.epub` there. EPUB files are intentionally gitignored and are not distributed with the public repository.

## Notes for later milestones
- Flash layout default: single factory app at 0x10000, 16 MB. Consider custom partition table + NVS for M3 book state.

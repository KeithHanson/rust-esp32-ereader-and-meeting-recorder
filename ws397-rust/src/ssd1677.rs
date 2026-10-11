//! Minimal SSD1677 driver for the Waveshare 3.97" 800x480 1-bpp panel.
//! Replicates the vendor ESP-IDF command sequence verbatim from
//! `vendor/ESP-IDF/01_E-Paper_Example/components/epaper_port/epaper_port.c`
//! (commit 9b12d407).
//!
//! Blocking by design: the panel's BUSY handshake and (for a reader) the ~3.5 s
//! full-refresh are inherently blocking, and CONFIG_SPI_MASTER_ISR_IN_IRAM=y
//! disables esp-idf-hal's async SPI impls anyway. A single FreeRTOS task drives
//! it to completion.
//!
//! Bus model: `spi` is a raw blocking `SpiBus` (no CS). This driver owns DC, CS,
//! RST and BUSY as plain GPIOs and issues one bus `write` per logical
//! command/data beat, toggling CS around each beat. This reproduces the vendor
//! exactly: its `spi_device_polling_transmit` (SPI mode 0) asserts hardware CS
//! around each single-byte transfer, while DC is latched by the panel on the
//! next clock edge and therefore persists across the CS boundary between a
//! command byte and the data that follows.

use embedded_graphics::{
    framebuffer::Framebuffer,
    image::GetPixel,
    pixelcolor::{
        raw::{BigEndian, RawU1},
        BinaryColor,
    },
    prelude::Point,
};
use embedded_hal::{
    delay::DelayNs,
    digital::{InputPin, OutputPin},
    spi::SpiBus,
};

use crate::board::panel;

/// 1-bpp framebuffer in the panel's native RAM layout: 800 px, MSB-first within
/// a byte (embedded-graphics `Framebuffer::set_pixel` uses `0x80 >> x%8`, which
/// matches the vendor `GUI_Paint.c:169-171` packing byte-for-byte), scanline
/// major. `data()` is sent straight to the panel RAM registers 0x24/0x26.
pub type Fb = Framebuffer<
    BinaryColor,
    RawU1,
    BigEndian,
    { panel::WIDTH },
    { panel::HEIGHT },
    { panel::FRAME_BYTES },
>;

/// Heap-allocate a framebuffer cleared to white (bit=1 -> white on the panel).
///
/// Built directly into a `Box` so the 48 KiB backing array is never
/// materialised on the task stack (which would overflow an 8 KiB stack).
pub fn blank_fb() -> Box<Fb> {
    let mut fb: Box<Fb> = Box::new(Fb::new());
    for b in fb.data_mut().iter_mut() {
        *b = 0xFF;
    }
    fb
}

/// 1-bpp framebuffer in PORTRAIT logical orientation (480 wide x 800 tall).
/// This is the surface drawing/text targets; it is rotated into the native
/// [`Fb`] before `display()` streams it to the controller RAM.
pub type PortraitFb = Framebuffer<
    BinaryColor,
    RawU1,
    BigEndian,
    { panel::PORTRAIT_WIDTH },
    { panel::PORTRAIT_HEIGHT },
    { panel::PORTRAIT_BYTES },
>;

/// Heap-allocate a portrait framebuffer cleared to white.
pub fn blank_portrait() -> Box<PortraitFb> {
    let mut fb: Box<PortraitFb> = Box::new(PortraitFb::new());
    for b in fb.data_mut().iter_mut() {
        *b = 0xFF;
    }
    fb
}

/// Rotate a 90° CCW portrait image into the native (landscape) buffer.
///
/// Mapping (90° CCW: the top edge of the portrait image runs down the native
/// LEFT edge; a portrait pixel `(x, y)` -> native `(y, Wp-1 - x)`), verified
/// against the panel by re-flash. Same 1-bpp MSB-first packing on both sides.
pub fn rotate90_ccw_into(src: &PortraitFb, dst: &mut Fb) {
    let w = panel::PORTRAIT_WIDTH;
    let h = panel::PORTRAIT_HEIGHT;
    for yp in 0..h {
        for xp in 0..w {
            // Portrait logical (x=xp rightward, y=yp downward) -> native.
            let nx = yp;
            let ny = (w - 1 - xp) as usize;
            let c = src
                .pixel(Point::new(xp as i32, yp as i32))
                .expect("pixel within portrait bounds");
            dst.set_pixel(Point::new(nx as i32, ny as i32), c);
        }
    }
}

/// Rotate a portrait image 180° (upside-down) into the native buffer — the
/// "landscape/inverted" reading mode: same portrait canvas, opposite page
/// direction (bottom-edge-held). `(x,y)` -> native `(Wp-1-x, Hp-1-y)` where
/// the portrait pixel at (x,y) maps through rotate90_ccw twice.
pub fn rotate180_ccw_into(src: &PortraitFb, dst: &mut Fb) {
    let w = panel::PORTRAIT_WIDTH;
    let h = panel::PORTRAIT_HEIGHT;
    for yp in 0..h {
        for xp in 0..w {
            // ccw once: (x,y)->(y, W-1-x); twice: (x,y)->(W-1-x, H-1-y) in
            // portrait space, then ccw mapping into native:
            let px = (w - 1 - xp) as usize; // after 180 in portrait space
            let py = (h - 1 - yp) as usize;
            let nx = py;
            let ny = (w - 1 - px) as usize;
            let c = src
                .pixel(Point::new(xp as i32, yp as i32))
                .expect("pixel within portrait bounds");
            dst.set_pixel(Point::new(nx as i32, ny as i32), c);
        }
    }
}

pub struct Ssd1677<S, C, D, R, B> {
    spi: S,
    cs: C,
    dc: D,
    rst: R,
    busy: B,
}

impl<S, C, D, R, B> Ssd1677<S, C, D, R, B>
where
    S: SpiBus,
    C: OutputPin,
    D: OutputPin,
    R: OutputPin,
    B: InputPin,
{
    pub fn new(spi: S, cs: C, dc: D, rst: R, busy: B) -> Self {
        Self {
            spi,
            cs,
            dc,
            rst,
            busy,
        }
    }

    /// Command beat: CS low, DC low, one byte, CS high.
    fn cmd(&mut self, c: u8) {
        self.cs.set_low().ok();
        self.dc.set_low().ok();
        let _ = self.spi.write(&[c]);
        self.cs.set_high().ok();
    }

    /// Data beat: CS low, DC high, payload, CS high.
    fn data(&mut self, d: &[u8]) {
        self.cs.set_low().ok();
        self.dc.set_high().ok();
        let _ = self.spi.write(d);
        self.cs.set_high().ok();
    }

    /// Vendor `EPD_Reset` (epaper_port.c:102-110): 50/2/50 ms.
    fn reset<T: DelayNs>(&mut self, dly: &mut T) {
        self.rst.set_high().ok();
        dly.delay_ms(50);
        self.rst.set_low().ok();
        dly.delay_ms(2);
        self.rst.set_high().ok();
        dly.delay_ms(50);
    }

    /// Vendor `EPD_ReadBusy` (epaper_port.c:164-175): 100 ms settle, then wait
    /// while BUSY is HIGH, 20 ms poll, with a 10 s safety cap.
    /// Returns false if BUSY never released (10 s cap) - callers MUST NOT
    /// issue further commands after that (datasheet: never interrupt Master
    /// Activation).

    fn wait_busy<T: DelayNs>(&mut self, dly: &mut T) -> bool {
        dly.delay_ms(100);
        for _ in 0..500 {
            if !self.busy.is_high().unwrap_or(false) {
                return true;
            }
            dly.delay_ms(20);
        }
        log::warn!("e-paper BUSY still high after 10 s");
        false
    }

    /// Vendor `EPD_Init` (epaper_port.c:222-328) verbatim.
    pub fn init<T: DelayNs>(&mut self, dly: &mut T) {
        dly.delay_ms(10);
        self.reset(dly);
        self.wait_busy(dly);
        self.cmd(0x12); // SWRESET
        self.wait_busy(dly);
        self.init_registers(dly);
    }

    /// The post-reset configuration beats of `EPD_Init`. Partial updates
    /// hardware-reset the panel (vendor `EPD_Display_Partial` calls
    /// `EPD_Reset()`), which clears register state — so the partial path must
    /// replay these (like the vendor's init, minus the SWRESET that only makes
    /// sense after power-on).
    fn init_registers<T: DelayNs>(&mut self, dly: &mut T) {
        let _ = dly;
        self.cmd(0x18); // internal temperature sensor
        self.data(&[0x80]);

        self.cmd(0x0C); // booster soft start
        self.data(&[0xAE, 0xC7, 0xC3, 0xC0, 0x80]);

        // Driver output control: MUX field is (gate count - 1), per SSD1677
        // (MUX[9:0]+1 = gates). Vendor sends HEIGHT-1 = 479 = 0x01DF. COM scan
        // down, SM=0, TB=1 in the third byte.
        let last_y = (panel::HEIGHT - 1) as u16;
        self.cmd(0x01);
        self.data(&[last_y as u8, (last_y >> 8) as u8, 0x02]);

        self.cmd(0x3C); // border waveform
        self.data(&[0x01]);

        self.cmd(0x11); // data entry: X increment, Y DECREMENT (mode 0x01)
        self.data(&[0x01]);

        self.set_window(0, panel::WIDTH as u16 - 1, 0, panel::HEIGHT as u16 - 1);
        self.wait_busy(dly);

        self.cmd(0x3C);
        self.data(&[0x01]);
    }

    /// Program the RAM address windows and counters for a region.
    ///
    /// `0x44` X window start->end ascending; `0x45` Y window is programmed
    /// start=high, end=low because the data-entry mode (0x11=0x01) walks Y
    /// DOWN (verified on panel; vendor `EPD_Display` uses the same order via
    /// HEIGHT-1/0). Counters are seeded at (xs, ys).
    ///
    /// Partial-refresh policy for this app (project rule): every UI update
    /// after the initial full refresh pushes ONLY the changed native-space
    /// window via `refresh_window`; a full-screen refresh is reserved for
    /// screen changes and every 10th update (panel anti-ghosting).
    pub fn set_window(&mut self, x0: u16, x1: u16, y0: u16, y1: u16) {
        // Y-decrement entry mode (0x11=0x01): 0x45 window start = y1 (top),
        // end = y0; counter seeds at y0. Full-frame path is panel-proven with
        // this convention.
        self.cmd(0x44);
        self.data(&[x0 as u8, (x0 >> 8) as u8, x1 as u8, (x1 >> 8) as u8]);
        self.cmd(0x45);
        self.data(&[y1 as u8, (y1 >> 8) as u8, y0 as u8, (y0 >> 8) as u8]);
        self.cmd(0x4E);
        self.data(&[x0 as u8, (x0 >> 8) as u8]);
        self.cmd(0x4F);
        self.data(&[y0 as u8, (y0 >> 8) as u8]);
    }

    /// Vendor `EPD_Display` + `EPD_TurnOnDisplay` (epaper_port.c:534-556,
    /// 186-192): write both RAM planes (0x24 new / 0x26 old) with identical
    /// content, then full refresh (LUT 0xF7).
    pub fn display<T: DelayNs>(&mut self, dly: &mut T, fb: &Fb) {
        const CHUNK: usize = 4096;
        let buf = fb.data();

        // A previous partial update leaves the RAM window narrowed; always
        // reprogram the full window before a whole-frame transfer.
        // A partial performs a hardware reset that restores POR entry mode
        // (0x11=0x03); the panel-proven full path uses X+/Y-decrement, so
        // re-establish it before programming the full window.
        self.cmd(0x11);
        self.data(&[0x01]);
        self.set_window(0, panel::WIDTH as u16 - 1, 0, panel::HEIGHT as u16 - 1);
        self.cmd(0x24);
        for c in buf.chunks(CHUNK) {
            self.data(c);
        }
        self.cmd(0x26);
        for c in buf.chunks(CHUNK) {
            self.data(c);
        }

        self.cmd(0x22);
        self.data(&[0xF7]); // load LUT, full refresh, then sleep (vendor EPD_TurnOnDisplay)
        self.cmd(0x20);
        self.wait_busy(dly);
        log::info!("display: BUSY released");
    }

    /// Refresh one native-space window without a full-screen flash.
    ///
    /// Hardware reset is required for Mode-2 activation on this panel. After
    /// reset, replay the proven full-path register state, then overwrite its
    /// full RAM window with the requested narrow window. The payload is the
    /// exact crop the working full-frame path would send to the same RAM
    /// coordinates. Only 0x24 receives the new frame; 0x26 retains the old
    /// frame used by the Mode-2 differential waveform.
    pub fn display_partial_planes<T: DelayNs>(
        &mut self,
        dly: &mut T,
        new: &Fb,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
    ) {
        let x0 = (x0 / 8 * 8).min(panel::WIDTH - 8);
        let x1 = ((x1 + 7) / 8 * 8).clamp(x0 + 8, panel::WIDTH);
        let y0 = y0.min(panel::HEIGHT - 1);
        let y1 = y1.clamp(y0 + 1, panel::HEIGHT);
        log::info!(
            "partial [{},{},{},{}] {}B",
            x0,
            y0,
            x1,
            y1,
            (x1 - x0) / 8 * (y1 - y0)
        );

        self.reset(dly);
        if !self.wait_busy(dly) {
            return;
        }
        self.init_registers(dly);

        self.cmd(0x11);
        self.data(&[0x01]);
        self.set_window(x0 as u16, (x1 - 1) as u16, y0 as u16, (y1 - 1) as u16);

        let stride = panel::WIDTH / 8;
        let xb0 = x0 / 8;
        let xb1 = x1 / 8;
        self.cmd(0x24);
        for ry in y0..y1 {
            self.data(&new.data()[ry * stride + xb0..ry * stride + xb1]);
        }

        self.cmd(0x22);
        self.data(&[0xFF]);
        self.cmd(0x20);
        self.wait_busy(dly);
    }
}

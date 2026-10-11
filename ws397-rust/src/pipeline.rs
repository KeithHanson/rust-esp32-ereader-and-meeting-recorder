//! Display pipeline: render -> rotate -> push.
//!
//! Partial-refresh policy (project rule): every *incremental* UI update
//! (selection moves, page turns) pushes ONLY the changed window with the
//! no-flash partial LUT. Whole-frame full refreshes happen on screen
//! entry/exit, font/rotation changes, book open, and as an anti-ghosting
//! flush every 11th update.
//!
//! Rotation model: portrait `(x,y)` maps to native `(y,479-x)`. Landscape is
//! drawn through a logical target into rotated 480x800 storage and maps back
//! to native `(x,y)`. Both mappings keep rectangles axis-aligned.

use crate::board::panel;
use crate::screens::{Action, AppState};
use crate::ssd1677::{self, Ssd1677};
use crate::ui::Orientation;
use embedded_graphics::image::GetPixel;
use embedded_graphics::prelude::*;
use esp_idf_svc::hal::delay::FreeRtos;

/// Map a logical changed rectangle to panel-native coordinates. Native X is
/// padded outward to the controller's 8-pixel RAM-addressing boundary.
fn native_rect(rect: (usize, usize, usize, usize), portrait: bool) -> (usize, usize, usize, usize) {
    let (x0, y0, x1, y1) = rect;
    if portrait {
        debug_assert!(x0 < x1 && x1 <= panel::PORTRAIT_WIDTH);
        debug_assert!(y0 < y1 && y1 <= panel::PORTRAIT_HEIGHT);
    } else {
        debug_assert!(x0 < x1 && x1 <= panel::WIDTH);
        debug_assert!(y0 < y1 && y1 <= panel::HEIGHT);
    }
    let (raw_nx0, ny0, raw_nx1, ny1) = if portrait {
        (
            y0,
            panel::PORTRAIT_WIDTH - x1,
            y1,
            panel::PORTRAIT_WIDTH - x0,
        )
    } else {
        (x0, y0, x1, y1)
    };
    let nx0 = raw_nx0 / 8 * 8;
    let nx1 = ((raw_nx1 + 7) / 8 * 8).min(panel::WIDTH);
    debug_assert!(nx0 < nx1 && ny0 < ny1 && ny1 <= panel::HEIGHT);
    (nx0, ny0, nx1, ny1)
}

fn logical_pixel(
    fb: &ssd1677::PortraitFb,
    portrait: bool,
    x: usize,
    y: usize,
) -> embedded_graphics::pixelcolor::BinaryColor {
    let point = if portrait {
        Point::new(x as i32, y as i32)
    } else {
        Point::new(y as i32, panel::PORTRAIT_HEIGHT as i32 - 1 - x as i32)
    };
    fb.pixel(point).expect("logical pixel maps inside storage")
}

/// Gather the logical frame storage into the native buffer over
/// [nx0,nx1)x[ny0,ny1), bit-for-bit equal to the corresponding full push.
fn gather_native_window(
    src: &ssd1677::PortraitFb,
    dst: &mut ssd1677::Fb,
    portrait: bool,
    nx0: usize,
    ny0: usize,
    nx1: usize,
    ny1: usize,
) {
    debug_assert!(nx0 % 8 == 0 && nx1 % 8 == 0);
    let pstride = panel::PORTRAIT_WIDTH / 8; // 60
    let nstride = panel::WIDTH / 8; // 100
    let s = src.data();
    let d = dst.data_mut();
    if portrait {
        // Inverse of rotate90_ccw_into: native (nx,ny) <- portrait (479-ny, nx).
        for ny in ny0..ny1 {
            let sx = panel::PORTRAIT_WIDTH - 1 - ny;
            let sb = sx / 8;
            let sm = 0x80u8 >> (sx & 7);
            let drow = ny * nstride;
            for xb in nx0 / 8..nx1 / 8 {
                let mut out = 0u8;
                for k in 0..8usize {
                    let sy = xb * 8 + k;
                    if s[sy * pstride + sb] & sm != 0 {
                        out |= 0x80u8 >> k;
                    }
                }
                d[drow + xb] = out;
            }
        }
    } else {
        // Inverse of rotate180_ccw_into: native (nx,ny) <- portrait (ny, 799-nx)
        // (implemented map: nx = 799-y, ny = x).
        for ny in ny0..ny1 {
            let sx = ny;
            let sb = sx / 8;
            let sm = 0x80u8 >> (sx & 7);
            let drow = ny * nstride;
            for xb in nx0 / 8..nx1 / 8 {
                let mut out = 0u8;
                for k in 0..8usize {
                    let nxp = xb * 8 + k;
                    let sy = panel::PORTRAIT_HEIGHT - 1 - nxp;
                    if s[sy * pstride + sb] & sm != 0 {
                        out |= 0x80u8 >> k;
                    }
                }
                d[drow + xb] = out;
            }
        }
    }
}

/// Persistent display state: mirrors of what the panel RAM holds.
pub struct Pipeline {
    /// Portrait surface with the image currently ON the panel.
    pub logical: Box<ssd1677::PortraitFb>,
    /// Portrait surface for the freshly rendered frame.
    pub shaded: Box<ssd1677::PortraitFb>,
    /// Native (landscape) mirror of `logical`'s rotated image.
    pub native: Box<ssd1677::Fb>,
    pub partials_since_full: u32,
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            logical: ssd1677::blank_portrait(),
            shaded: ssd1677::blank_portrait(),
            native: ssd1677::blank_fb(),
            partials_since_full: 0,
        }
    }

    /// Full push: rotate all of `shaded` into `native`, full-refresh, swap.
    fn push_full<S, C, D, R, B>(&mut self, epd: &mut Ssd1677<S, C, D, R, B>, portrait: bool)
    where
        S: embedded_hal::spi::SpiBus,
        C: embedded_hal::digital::OutputPin,
        D: embedded_hal::digital::OutputPin,
        R: embedded_hal::digital::OutputPin,
        B: embedded_hal::digital::InputPin,
    {
        if portrait {
            ssd1677::rotate90_ccw_into(&self.shaded, &mut self.native);
        } else {
            ssd1677::rotate180_ccw_into(&self.shaded, &mut self.native);
        }
        epd.display(&mut FreeRtos, &self.native);
        std::mem::swap(&mut self.logical, &mut self.shaded);
        self.partials_since_full = 0;
    }

    /// Render and refresh. Full updates redraw both RAM planes with the
    /// panel-proven 0xF7 waveform. Incremental updates compute the changed
    /// portrait rectangle, gather the byte-identical native framebuffer crop,
    /// and send it through the 0xFF Mode-2 window refresh. Every eleventh
    /// incremental update uses a full refresh to limit ghosting.
    pub fn draw<S, C, D, R, B>(
        &mut self,
        st: &AppState,
        epd: &mut Ssd1677<S, C, D, R, B>,
        full: bool,
        region: Option<(usize, usize, usize, usize)>,
    ) where
        S: embedded_hal::spi::SpiBus,
        C: embedded_hal::digital::OutputPin,
        D: embedded_hal::digital::OutputPin,
        R: embedded_hal::digital::OutputPin,
        B: embedded_hal::digital::InputPin,
    {
        let t0 = std::time::Instant::now();
        let portrait = st.layout.orientation == Orientation::Portrait;
        let (w, h) = (st.layout.width, st.layout.height);
        // Render fresh every frame (white + draw). The partial window is the
        // DIFF between the fresh frame and `logical`, so moved-away
        // highlights are white inside the window and erase correctly.
        for b in self.shaded.data_mut().iter_mut() {
            *b = 0xFF;
        }
        crate::screens::render(
            &mut *self.shaded,
            st,
            &mut crate::screens::Dirty::empty(),
            full,
        );

        if full {
            self.push_full(epd, portrait);
            log::info!("frame {} ms (full)", t0.elapsed().as_millis());
            return;
        }

        // Determine the changed logical rectangle.
        let rect = match region {
            Some(r) => r,
            None => {
                let mut x0 = usize::MAX;
                let mut x1 = 0usize;
                let mut y0 = usize::MAX;
                let mut y1 = 0usize;
                for y in 0..h {
                    for x in 0..w {
                        if logical_pixel(&self.logical, portrait, x, y)
                            != logical_pixel(&self.shaded, portrait, x, y)
                        {
                            if x < x0 {
                                x0 = x;
                            }
                            if x + 1 > x1 {
                                x1 = x + 1;
                            }
                            if y < y0 {
                                y0 = y;
                            }
                            if y + 1 > y1 {
                                y1 = y + 1;
                            }
                        }
                    }
                }
                if x0 == usize::MAX {
                    return; // no visual change
                }
                (x0, y0, x1, y1)
            }
        };

        self.partials_since_full += 1;
        if self.partials_since_full >= 11 {
            self.push_full(epd, portrait);
            log::info!("frame {} ms (anti-ghost full)", t0.elapsed().as_millis());
            return;
        }

        let (nx0, ny0, nx1, ny1) = native_rect(rect, portrait);
        gather_native_window(&self.shaded, &mut self.native, portrait, nx0, ny0, nx1, ny1);
        epd.display_partial_planes(&mut FreeRtos, &self.native, nx0, ny0, nx1, ny1);
        // Mirror the portrait frame. Do NOT resync the panel's 0x26 plane:
        // Mode 2 drives transitions from 0x26(old) to 0x24(new), and the
        // panel's retained 0x26 is the correct "old" image for the NEXT
        // partial. Overwriting it with the new frame makes every later
        // Mode-2 a silent no-op. `display()` rewrites BOTH planes anyway.
        self.logical.data_mut().copy_from_slice(self.shaded.data());
        log::info!(
            "frame {} ms partial [{},{},{},{}]p",
            t0.elapsed().as_millis(),
            nx0,
            ny0,
            nx1,
            ny1
        );
    }
}

/// Which portrait region did this local action change, if the update is
/// partial-eligible at all? `None` = full refresh required.
pub fn local_region(
    st: &AppState,
    action: Action,
    changed: bool,
) -> Option<Option<(usize, usize, usize, usize)>> {
    if !changed {
        return None;
    }
    use crate::screens::Screen;
    match st.screen {
        Screen::Home => {
            if !matches!(action, Action::Up | Action::Down) {
                return Some(None);
            }
            Some(Some((16, 130, st.layout.width - 16, 130 + 72 * 4 + 8)))
        }
        Screen::Library => {
            if !matches!(action, Action::Up | Action::Down) {
                return Some(None);
            }
            Some(Some((12, 84, st.layout.width - 12, st.layout.height - 64)))
        }
        Screen::Chapters => {
            if !matches!(action, Action::Up | Action::Down) {
                return Some(None);
            }
            Some(Some((12, 84, st.layout.width - 12, st.layout.height - 64)))
        }
        Screen::Reader => {
            if matches!(action, Action::Up | Action::Down) {
                Some(Some((9, 9, st.layout.width - 9, st.layout.height - 9)))
            } else {
                Some(Some(crate::screens::reader_popup_region(st)))
            }
        }
        Screen::ReaderMenu | Screen::FontSizeMenu => {
            Some(Some(crate::screens::reader_popup_region(st)))
        }
        _ => Some(None),
    }
}

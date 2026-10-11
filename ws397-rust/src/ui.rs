//! Firmware UI layer: reader model (pagination, layout, book access),
//! framebuffer rendering, and the screen state machine. The pagination
//! algorithm is mirrored — and regression-tested — by the host simulator
//! crate `ws397-ui-sim`.

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    Drawable,
};
use u8g2_fonts::{
    fonts,
    types::{FontColor, VerticalPosition},
    FontRenderer,
};

use crate::board::panel;

// ---------------------------------------------------------------- fonts ----

/// One body-font choice (the font-size setting indexes these).
pub struct BodyFont {
    pub renderer: FontRenderer,
}

/// Reader sizes exposed by the font-size menu. The smallest choice starts at
/// the old largest size; medium is the default.
pub fn body_fonts() -> [BodyFont; 4] {
    [
        BodyFont {
            renderer: FontRenderer::new::<fonts::u8g2_font_helvR24_tf>(),
        },
        BodyFont {
            renderer: FontRenderer::new::<fonts::u8g2_font_fur30_tf>(),
        },
        BodyFont {
            renderer: FontRenderer::new::<fonts::u8g2_font_fur35_tf>(),
        },
        BodyFont {
            renderer: FontRenderer::new::<fonts::u8g2_font_fur42_tf>(),
        },
    ]
}

/// UI chrome renderer: compact sans used for footers and menus.
pub fn ui_renderer() -> FontRenderer {
    FontRenderer::new::<fonts::u8g2_font_profont12_tf>()
}
/// Larger reader title/footer font; menu chrome remains compact and scaled.
pub fn reader_chrome_renderer() -> FontRenderer {
    FontRenderer::new::<fonts::u8g2_font_helvR18_tf>()
}

/// Width of `s` in px with `r` (advance width).
pub fn measure(r: &FontRenderer, s: &str) -> usize {
    r.get_rendered_dimensions(s, Point::zero(), VerticalPosition::Top)
        .map(|d| d.advance.x.max(0) as usize)
        .unwrap_or(s.len() * 8)
}

// --------------------------------------------------------------- layout ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// Reading-layout geometry (px) for an orientation + font choice.
#[derive(Clone, Copy)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub font_size: usize,
    pub orientation: Orientation,
    /// Body line pitch in px (font line height + leading).
    pub line_h: usize,
    /// Body text box.
    pub body_x: usize,
    pub body_y: usize,
    pub body_w: usize,
    pub body_h: usize,
}

impl Layout {
    pub fn new(orientation: Orientation, font_size: usize) -> Self {
        let (width, height) = match orientation {
            Orientation::Portrait => (panel::PORTRAIT_WIDTH, panel::PORTRAIT_HEIGHT),
            Orientation::Landscape => (panel::WIDTH, panel::HEIGHT),
        };
        let fonts = body_fonts();
        let base_lh = fonts[font_size.min(3)].renderer.get_default_line_height() as usize;
        let line_h = base_lh + 3;
        let chrome_lh = reader_chrome_renderer().get_default_line_height() as usize;
        let header_h = chrome_lh + 18;
        let footer_h = chrome_lh + 18;
        let m = 8; // outer margin
        let inner = m + 1; // border stroke at margin
        let pad = 6;
        Self {
            width,
            height,
            font_size,
            orientation,
            line_h,
            body_x: inner + pad + 1,
            body_y: inner + header_h + 1 + 4,
            body_w: width.saturating_sub(2 * (inner + 1) + 2 * pad),
            body_h: height.saturating_sub(2 * (inner + 1) + header_h + footer_h + 6),
        }
    }

    pub fn body_lines(&self) -> usize {
        (self.body_h / self.line_h).max(1)
    }
}

// ---------------------------------------------------------------- model ----

/// One chapter of extracted, cleaned text (paragraphs separated by "\n\n").
pub struct Chapter {
    pub title: String,
    pub text: String,
}

pub struct BookText {
    pub chapters: Vec<Chapter>,
}

/// One page: a byte span of one chapter plus chrome metadata.
#[derive(Clone)]
pub struct Page {
    pub chapter: usize,
    pub page: usize,
    pub pages_in_chapter: usize,
    pub start: usize,
    pub end: usize,
    pub progress: f32,
}

impl BookText {
    pub fn total_bytes(&self) -> usize {
        self.chapters.iter().map(|c| c.text.len()).sum()
    }

    /// Paginate one chapter for a layout: paragraph-atomic greedy packing of
    /// wrapped lines. Identical algorithm to the tested simulator.
    pub fn chapter_pages(&self, ch: usize, layout: &Layout) -> Vec<Page> {
        let chapter = &self.chapters[ch];
        let max_lines = layout.body_lines();
        let bytes_before: usize = self.chapters[..ch].iter().map(|c| c.text.len()).sum();
        let total = self.total_bytes().max(1);
        let font = &body_fonts()[layout.font_size.min(3)];

        // Wrap each paragraph, keeping its [start,end) byte span.
        let mut paras: Vec<(usize, usize, usize)> = Vec::new(); // (start, end, lines)
        let mut pos = 0usize;
        for para in chapter.text.split("\n\n") {
            let trimmed = para.trim();
            let tstart = pos + (para.len() - para.trim_start().len());
            let tend = tstart + trimmed.len();
            pos += para.len() + 2;
            if trimmed.is_empty() {
                continue;
            }
            let lines = wrap(&font.renderer, trimmed, layout.body_w).len();
            paras.push((tstart, tend, lines));
        }

        let mut pages: Vec<Page> = Vec::new();
        let mut pending: Option<(usize, usize, usize)> = None; // (start, end, lines)
        for &(s, e, n) in &paras {
            let cur = pending.map(|(_, _, l)| l).unwrap_or(0);
            let fits = pending.is_none() || cur + 1 + n <= max_lines;
            if !fits {
                let (ps, pe, _) = pending.take().unwrap();
                pages.push(Page {
                    chapter: ch,
                    page: 0,
                    pages_in_chapter: 0,
                    start: ps,
                    end: pe,
                    progress: (bytes_before + pe) as f32 / total as f32,
                });
            }
            pending = Some(match pending {
                Some((ps, _, l)) => (ps, e, l + 1 + n),
                None => (s, e, n),
            });
            if pending.map(|(_, _, l)| l >= max_lines).unwrap_or(false) {
                let (ps, pe, _) = pending.take().unwrap();
                pages.push(Page {
                    chapter: ch,
                    page: 0,
                    pages_in_chapter: 0,
                    start: ps,
                    end: pe,
                    progress: (bytes_before + pe) as f32 / total as f32,
                });
            }
        }
        if let Some((ps, pe, _)) = pending {
            pages.push(Page {
                chapter: ch,
                page: 0,
                pages_in_chapter: 0,
                start: ps,
                end: pe,
                progress: (bytes_before + pe) as f32 / total as f32,
            });
        }
        if pages.is_empty() {
            pages.push(Page {
                chapter: ch,
                page: 0,
                pages_in_chapter: 1,
                start: 0,
                end: 0,
                progress: (bytes_before + chapter.text.len()) as f32 / total as f32,
            });
        }
        let n = pages.len();
        for (i, p) in pages.iter_mut().enumerate() {
            p.page = i;
            p.pages_in_chapter = n;
        }
        pages
    }

    /// Paint one reader page (chrome + body) into `fb`.
    pub fn render_page<D>(&self, fb: &mut D, page: &Page, layout: &Layout, book_title: &str)
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        // NOTE: renders opaque black-on-white content only; the CALLER must
        // have cleared the body area first (see screens::render_reader).
        let font = &body_fonts()[layout.font_size.min(3)];
        // Clear the interior so stale glyphs from the previous page vanish
        // (rendering is transparent-pixel only).
        Rectangle::new(
            Point::new(9, 9),
            Size::new((layout.width - 18) as u32, (layout.height - 18) as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(fb)
        .ok();
        // Body first; chrome drawn over it.
        let chapter = &self.chapters[page.chapter];
        let text = &chapter.text[page.start..page.end];
        let mut y = layout.body_y as i32;
        let max_y = (layout.body_y + layout.body_h) as i32;
        for para in text.split("\n\n") {
            let trimmed = para.trim();
            if trimmed.is_empty() {
                continue;
            }
            for line in wrap(&font.renderer, trimmed, layout.body_w) {
                if y + layout.line_h as i32 > max_y {
                    break; // never show a clipped line
                }
                font.renderer
                    .render(
                        line.as_str(),
                        Point::new(layout.body_x as i32, y),
                        VerticalPosition::Top,
                        FontColor::Transparent(BinaryColor::Off),
                        fb,
                    )
                    .ok();
                y += layout.line_h as i32;
            }
        }

        // ---- chrome: outer box, title separator + book title, footer ----
        let m = 8i32;
        let w = layout.width as i32;
        let h = layout.height as i32;
        let outer = Rectangle::new(
            Point::new(m, m),
            Size::new((w - 2 * m) as u32, (h - 2 * m) as u32),
        );
        outer
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::Off, 1))
            .draw(fb)
            .ok();

        let sep_y = layout.body_y as i32 - 6;
        Line::new(Point::new(m + 1, sep_y), Point::new(w - m - 2, sep_y))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::Off, 1))
            .draw(fb)
            .ok();

        let ui = reader_chrome_renderer();
        let title = clip_with(&ui, book_title, layout.body_w.saturating_sub(8));
        ui.render(
            title.as_str(),
            Point::new(layout.body_x as i32, m + 8),
            VerticalPosition::Top,
            FontColor::Transparent(BinaryColor::Off),
            fb,
        )
        .ok();

        let foot_y = h - m - (ui.get_default_line_height() as i32) - 8;
        Line::new(
            Point::new(m + 1, foot_y - 4),
            Point::new(w - m - 2, foot_y - 4),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::Off, 1))
        .draw(fb)
        .ok();
        let footer = format!(
            "Ch {}  p {}/{}   {:.1}%",
            page.chapter + 1,
            page.page + 1,
            page.pages_in_chapter,
            page.progress * 100.0
        );
        ui.render(
            footer.as_str(),
            Point::new(layout.body_x as i32, foot_y + 2),
            VerticalPosition::Top,
            FontColor::Transparent(BinaryColor::Off),
            fb,
        )
        .ok();
    }
}

// ------------------------------------------------------------- wrapping ----

/// Greedy word wrap to `max_px`. Overlong single words split on char
/// boundaries at the px limit.
pub fn wrap(r: &FontRenderer, para: &str, max_px: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    let space_w = measure(r, " ").max(1);
    for word in para.split_whitespace() {
        let ww = measure(r, word);
        if cur.is_empty() {
            cur.push_str(word);
            cur_w = ww;
        } else if cur_w + space_w + ww <= max_px {
            cur.push(' ');
            cur.push_str(word);
            cur_w += space_w + ww;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur_w = ww;
            cur.push_str(word);
        }
        while cur_w > max_px {
            let mut split = cur.len();
            while split > 1 && measure(r, &cur[..split]) > max_px {
                split -= 1;
            }
            let rest = cur.split_off(split);
            lines.push(std::mem::take(&mut cur));
            cur = rest;
            cur_w = measure(r, &cur);
            if cur.is_empty() {
                break;
            }
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Clip `s` to `max_px` with the specified renderer, appending `..` when cut.
pub fn clip_with(r: &FontRenderer, s: &str, max_px: usize) -> String {
    if measure(r, s) <= max_px {
        return s.to_string();
    }
    let mut out = s.to_string();
    while out.len() > 1 && measure(r, &format!("{}..", out)) > max_px {
        let mut cut = out.len() - 1;
        while cut > 1 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out.push_str("..");
    out
}

/// Clip menu text using the menu renderer.
pub fn clip(s: &str, max_px: usize) -> String {
    clip_with(&ui_renderer(), s, max_px)
}

//! Screen state machine: Home (menu), Reader, Chapter list, and the Settings
//! stubs. All input is the 3-way rocker + BOOT; rendering paints full frames
//! into the caller-provided framebuffer (in logical orientation).

use embedded_graphics::{
    framebuffer::Framebuffer,
    image::GetPixel,
    pixelcolor::{
        raw::{BigEndian, RawU1},
        BinaryColor,
    },
    prelude::*,
    primitives::{PrimitiveStyleBuilder, Rectangle},
    Drawable,
};
use u8g2_fonts::FontRenderer;

use crate::board::BookEntry;
use crate::ui::{self, Layout, Orientation, Page};

/// UI scale: fonts render 1x into a scratch, then nearest-neighbour upscale.
const SCALE: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq, core::fmt::Debug)]
pub enum Action {
    Up,
    Down,
    Press,     // rocker press (short)
    LongPress, // rocker press held >= 2 s
    Back,      // BOOT tap
}

/// Dirty rectangle in logical (portrait) space. Renderers union every region
/// they touch into it; `main.rs` rotates it into native coordinates and
/// partial-refreshes ONLY that window (project rule: no whole-frame pushes
/// for incremental updates).
#[derive(Clone, Copy)]
pub struct Dirty {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Dirty {
    pub fn empty() -> Self {
        Self {
            x0: i32::MAX,
            y0: i32::MAX,
            x1: i32::MIN,
            y1: i32::MIN,
        }
    }
    pub fn cover(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x + w);
        self.y1 = self.y1.max(y + h);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Library,
    Reader,
    ReaderMenu,
    FontSizeMenu,
    Chapters,
    Wifi,
    Transcript,
}

pub const HOME_ITEMS: [&str; 4] = ["Read Book", "Wi-Fi Settings", "Transcript Mode", "About"];

pub struct AppState {
    pub screen: Screen,
    pub menu_idx: usize,
    pub library_idx: usize,
    pub reader_menu_idx: usize,
    pub font_menu_idx: usize,
    pub books: Vec<BookEntry>,
    pub text: Option<crate::ui::BookText>,
    pub book_title: String,
    pub layout: Layout,
    pub pages: Vec<Page>,
    pub page_idx: usize,
    pub chapter_idx: usize,
}

/// Logical drawing target backed by the fixed 480x800 framebuffer. Landscape
/// pixels are stored rotated so the existing native gather path can stream
/// them directly to the 800x480 panel.
struct LogicalFb<'a> {
    fb: &'a mut crate::ssd1677::PortraitFb,
    orientation: Orientation,
}

impl OriginDimensions for LogicalFb<'_> {
    fn size(&self) -> Size {
        match self.orientation {
            Orientation::Portrait => Size::new(
                crate::board::panel::PORTRAIT_WIDTH as u32,
                crate::board::panel::PORTRAIT_HEIGHT as u32,
            ),
            Orientation::Landscape => Size::new(
                crate::board::panel::WIDTH as u32,
                crate::board::panel::HEIGHT as u32,
            ),
        }
    }
}

impl DrawTarget for LogicalFb<'_> {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let orientation = self.orientation;
        self.fb
            .draw_iter(pixels.into_iter().filter_map(move |Pixel(p, color)| {
                let (width, height) = match orientation {
                    Orientation::Portrait => (
                        crate::board::panel::PORTRAIT_WIDTH as i32,
                        crate::board::panel::PORTRAIT_HEIGHT as i32,
                    ),
                    Orientation::Landscape => (
                        crate::board::panel::WIDTH as i32,
                        crate::board::panel::HEIGHT as i32,
                    ),
                };
                if p.x < 0 || p.y < 0 || p.x >= width || p.y >= height {
                    return None;
                }
                let stored = match orientation {
                    Orientation::Portrait => p,
                    Orientation::Landscape => {
                        Point::new(p.y, crate::board::panel::PORTRAIT_HEIGHT as i32 - 1 - p.x)
                    }
                };
                Some(Pixel(stored, color))
            }))
    }
}

/// Render the current screen into `fb` (logical portrait space), recording
/// every touched region in `d`. `full=true` clears first (screen change /
/// anti-ghosting flush); otherwise previous content stays and only the
/// union-of-touch window needs pushing.
pub fn render(fb: &mut crate::ssd1677::PortraitFb, st: &AppState, d: &mut Dirty, full: bool) {
    let mut target = LogicalFb {
        fb,
        orientation: st.layout.orientation,
    };
    if full {
        d.cover(0, 0, st.layout.width as i32, st.layout.height as i32);
        clear(&mut target, st.layout.width, st.layout.height);
    }
    match st.screen {
        Screen::Home => render_home(&mut target, st, d),
        Screen::Library => render_library(&mut target, st, d),
        Screen::Reader => render_reader(&mut target, st, d),
        Screen::ReaderMenu => render_reader_menu(&mut target, st, d),
        Screen::FontSizeMenu => render_font_size_menu(&mut target, st, d),
        Screen::Chapters => render_chapters(&mut target, st, d),
        Screen::Wifi => render_stub(
            &mut target,
            st.layout.width,
            st.layout.height,
            "Wi-Fi Settings",
            d,
        ),
        Screen::Transcript => render_stub(
            &mut target,
            st.layout.width,
            st.layout.height,
            "Transcript Mode",
            d,
        ),
    }
}

fn clear(fb: &mut impl DrawTarget<Color = BinaryColor>, w: usize, h: usize) {
    // whole surface -> caller already marked Dirty::full equivalent
    Rectangle::new(Point::zero(), Size::new(w as u32, h as u32))
        .into_styled(
            PrimitiveStyleBuilder::new()
                .fill_color(BinaryColor::On)
                .build(),
        )
        .draw(fb)
        .ok();
}

/// Scaled text: render 1x into a strip, upscale `SCALE`x with an `Image`
/// blit. `invert` swaps fg/bg for selection bars.
fn text(
    fb: &mut impl DrawTarget<Color = BinaryColor>,
    r: &FontRenderer,
    s: &str,
    at: Point,
    invert: bool,
    d: &mut Dirty,
) {
    const SW: usize = 1024;
    const SH: usize = 64;
    type Strip = Framebuffer<BinaryColor, RawU1, BigEndian, SW, SH, { SW * SH / 8 }>;
    let lh = r.get_default_line_height() as usize;
    let sw = ui::measure(r, s).saturating_add(2).min(SW);
    if sw == 0 || lh == 0 || lh > SH {
        return;
    }
    let mut strip = Box::new(Strip::new());
    let fill = if invert { 0x00 } else { 0xFF };
    for b in strip.data_mut().iter_mut() {
        *b = fill;
    }
    let glyph = if invert {
        BinaryColor::On
    } else {
        BinaryColor::Off
    };
    r.render(
        s,
        Point::new(0, 0),
        u8g2_fonts::types::VerticalPosition::Top,
        u8g2_fonts::types::FontColor::Transparent(glyph),
        &mut *strip,
    )
    .ok();
    d.cover(
        at.x,
        at.y,
        (sw * SCALE as usize) as i32,
        (lh * SCALE as usize) as i32,
    );
    for sy in 0..lh as u32 {
        for sx in 0..sw as u32 {
            let p = (*strip).pixel(Point::new(sx as i32, sy as i32));
            let want = if invert {
                BinaryColor::On
            } else {
                BinaryColor::Off
            };
            if p == Some(want) {
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        fb.draw_iter(core::iter::once(Pixel(
                            at + Point::new((sx * SCALE + dx) as i32, (sy * SCALE + dy) as i32),
                            if invert {
                                BinaryColor::On
                            } else {
                                BinaryColor::Off
                            },
                        )))
                        .ok();
                    }
                }
            }
        }
    }
}

fn frame(fb: &mut impl DrawTarget<Color = BinaryColor>, w: usize, h: usize, d: &mut Dirty) {
    d.cover(8, 8, w as i32 - 16, h as i32 - 16);
    Rectangle::new(Point::new(8, 8), Size::new(w as u32 - 16, h as u32 - 16))
        .into_styled(
            PrimitiveStyleBuilder::new()
                .stroke_color(BinaryColor::Off)
                .stroke_width(1)
                .build(),
        )
        .draw(fb)
        .ok();
}

fn header(
    fb: &mut impl DrawTarget<Color = BinaryColor>,
    r: &FontRenderer,
    title: &str,
    d: &mut Dirty,
) {
    let t = ui::clip(title, 480 - 60);
    text(fb, r, &t, Point::new(24, 20), false, d);
}

fn render_home(fb: &mut impl DrawTarget<Color = BinaryColor>, st: &AppState, d: &mut Dirty) {
    let (w, h) = (st.layout.width, st.layout.height);
    frame(fb, w, h, d);
    let r = ui::ui_renderer();
    header(fb, &r, "ws397 reader", d);
    let lh = r.get_default_line_height() as usize * SCALE as usize + 12;
    // Selection can move across the whole list; dirty the menu band.
    d.cover(20, 136, w as i32 - 40, (lh * HOME_ITEMS.len()) as i32 + 8);
    let mut y = 140i32;
    for (i, item) in HOME_ITEMS.iter().enumerate() {
        if i == st.menu_idx {
            let tw = ui::measure(&r, item) * SCALE as usize + 24;
            d.cover(20, y - 4, tw as i32, (lh - 4) as i32);
            Rectangle::new(
                Point::new(20, y as i32 - 4),
                Size::new(tw as u32, (lh - 4) as u32),
            )
            .into_styled(
                PrimitiveStyleBuilder::new()
                    .fill_color(BinaryColor::Off)
                    .build(),
            )
            .draw(fb)
            .ok();
            text(fb, &r, item, Point::new(32, y), true, d);
        } else {
            text(fb, &r, item, Point::new(32, y), false, d);
        }
        y += lh as i32;
    }
    text(
        fb,
        &r,
        HOME_HINT,
        Point::new(
            ((w - ui::measure(&r, HOME_HINT) * SCALE as usize) / 2) as i32,
            h as i32 - 56,
        ),
        false,
        d,
    );
}

const HOME_HINT: &str = "up/down select  press ok";

fn render_library(fb: &mut impl DrawTarget<Color = BinaryColor>, st: &AppState, d: &mut Dirty) {
    let (w, h) = (st.layout.width, st.layout.height);
    frame(fb, w, h, d);
    let r = ui::ui_renderer();
    header(fb, &r, "Book Library", d);
    let row_h = r.get_default_line_height() as usize * SCALE as usize + 10;
    let visible = ((h - 150) / row_h).max(1);
    let top = st.library_idx.saturating_sub(visible / 2);
    for (slot, book) in st.books.iter().enumerate().skip(top).take(visible) {
        let y = 90 + (slot - top) as i32 * row_h as i32;
        let title = ui::clip(book.name.trim_end_matches(".epub"), w - 72);
        if slot == st.library_idx {
            Rectangle::new(
                Point::new(16, y - 4),
                Size::new((w - 32) as u32, (row_h - 4) as u32),
            )
            .into_styled(
                PrimitiveStyleBuilder::new()
                    .fill_color(BinaryColor::Off)
                    .build(),
            )
            .draw(fb)
            .ok();
            text(fb, &r, &title, Point::new(24, y), true, d);
        } else {
            text(fb, &r, &title, Point::new(24, y), false, d);
        }
    }
    d.cover(16, 84, w as i32 - 32, (visible * row_h) as i32 + 8);
    text(
        fb,
        &r,
        "up/down  press open  boot back",
        Point::new(24, h as i32 - 56),
        false,
        d,
    );
}

fn render_stub(
    fb: &mut impl DrawTarget<Color = BinaryColor>,
    w: usize,
    h: usize,
    title: &str,
    d: &mut Dirty,
) {
    frame(fb, w, h, d);
    let r = ui::ui_renderer();
    header(fb, &r, title, d);
    text(fb, &r, "Coming soon", Point::new(28, 140), false, d);
    text(
        fb,
        &r,
        "BOOT: back",
        Point::new(28, h as i32 - 56),
        false,
        d,
    );
}

fn render_chapters(fb: &mut impl DrawTarget<Color = BinaryColor>, st: &AppState, d: &mut Dirty) {
    let (w, h) = (st.layout.width, st.layout.height);
    frame(fb, w, h, d);
    let Some(text_data) = &st.text else { return };
    let r = ui::ui_renderer();
    header(fb, &r, &st.book_title, d);
    let row_h = r.get_default_line_height() as usize * SCALE as usize + 6;
    let visible = (h - 150) / row_h;
    let sel = st.chapter_idx;
    let top = sel.saturating_sub(visible / 2);
    let y = 90 + row_h as i32 / 2;
    for (i, ch) in text_data
        .chapters
        .iter()
        .enumerate()
        .skip(top)
        .take(visible)
    {
        let yy = y + (i - top) as i32 * row_h as i32;
        if i == sel {
            d.cover(16, yy - 2, w as i32 - 32, (row_h - 2) as i32);
            Rectangle::new(
                Point::new(16, yy - 2),
                Size::new(w as u32 - 32, (row_h - 2) as u32),
            )
            .into_styled(
                PrimitiveStyleBuilder::new()
                    .fill_color(BinaryColor::Off)
                    .build(),
            )
            .draw(fb)
            .ok();
            text(fb, &r, &ch.title, Point::new(24, yy), true, d);
        } else {
            let t = ui::clip(&ch.title, w - 64);
            text(fb, &r, &t, Point::new(24, yy), false, d);
        }
    }
    // Selection may have moved from anywhere in the list; dirty the list band.
    d.cover(
        16,
        90,
        w as i32 - 32,
        (visible * row_h) as i32 + row_h as i32,
    );
    let hint = format!("up/down  press go  boot back  long: rotate",);
    text(fb, &r, &hint, Point::new(24, h as i32 - 56), false, d);
}

fn render_reader(fb: &mut impl DrawTarget<Color = BinaryColor>, st: &AppState, d: &mut Dirty) {
    let Some(text_data) = &st.text else { return };
    let Some(page) = st.pages.get(st.page_idx) else {
        return;
    };
    // render_page clears the interior itself; the whole inner area changed.
    d.cover(
        9,
        9,
        st.layout.width as i32 - 18,
        st.layout.height as i32 - 18,
    );
    text_data.render_page(fb, page, &st.layout, &st.book_title);
}
const READER_MENU_ITEMS: [&str; 4] = ["chapter select", "font-size", "back", "close menu"];
const FONT_SIZE_ITEMS: [&str; 4] = ["small", "medium", "large", "xlarge"];

fn popup_geometry(st: &AppState) -> (usize, usize, usize, usize, usize) {
    let r = ui::ui_renderer();
    let row_h = r.get_default_line_height() as usize * SCALE as usize + 12;
    let box_w = st.layout.width.saturating_sub(48).min(400);
    let box_h = 32 + row_h * 5;
    let x = (st.layout.width - box_w) / 2;
    let y = (st.layout.height - box_h) / 2;
    (x, y, box_w, box_h, row_h)
}

pub fn reader_popup_region(st: &AppState) -> (usize, usize, usize, usize) {
    let (x, y, w, h, _) = popup_geometry(st);
    (x, y, x + w, y + h)
}

fn render_popup(
    fb: &mut impl DrawTarget<Color = BinaryColor>,
    st: &AppState,
    d: &mut Dirty,
    title: &str,
    items: &[&str],
    selected: usize,
) {
    let (x, y, w, h, row_h) = popup_geometry(st);
    d.cover(x as i32, y as i32, w as i32, h as i32);
    Rectangle::new(
        Point::new(x as i32, y as i32),
        Size::new(w as u32, h as u32),
    )
    .into_styled(
        PrimitiveStyleBuilder::new()
            .fill_color(BinaryColor::On)
            .stroke_color(BinaryColor::Off)
            .stroke_width(2)
            .build(),
    )
    .draw(fb)
    .ok();

    let r = ui::ui_renderer();
    text(
        fb,
        &r,
        title,
        Point::new((x + 16) as i32, (y + 12) as i32),
        false,
        d,
    );
    for (i, item) in items.iter().enumerate() {
        let row_y = y + 20 + row_h * (i + 1);
        if i == selected {
            Rectangle::new(
                Point::new((x + 10) as i32, (row_y - 5) as i32),
                Size::new((w - 20) as u32, (row_h - 2) as u32),
            )
            .into_styled(
                PrimitiveStyleBuilder::new()
                    .fill_color(BinaryColor::Off)
                    .build(),
            )
            .draw(fb)
            .ok();
        }
        text(
            fb,
            &r,
            item,
            Point::new((x + 20) as i32, row_y as i32),
            i == selected,
            d,
        );
    }
}

fn render_reader_menu(fb: &mut impl DrawTarget<Color = BinaryColor>, st: &AppState, d: &mut Dirty) {
    render_reader(fb, st, d);
    render_popup(
        fb,
        st,
        d,
        "reader menu",
        &READER_MENU_ITEMS,
        st.reader_menu_idx,
    );
}

fn render_font_size_menu(
    fb: &mut impl DrawTarget<Color = BinaryColor>,
    st: &AppState,
    d: &mut Dirty,
) {
    render_reader(fb, st, d);
    render_popup(fb, st, d, "font-size", &FONT_SIZE_ITEMS, st.font_menu_idx);
}

/// Apply an action; returns true when the display must refresh.
pub fn handle(st: &mut AppState, a: Action) -> bool {
    match st.screen {
        Screen::Home => match a {
            Action::Up => {
                st.menu_idx = (st.menu_idx + HOME_ITEMS.len() - 1) % HOME_ITEMS.len();
                true
            }
            Action::Down => {
                st.menu_idx = (st.menu_idx + 1) % HOME_ITEMS.len();
                true
            }
            Action::Press => {
                match st.menu_idx {
                    0 => {
                        if st.books.is_empty() {
                            return false;
                        }
                        st.library_idx = st.library_idx.min(st.books.len() - 1);
                        st.screen = Screen::Library;
                    }
                    1 => st.screen = Screen::Wifi,
                    2 => st.screen = Screen::Transcript,
                    _ => {}
                }
                true
            }
            Action::Back | Action::LongPress => false,
        },
        Screen::Library => match a {
            Action::Up => {
                if st.books.is_empty() {
                    false
                } else {
                    st.library_idx = (st.library_idx + st.books.len() - 1) % st.books.len();
                    true
                }
            }
            Action::Down => {
                if st.books.is_empty() {
                    false
                } else {
                    st.library_idx = (st.library_idx + 1) % st.books.len();
                    true
                }
            }
            Action::Press => {
                let selected = st.library_idx;
                open_book(st, selected);
                if st.text.is_some() {
                    st.screen = Screen::Reader;
                    true
                } else {
                    false
                }
            }
            Action::Back => {
                st.screen = Screen::Home;
                true
            }
            Action::LongPress => false,
        },
        Screen::Reader => match a {
            Action::Up => {
                if st.page_idx > 0 {
                    st.page_idx -= 1;
                    true
                } else if st.chapter_idx > 0 {
                    goto_chapter(st, st.chapter_idx - 1);
                    true
                } else {
                    false
                }
            }
            Action::Down => {
                let nch = st.text.as_ref().map_or(0, |t| t.chapters.len());
                if st.page_idx + 1 < st.pages.len() {
                    st.page_idx += 1;
                    true
                } else if st.chapter_idx + 1 < nch {
                    goto_chapter(st, st.chapter_idx + 1);
                    true
                } else {
                    false
                }
            }
            Action::LongPress => {
                st.reader_menu_idx = 0;
                st.screen = Screen::ReaderMenu;
                true
            }
            Action::Back => {
                st.screen = Screen::Home;
                true
            }
            Action::Press => false,
        },
        Screen::ReaderMenu => match a {
            Action::Up => {
                st.reader_menu_idx =
                    (st.reader_menu_idx + READER_MENU_ITEMS.len() - 1) % READER_MENU_ITEMS.len();
                true
            }
            Action::Down => {
                st.reader_menu_idx = (st.reader_menu_idx + 1) % READER_MENU_ITEMS.len();
                true
            }
            Action::Press => {
                match st.reader_menu_idx {
                    0 => st.screen = Screen::Chapters,
                    1 => {
                        st.font_menu_idx = st.layout.font_size;
                        st.screen = Screen::FontSizeMenu;
                    }
                    2 => st.screen = Screen::Library,
                    _ => st.screen = Screen::Reader,
                }
                true
            }
            Action::Back | Action::LongPress => {
                st.screen = Screen::Reader;
                true
            }
        },
        Screen::FontSizeMenu => match a {
            Action::Up => {
                st.font_menu_idx =
                    (st.font_menu_idx + FONT_SIZE_ITEMS.len() - 1) % FONT_SIZE_ITEMS.len();
                true
            }
            Action::Down => {
                st.font_menu_idx = (st.font_menu_idx + 1) % FONT_SIZE_ITEMS.len();
                true
            }
            Action::Press => {
                st.layout = Layout::new(st.layout.orientation, st.font_menu_idx);
                repaginate_at_current(st);
                st.screen = Screen::ReaderMenu;
                true
            }
            Action::Back => {
                st.screen = Screen::ReaderMenu;
                true
            }
            Action::LongPress => {
                st.screen = Screen::Reader;
                true
            }
        },
        Screen::Chapters => match a {
            Action::Up => {
                if st.chapter_idx > 0 {
                    st.chapter_idx -= 1;
                    true
                } else {
                    false
                }
            }
            Action::Down => {
                if st.chapter_idx + 1 < st.text.as_ref().map_or(0, |t| t.chapters.len()) {
                    st.chapter_idx += 1;
                    true
                } else {
                    false
                }
            }
            Action::Press => {
                let selected = st.chapter_idx;
                goto_chapter(st, selected);
                st.screen = Screen::Reader;
                true
            }
            Action::LongPress => {
                toggle_rotation(st);
                true
            }
            Action::Back => {
                repaginate_at_current(st);
                st.screen = Screen::Reader;
                true
            }
        },
        Screen::Wifi | Screen::Transcript => match a {
            Action::Back | Action::Press | Action::LongPress => {
                st.screen = Screen::Home;
                true
            }
            _ => false,
        },
    }
}

pub fn open_book(st: &mut AppState, idx: usize) {
    let Some(entry) = st.books.get(idx) else {
        return;
    };
    match crate::board::decode_book(entry) {
        Some((title, text)) => {
            log::info!("opened '{}' ({} chapters)", title, text.chapters.len());
            st.book_title = title;
            st.text = Some(text);
            st.chapter_idx = 0;
            st.page_idx = 0;
            repaginate_at_current(st);
        }
        None => log::error!("invalid compiled book {}", entry.name),
    }
}

pub fn goto_chapter(st: &mut AppState, ch: usize) {
    st.chapter_idx = ch;
    st.page_idx = 0;
    repaginate_at_current(st);
}

fn repaginate_at_current(st: &mut AppState) {
    if let Some(t) = &st.text {
        st.pages = t.chapter_pages(st.chapter_idx, &st.layout);
        st.page_idx = st.page_idx.min(st.pages.len().saturating_sub(1));
    }
}

pub fn toggle_rotation(st: &mut AppState) {
    st.layout.orientation = match st.layout.orientation {
        Orientation::Portrait => Orientation::Landscape,
        Orientation::Landscape => Orientation::Portrait,
    };
    st.layout = Layout::new(st.layout.orientation, st.layout.font_size);
    repaginate_at_current(st);
}

//! Firmware device layer: panel geometry, pins (e-paper + rocker), embedded
//! book library.

/// Display panel geometry (native landscape orientation, as the controller
/// addresses RAM).
pub mod panel {
    /// Native controller RAM geometry (landscape). `display()` always streams
    /// a buffer laid out for this; portrait is a software rotation into it.
    pub const WIDTH: usize = 800;
    pub const HEIGHT: usize = 480;
    /// Bytes in one 1-bpp framebuffer: 800*480/8 = 48_000.
    pub const FRAME_BYTES: usize = WIDTH * HEIGHT / 8;

    /// Portrait logical geometry (90° software rotation of the native buffer).
    /// The reader draws in portrait; rotation happens before refresh.
    pub const PORTRAIT_WIDTH: usize = HEIGHT; // 480
    pub const PORTRAIT_HEIGHT: usize = WIDTH; // 800
    pub const PORTRAIT_BYTES: usize = PORTRAIT_WIDTH * PORTRAIT_HEIGHT / 8;
}

/// All e-paper control GPIOs, type-erased to `Any*Pin` so the driver does not
/// bake in a concrete GPIO type per pin. CS is software-controlled (the panel
/// latches DC on the next clock edge; CS must stay asserted across a
/// command+data pair — see the driver's byte writer).
/// The board's rocker: three independent active-low switches with external/
/// internal pull-ups — verified against vendor `button_bsp.c` for this board
/// (`ESP-IDF/08_.../components/button_bsp`), 2026-10-10: it is NOT an encoder.
/// GPIO0 is the side BOOT button (also the download-mode strap).
/// Embedded EPUBs are extracted to a compact book blob by build.rs. Runtime
/// opening only decodes lengths and UTF-8; ZIP/XML work never blocks the UI.
#[derive(Clone, Copy)]
pub struct BookEntry {
    pub name: &'static str,
    pub data: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/book_index.rs"));

pub fn embedded_books() -> Vec<BookEntry> {
    EMBEDDED_BOOKS.to_vec()
}

fn take_u32(data: &[u8], offset: &mut usize) -> Option<usize> {
    let bytes: [u8; 4] = data.get(*offset..*offset + 4)?.try_into().ok()?;
    *offset += 4;
    Some(u32::from_le_bytes(bytes) as usize)
}

fn take_str(data: &'static [u8], offset: &mut usize) -> Option<&'static str> {
    let len = take_u32(data, offset)?;
    let bytes = data.get(*offset..*offset + len)?;
    *offset += len;
    core::str::from_utf8(bytes).ok()
}

pub fn decode_book(entry: &BookEntry) -> Option<(String, crate::ui::BookText)> {
    let data = entry.data;
    if data.get(..5)? != b"WSBK\x01" {
        return None;
    }
    let mut offset = 5;
    let title = take_str(data, &mut offset)?.to_owned();
    let count = take_u32(data, &mut offset)?;
    let mut chapters = Vec::with_capacity(count);
    for _ in 0..count {
        chapters.push(crate::ui::Chapter {
            title: take_str(data, &mut offset)?.to_owned(),
            text: take_str(data, &mut offset)?.to_owned(),
        });
    }
    (offset == data.len()).then_some((title, crate::ui::BookText { chapters }))
}

//! Layout geometry: maps pixel sizes to a readable grid. The device build
//! measures text with u8g2 fonts; the simulator approximates with a fixed
//! char cell. Keeping the *page model* (box margins, line count) here means
//! pagination behaviour is tested on the host against the same logic.

/// A reading-layout configuration.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// Panel width in pixels (after rotation).
    pub width_px: usize,
    /// Panel height in pixels.
    pub height_px: usize,
    /// Box margin from panel edge.
    pub margin: usize,
    /// Header (title) box height in px.
    pub header_h: usize,
    /// Footer (progress) box height in px.
    pub footer_h: usize,
    /// Body text cell height (line height) in px.
    pub line_h: usize,
    /// Average character width for wrapping, in px (font-dependent).
    pub char_w: usize,
}

impl Layout {
    /// Portrait 480x800 with a ~10x20-ish body at the default font size.
    pub fn portrait_default() -> Self {
        Self {
            width_px: 480,
            height_px: 800,
            margin: 8,
            header_h: 28,
            footer_h: 24,
            line_h: 20,
            char_w: 10,
        }
    }

    /// Landscape 800x480.
    pub fn landscape_default() -> Self {
        Self {
            width_px: 800,
            height_px: 480,
            ..Self::portrait_default()
        }
    }

    pub fn with_line_h(mut self, h: usize, char_w: usize) -> Self {
        self.line_h = h;
        self.char_w = char_w;
        self
    }

    /// Body text box: inside the outer border, under the header, above the
    /// footer. Border drawn 1px inside `margin`.
    pub fn body_rect_px(&self) -> (usize, usize, usize, usize) {
        let inner = self.margin + 1;
        let x = inner + 1;
        let y = inner + 1 + self.header_h + 1;
        let w = self.width_px - 2 * (inner + 1);
        let h = self.height_px - 2 * (inner + 1) - self.header_h - self.footer_h - 2;
        (x, y, w, h)
    }

    /// How many body lines fit.
    pub fn body_lines(&self) -> usize {
        let (_, _, _, h) = self.body_rect_px();
        h / self.line_h
    }

    /// How many characters fit per line (minus a small inner padding).
    pub fn width_chars(&self) -> usize {
        let (_, _, w, _) = self.body_rect_px();
        (w - 4) / self.char_w
    }

    /// Greedy word-wrap one paragraph to `width_chars()`. Matches
    /// `embedded-text`'s word-boundary behaviour for Latin text closely
    /// enough for host validation.
    pub fn wrap(&self, para: &str) -> Vec<String> {
        let max = self.width_chars();
        let mut lines = Vec::new();
        let mut cur = String::new();
        for word in para.split_whitespace() {
            if cur.is_empty() {
                cur.push_str(word);
            } else if cur.len() + 1 + word.len() <= max {
                cur.push(' ');
                cur.push_str(word);
            } else {
                lines.push(std::mem::take(&mut cur));
                cur.push_str(word);
            }
            // A single word longer than the line: hard-split it.
            while cur.len() > max {
                let mut split = max;
                for i in (0..=max).rev() {
                    if cur.is_char_boundary(i) {
                        split = i;
                        break;
                    }
                }
                let rest = cur.split_off(split);
                lines.push(std::mem::take(&mut cur));
                cur = rest;
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }
}

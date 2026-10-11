//! UI layer shared by the host simulator and the firmware.
//!
//! Design: pure-data layout + pagination. Rendering to a framebuffer (device)
//! or to text (simulator) are separate backends over the same `Page` data.
//! Pagination walks paragraphs, wrapping each with the layout and tracking
//! *source byte offsets* — so page boundaries are exact and progress is a
//! simple consumed-bytes ratio.

use std::io::Write;

pub mod epub;
pub mod layout;

use layout::Layout;

/// One chapter of extracted, cleaned text (paragraphs separated by "\n\n").
#[derive(Debug, Clone)]
pub struct Chapter {
    pub title: String,
    /// Cleaned text; byte offsets below are valid into this string.
    pub text: String,
}

/// A paginated book: chapters of text with lazy page indices.
pub struct BookText {
    pub chapters: Vec<Chapter>,
}

/// One rendered page: a span of one chapter plus chrome metadata.
#[derive(Debug, Clone)]
pub struct Page<'a> {
    pub chapter: usize,
    /// Page number within the chapter (0-based).
    pub page: usize,
    /// Total pages in the chapter at this layout.
    pub pages_in_chapter: usize,
    /// First byte of the chapter span shown on this page.
    pub start: usize,
    /// End byte (exclusive) of the chapter span shown on this page.
    pub end: usize,
    /// Global progress through the whole book: bytes displayed / total bytes.
    pub progress: f32,
    /// Pre-wrapped display lines (includes blank paragraph spacers).
    pub lines: Vec<String>,
    /// The chapter title for the header box.
    pub title: &'a str,
}

impl BookText {
    pub fn total_bytes(&self) -> usize {
        self.chapters.iter().map(|c| c.text.len()).sum()
    }

    /// Paginate one chapter for the given layout (lazy: only this chapter is
    /// laid out). Paragraph-atomic: a paragraph that doesn't fit the page
    /// moves whole to the next page (no mid-paragraph page breaks).
    pub fn chapter_pages(&self, ch: usize, layout: &Layout) -> Vec<Page<'_>> {
        let chapter = &self.chapters[ch];
        let max_lines = layout.body_lines();
        let bytes_before: usize = self.chapters[..ch].iter().map(|c| c.text.len()).sum();
        let total = self.total_bytes().max(1);

        // Wrap each paragraph, remembering its [start,end) byte span.
        struct Para {
            start: usize,
            end: usize,
            lines: Vec<String>,
        }
        let mut paras: Vec<Para> = Vec::new();
        let mut pos = 0usize;
        for para in chapter.text.split("\n\n") {
            let trimmed = para.trim();
            let tstart = pos + (para.len() - para.trim_start().len());
            let tend = tstart + trimmed.len();
            pos += para.len() + 2; // "\n\n"
            if trimmed.is_empty() {
                continue;
            }
            paras.push(Para {
                start: tstart,
                end: tend,
                lines: layout.wrap(trimmed),
            });
        }

        let mut pages: Vec<Page> = Vec::new();
        // The page under construction: Some((start, end, lines)).
        let mut pending: Option<(usize, usize, Vec<String>)> = None;

        for p in &paras {
            let cur_len = match &pending {
                Some((_, _, l)) => l.len(),
                None => 0,
            };
            // Appending p adds a paragraph spacer when the page is non-empty.
            if pending.is_some() && cur_len + 1 + p.lines.len() > max_lines {
                pages.push(flush_page(&mut pending, ch, bytes_before, total, &chapter.title));
            }
            match &mut pending {
                Some((start, end, lines)) => {
                    if !lines.is_empty() {
                        lines.push(String::new()); // paragraph spacer
                    }
                    *end = p.end;
                    lines.extend(p.lines.iter().cloned());
                    let _ = start;
                }
                None => {
                    pending = Some((p.start, p.end, p.lines.clone()));
                }
            }
            // A paragraph that alone fills the page flushes immediately; the
            // renderer clips any overflow to the body height.
            if let Some((_, _, l)) = &pending {
                if l.len() > max_lines {
                    pages.push(flush_page(&mut pending, ch, bytes_before, total, &chapter.title));
                }
            }
        }
        if pending.is_some() {
            pages.push(flush_page(&mut pending, ch, bytes_before, total, &chapter.title));
        }

        if pages.is_empty() {
            pages.push(Page {
                chapter: ch,
                page: 0,
                pages_in_chapter: 1,
                start: 0,
                end: 0,
                progress: (bytes_before + chapter.text.len()) as f32 / total as f32,
                lines: Vec::new(),
                title: &chapter.title,
            });
        }
        let n = pages.len();
        for (i, p) in pages.iter_mut().enumerate() {
            p.page = i;
            p.pages_in_chapter = n;
        }
        pages
    }
}

/// Turn the pending page into a `Page`, fixing ordering metadata.
fn flush_page<'a>(
    pending: &mut Option<(usize, usize, Vec<String>)>,
    ch: usize,
    bytes_before: usize,
    total: usize,
    title: &'a str,
) -> Page<'a> {
    let (start, end, lines) = pending.take().expect("pending page");
    Page {
        chapter: ch,
        page: usize::MAX, // fixed up by the caller at the end
        pages_in_chapter: 0,
        start,
        end,
        progress: (bytes_before + end) as f32 / total as f32,
        lines,
        title,
    }
}

/// Render a page as text (simulator backend): a `+----+` box with a title
/// area, body lines, and a footer progress line.
pub fn render_text(out: &mut impl Write, page: &Page, layout: &Layout) {
    let w = layout.width_chars();
    let sep = format!("+{}+", "-".repeat(w));
    writeln!(out, "{sep}").unwrap();
    writeln!(out, "|{:w$}|", format!(" {} ", page.title), w = w).unwrap();
    writeln!(out, "{sep}").unwrap();
    for l in page.lines.iter().take(layout.body_lines()) {
        writeln!(out, "|{:w$}|", format!(" {l}"), w = w).unwrap();
    }
    writeln!(out, "{sep}").unwrap();
    let footer = format!(
        " {}/{} ch{}  {:.1}%",
        page.page + 1,
        page.pages_in_chapter,
        page.chapter + 1,
        page.progress * 100.0
    );
    writeln!(out, "|{:w$}|", footer, w = w).unwrap();
    writeln!(out, "{sep}").unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bt(paras: &[String]) -> BookText {
        BookText {
            chapters: vec![Chapter {
                title: "T".into(),
                text: paras.join("\n\n"),
            }],
        }
    }

    #[test]
    fn paragraph_atomic_pagination() {
        // Paragraphs that wrap to many lines; pagination must never split a
        // paragraph across pages unless it alone exceeds a page, and must
        // cover every paragraph exactly once.
        let paras: Vec<String> = (0..10)
            .map(|i| "word ".repeat(60).trim().to_string() + &format!(" {i}"))
            .collect();
        let layout = Layout {
            line_h: 20,
            char_w: 10,
            ..Layout::portrait_default()
        };
        let wrapped: Vec<usize> = paras.iter().map(|p| layout.wrap(p).len()).collect();
        let max_lines = layout.body_lines();
        // Greedy paragraph packing, mirroring the implementation.
        let expect = || {
            let mut n = 0;
            let mut cur = 0;
            for w in &wrapped {
                if cur > 0 && cur + 1 + w > max_lines {
                    n += 1;
                    cur = 0;
                }
                cur = if cur == 0 { *w } else { cur + 1 + w };
            }
            n + 1
        };
        let b = bt(&paras);
        let pages = b.chapter_pages(0, &layout);
        assert_eq!(pages.len(), expect());
        assert_eq!(pages[0].pages_in_chapter, pages.len());
        // Each page's non-empty line count equals the sum of its paragraph
        // wrapped counts (paragraph-atomic: whole paragraphs per page).
        let mut wi = 0usize;
        for p in &pages {
            let nonempty = p.lines.iter().filter(|l| !l.is_empty()).count();
            let spacers = p.lines.len() - nonempty;
            let mut want = 0;
            let mut used = 0;
            while wi + used < wrapped.len()
                && want + wrapped[wi + used] <= nonempty
                && used <= spacers + 1
            {
                want += wrapped[wi + used];
                used += 1;
                if used > spacers {
                    break;
                }
            }
            assert!(used >= 1, "page must carry a paragraph");
            assert_eq!(want, nonempty, "page {} paragraph-atomic", p.page);
            wi += used;
        }
        assert_eq!(wi, wrapped.len(), "all paragraphs consumed");
        for w in pages.windows(2) {
            assert!(w[0].end < w[1].start, "spans disjoint");
            assert!(w[1].progress > w[0].progress);
        }
    }

    #[test]
    fn progress_monotonic() {
        let paras: Vec<String> = (0..40)
            .map(|i| format!("paragraph {i}: ").repeat(20))
            .collect();
        let b = bt(&paras);
        let layout = Layout::portrait_default();
        let pages = b.chapter_pages(0, &layout);
        let mut last = -1.0f32;
        let mut last_end = 0usize;
        for p in &pages {
            assert!(p.progress >= last, "progress monotonic");
            assert!(p.start >= last_end, "spans monotonic");
            last = p.progress;
            last_end = p.end;
        }
        assert!(last > 0.0 && last <= 1.0);
    }
}

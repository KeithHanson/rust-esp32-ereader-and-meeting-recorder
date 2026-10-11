//! Host simulator: paginate Dungeon Crawler Carl and dump a text-mode render
//! of each page for the 480x800 (portrait) or 800x480 (landscape) reading
//! surface. Exercises the same `ui` module the firmware will use.

use std::io::Write;

mod ui;

fn main() {
    let book = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("ws397-rust/books/dungeon-crawler-carl.epub");

    let mut epub = epub::doc::EpubDoc::new(&book).expect("open epub");
    let chapters = ui::epub::chapters(&mut epub);
    println!("title: {}", ui::epub::title(&epub));
    println!("chapters: {}", chapters.len());
    for (i, c) in chapters.iter().take(8).enumerate() {
        println!("  [{i:3}] {:30} {}", c.title, c.text.len());
    }
    // args: [chapter] [page]
    let arg_ch: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let arg_page: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let book_text = ui::BookText { chapters };
    let layout = ui::layout::Layout::portrait_default();
    let pages = book_text.chapter_pages(arg_ch, &layout);
    println!("\nchapter {arg_ch} pages: {}", pages.len());
    let mut out = std::io::stdout();
    if let Some(page) = pages.get(arg_page) {
        ui::render_text(&mut out, page, &layout);
    } else {
        use std::io::Write;
        writeln!(out, "page {arg_page} out of range").unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_pages_fit_and_paginate() {
        let book = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("ws397-rust/books/dungeon-crawler-carl.epub");
        let mut doc = epub::doc::EpubDoc::new(&book).unwrap();
        let chapters = ui::epub::chapters(&mut doc);
        assert!(chapters.len() > 40);
        assert!(chapters.iter().any(|c| c.title == "Chapter 1"));
        let layout = ui::layout::Layout::portrait_default();
        let bt = ui::BookText { chapters };
        let ch1 = bt.chapters.iter().position(|c| c.title == "Chapter 1").unwrap();
        let pages = bt.chapter_pages(ch1, &layout);
        let ml = layout.body_lines();
        // Spans tile the chapter exactly: contiguous, increasing, bounded.
        let text = &bt.chapters[ch1].text;
        assert_eq!(text[..pages[0].start].trim(), "", "page 0 starts at content");
        assert_eq!(pages.last().unwrap().end, text.trim_end().len());
        for w in pages.windows(2) {
            assert!(w[0].end <= w[1].start, "spans must not overlap");
            // Only paragraph separators ("\n\n") may lie between pages.
            assert_eq!(text[w[0].end..w[1].start].trim(), "");
        }
        // Spans cover real text: each span's own content, re-wrapped, equals
        // the page's non-spacer lines.
        for p in &pages {
            let span = &text[p.start..p.end];
            let expect: Vec<String> = span
                .split("\n\n")
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .flat_map(|s| layout.wrap(s))
                .collect();
            let got: Vec<String> = p.lines.iter().filter(|l| !l.is_empty()).cloned().collect();
            assert_eq!(got, expect, "page {} content", p.page);
            assert!(p.lines.len() <= ml + 1, "allow one long paragraph overflow");
            for l in &p.lines {
                assert!(l.chars().count() <= layout.width_chars());
            }
        }
        // progress monotonic and <= 1
        let mut last = 0.0;
        for p in &pages {
            assert!(p.progress >= last);
            assert!(p.progress <= 1.0);
            last = p.progress;
        }
        // landscape yields more chars/line
        let ls = ui::layout::Layout::landscape_default();
        let pages_ls = bt.chapter_pages(ch1, &ls);
        assert!(ls.width_chars() > layout.width_chars());
        assert!(!pages_ls.is_empty());
    }
}

//! EPUB extraction: chapter list + cleaned text, via the `epub` crate.
//! The firmware reuses these exact functions (same crate, on-device).
//!
//! Strategy: the spine is the authoritative reading order; titles come from
//! the first NCX `NavPoint` that targets each spine resource (navmaps can
//! skip or nest; spine order is what pagination needs).

use std::path::PathBuf;

use epub::doc::EpubDoc;

use super::Chapter;

pub fn title<R: std::io::Read + std::io::Seek>(doc: &EpubDoc<R>) -> String {
    doc.get_title().unwrap_or_else(|| "Untitled".into())
}

/// Chapters in spine order, titled from the TOC where available, with
/// cleaned reading text. Empty resources (images-only pages) are skipped.
pub fn chapters<R: std::io::Read + std::io::Seek>(doc: &mut EpubDoc<R>) -> Vec<Chapter> {
    // Path -> title from a depth-first walk of the NCX.
    let mut titles: std::collections::HashMap<PathBuf, String> = std::collections::HashMap::new();
    fn walk(
        pts: &[epub::doc::NavPoint],
        out: &mut std::collections::HashMap<PathBuf, String>,
    ) {
        for p in pts {
            // First writer wins: outer/higher TOC entries label their file.
            out.entry(p.content.clone()).or_insert_with(|| p.label.clone());
            walk(&p.children, out);
        }
    }
    walk(&doc.toc, &mut titles);

    let spine = doc.spine.clone();
    let mut out = Vec::with_capacity(spine.len());
    for item in spine.iter().filter(|s| s.linear) {
        let Some(resource) = doc.resources.get(&item.idref) else {
            continue;
        };
        let path = resource.path.clone();
        let Some(data) = doc.get_resource_by_path(&path) else {
            continue;
        };
        let text = clean_xhtml(&data);
        if text.trim().is_empty() {
            continue;
        }
        // TOC content paths are relative to the OPF root, same as resource
        // paths here; try both raw and joined with root_base for safety.
        let title = titles
            .get(&path)
            .or_else(|| titles.get(&doc.root_base.join(&path)))
            .cloned()
            .unwrap_or_else(|| format!("untitled {}", out.len() + 1));
        out.push(Chapter { title, text });
    }
    out
}

/// Strip tags/entities from XHTML to reading text. Block tags (`p`, `div`,
/// `h1-6`, `br`, `li`, `ul`, `ol`) become paragraph/line breaks; everything
/// else is stripped. Runs of whitespace collapse; paragraph breaks survive.
pub fn clean_xhtml(data: &[u8]) -> String {
    let s = String::from_utf8_lossy(data);
    let mut out = String::with_capacity(s.len() / 2);
    let mut in_tag = false;
    let mut skipping = false; // inside style/script: drop text content
    let mut in_entity = false;
    let mut entity = String::new();

    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if in_entity {
            if c == ';' {
                decode_entity(&entity, &mut out);
                entity.clear();
                in_entity = false;
            } else if entity.len() < 10 {
                entity.push(c);
            } else {
                in_entity = false; // malformed; drop
            }
            continue;
        }
        match c {
            '<' => {
                // Peek the tag name to decide if it's a block break.
                let mut name = String::new();
                let closing = matches!(it.peek(), Some('/'));
                if closing {
                    it.next();
                }
                for nc in it.clone().take(10) {
                    if nc.is_ascii_alphabetic() {
                        name.push(nc);
                    } else {
                        break;
                    }
                }
                match (name.as_str(), closing) {
                    ("p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "ul"
                    | "ol" | "blockquote" | "section" | "body", false) => {
                        if !out.ends_with("\n\n") && !out.is_empty() {
                            out.push_str("\n\n");
                        }
                    }
                    ("br", _) => out.push('\n'),
                    ("style" | "script", false) => skipping = true,
                    ("style" | "script", true) => skipping = false,
                    _ => {}
                }
                in_tag = true;
            }
            '>' => in_tag = false,
            '&' => in_entity = true,
            _ => {
                if !in_tag && !skipping {
                    out.push(c);
                }
            }
        }
    }

    normalize_lines(&out)
}

fn decode_entity(name: &str, out: &mut String) {
    match name {
        "amp" => out.push('&'),
        "lt" => out.push('<'),
        "gt" => out.push('>'),
        "quot" => out.push('"'),
        "apos" => out.push('\''),
        "nbsp" => out.push(' '),
        _ if name.starts_with("#x") || name.starts_with("#X") => {
            if let Some(ch) = u32::from_str_radix(&name[2..], 16).ok().and_then(char::from_u32) {
                out.push(ch);
            }
        }
        _ if name.starts_with('#') => {
            if let Some(ch) = name[1..].parse::<u32>().ok().and_then(char::from_u32) {
                out.push(ch);
            }
        }
        _ => {}
    }
}

/// Collapse intra-line whitespace, keep paragraphs as blank-line separators.
fn normalize_lines(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for line in text.lines() {
        let t: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            if !cleaned.is_empty() && !cleaned.ends_with("\n\n") {
                cleaned.push_str("\n\n");
            }
        } else {
            if !cleaned.is_empty() && !cleaned.ends_with('\n') {
                cleaned.push('\n');
            }
            cleaned.push_str(&t);
        }
    }
    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_keeps_paragraphs() {
        let x =
            b"<html><body><h1>Hi</h1><p>One &amp; two.</p><p>Second para<br/>line</p></body></html>";
        let t = clean_xhtml(x);
        assert_eq!(t, "Hi\n\nOne & two.\n\nSecond para\nline");
    }

    #[test]
    fn decodes_numeric_entities() {
        let t = clean_xhtml(b"<p>a&#8217;b&#x2014;c</p>");
        assert_eq!(t, "a\u{2019}b\u{2014}c");
    }

    #[test]
    fn skips_css_and_script() {
        let x = b"<style>p{color:red}</style><p>real</p><script>x<y</script><p>text</p>";
        let t = clean_xhtml(x);
        assert!(t.contains("real"));
        assert!(t.contains("text"));
    }
}

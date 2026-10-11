use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

mod ui {
    pub struct Chapter {
        pub title: String,
        pub text: String,
    }
}

#[path = "src/epub_extract.rs"]
mod epub_extract;

fn put_u32(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&(value as u32).to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, value: &str) {
    put_u32(out, value.len());
    out.extend_from_slice(value.as_bytes());
}

fn compile_book(path: &Path) -> Vec<u8> {
    let source = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut doc = epub::doc::EpubDoc::from_reader(Cursor::new(source))
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    let title = epub_extract::title(&doc);
    let chapters = epub_extract::chapters(&mut doc);
    assert!(
        !chapters.is_empty(),
        "{} contains no readable chapters",
        path.display()
    );

    let mut out = Vec::new();
    out.extend_from_slice(b"WSBK\x01");
    put_str(&mut out, &title);
    put_u32(&mut out, chapters.len());
    for chapter in chapters {
        put_str(&mut out, &chapter.title);
        put_str(&mut out, &chapter.text);
    }
    out
}

fn main() {
    embuild::espidf::sysenv::output();

    let books_dir = PathBuf::from("books");
    println!("cargo:rerun-if-changed={}", books_dir.display());
    let mut books: Vec<PathBuf> = fs::read_dir(&books_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", books_dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "epub"))
        .collect();
    books.sort();

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let mut index = String::from("pub static EMBEDDED_BOOKS: &[BookEntry] = &[\n");
    for (i, path) in books.iter().enumerate() {
        let blob_name = format!("book-{i}.wsbk");
        fs::write(out_dir.join(&blob_name), compile_book(path))
            .unwrap_or_else(|e| panic!("write {blob_name}: {e}"));
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("UTF-8 EPUB filename");
        index.push_str(&format!(
            "    BookEntry {{ name: {name:?}, data: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{blob_name}\")) }},\n"
        ));
    }
    index.push_str("];\n");
    fs::write(out_dir.join("book_index.rs"), index).expect("write book index");
}

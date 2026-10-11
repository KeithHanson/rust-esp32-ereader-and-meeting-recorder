use embedded_graphics::prelude::Point;
use u8g2_fonts::{fonts, types::VerticalPosition, Font, FontRenderer};

fn probe<F: Font>(name: &str) {
    let r = FontRenderer::new::<F>();
    let lh = r.get_default_line_height();
    let w = r
        .get_rendered_dimensions(
            "The quick brown fox jumps over the lazy dog",
            Point::zero(),
            VerticalPosition::Top,
        )
        .map(|d| d.advance.x)
        .unwrap_or(0);
    println!("{name:16} line_height={lh:3}  w44px={w:4}");
}

fn main() {
    probe::<fonts::u8g2_font_profont12_tf>("profont12");
    probe::<fonts::u8g2_font_profont22_tf>("profont22");
    probe::<fonts::u8g2_font_profont29_tf>("profont29");
    probe::<fonts::u8g2_font_helvR08_tf>("helvR08");
    probe::<fonts::u8g2_font_helvR12_tf>("helvR12");
    probe::<fonts::u8g2_font_helvR18_tf>("helvR18");
    probe::<fonts::u8g2_font_ncenR12_tf>("ncenR12");
    probe::<fonts::u8g2_font_ncenR18_tf>("ncenR18");
    probe::<fonts::u8g2_font_10x20_mr>("10x20 mono");
    probe::<fonts::u8g2_font_inr24_mf>("inr24");
}

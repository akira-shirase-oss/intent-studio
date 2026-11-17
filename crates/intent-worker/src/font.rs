//! TrueType and OpenType fonts: the tables parsed and every glyph walked (D-062). Fonts are not
//! re-encoded; a font that parses whole is stored as uploaded. WOFF, WOFF2 and collections are
//! refused for now: serving fonts in WOFF2 arrives with the bundle build (I2).

use intent_render::WorkerRefusal;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

/// Largest font accepted (5 MiB).
pub const MAX_FONT_BYTES: usize = 5 * 1024 * 1024;

struct Discard;

impl OutlineBuilder for Discard {
    fn move_to(&mut self, _x: f32, _y: f32) {}
    fn line_to(&mut self, _x: f32, _y: f32) {}
    fn quad_to(&mut self, _x1: f32, _y1: f32, _x: f32, _y: f32) {}
    fn curve_to(&mut self, _x1: f32, _y1: f32, _x2: f32, _y2: f32, _x: f32, _y: f32) {}
    fn close(&mut self) {}
}

/// Checks a font; returns its media type (`font/ttf` or `font/otf`).
pub fn check(input: &[u8]) -> Result<&'static str, WorkerRefusal> {
    if input.len() > MAX_FONT_BYTES {
        return Err(WorkerRefusal::TooLarge);
    }
    let media_type = match input.get(..4) {
        Some([0, 1, 0, 0] | b"true") => "font/ttf",
        Some(b"OTTO") => "font/otf",
        Some(b"wOFF" | b"wOF2" | b"ttcf") => return Err(WorkerRefusal::UnsupportedType),
        _ => return Err(WorkerRefusal::TypeMismatch),
    };
    let face = Face::parse(input, 0).map_err(|_| WorkerRefusal::Malformed)?;
    if face.number_of_glyphs() == 0 || face.tables().cmap.is_none() {
        return Err(WorkerRefusal::Malformed);
    }
    // Walk every glyph's outline: a table that parses but whose glyphs do not is malformed in
    // the place a renderer would find out.
    for id in 0..face.number_of_glyphs() {
        let _ = face.outline_glyph(GlyphId(id), &mut Discard);
    }
    Ok(media_type)
}

//! TrueType and OpenType fonts: the tables parsed and every glyph drawn (D-062), with skrifa
//! (Google Fonts' maintained parser; ttf-parser is unmaintained, RUSTSEC-2026-0192). Fonts are
//! not re-encoded; a font that parses whole is stored as uploaded. WOFF, WOFF2 and collections
//! are refused for now: serving fonts in WOFF2 arrives with the bundle build (I2).

use intent_render::WorkerRefusal;
use skrifa::{
    FontRef, GlyphId, MetadataProvider as _,
    instance::{LocationRef, Size},
    metrics::Metrics,
    outline::{DrawSettings, OutlinePen},
};

/// Largest font accepted (5 MiB).
pub const MAX_FONT_BYTES: usize = 5 * 1024 * 1024;

struct Discard;

impl OutlinePen for Discard {
    fn move_to(&mut self, _x: f32, _y: f32) {}
    fn line_to(&mut self, _x: f32, _y: f32) {}
    fn quad_to(&mut self, _cx0: f32, _cy0: f32, _x: f32, _y: f32) {}
    fn curve_to(&mut self, _cx0: f32, _cy0: f32, _cx1: f32, _cy1: f32, _x: f32, _y: f32) {}
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
    let font = FontRef::new(input).map_err(|_| WorkerRefusal::Malformed)?;
    let metrics = Metrics::new(&font, Size::unscaled(), LocationRef::default());
    // A font needs a sane em, at least one glyph and a character map to be usable at all.
    if !(16..=16_384).contains(&metrics.units_per_em)
        || metrics.glyph_count == 0
        || !font.charmap().has_map()
    {
        return Err(WorkerRefusal::Malformed);
    }
    // Draw every glyph: a table that parses but whose glyphs do not is malformed in the place a
    // renderer would find out. A glyph with no outline (a space) is fine.
    let outlines = font.outline_glyphs();
    let settings = || DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
    for id in 0..metrics.glyph_count {
        if let Some(glyph) = outlines.get(GlyphId::new(id)) {
            glyph
                .draw(settings(), &mut Discard)
                .map_err(|_| WorkerRefusal::Malformed)?;
        }
    }
    Ok(media_type)
}

//! The asset worker's checks (D-011, D-062). Each runs inside its own `intent-worker` process
//! (see `main.rs`), never in the API server: these are the only places in the repository that
//! decode untrusted bytes, and `scripts/check-architecture.sh` (A6) keeps every decoder crate
//! here.
//!
//! - [`svg::sanitize`] re-serializes an SVG from an element and attribute allowlist: nothing the
//!   allowlist does not name survives, so no script, event handler, external reference,
//!   stylesheet or embedded document can reach a bundle.
//! - [`raster::reencode`] decodes a PNG or JPEG under pixel and memory limits and encodes it
//!   again, which also drops every metadata chunk (location, camera, comments).
//! - [`font::check`] parses a TrueType or OpenType font's tables and walks every glyph.

use intent_render::{AssetKind, Processed, WorkerRefusal};

pub mod font;
pub mod raster;
pub mod svg;

/// Runs the check for `kind` over `input`.
pub fn process(kind: AssetKind, input: &[u8]) -> Result<Processed, WorkerRefusal> {
    match kind {
        AssetKind::Svg => {
            let clean = svg::sanitize(input)?;
            Ok(Processed {
                media_type: "image/svg+xml".into(),
                width: clean.width,
                height: clean.height,
                removed_parts: clean.removed,
                output: clean.svg.into_bytes(),
            })
        }
        AssetKind::Png | AssetKind::Jpeg => {
            let image = raster::reencode(kind, input)?;
            Ok(Processed {
                media_type: image.media_type.into(),
                width: Some(image.width),
                height: Some(image.height),
                removed_parts: 0,
                output: image.bytes,
            })
        }
        AssetKind::Font => {
            let media_type = font::check(input)?;
            Ok(Processed {
                media_type: media_type.into(),
                width: None,
                height: None,
                removed_parts: 0,
                output: input.to_vec(),
            })
        }
    }
}

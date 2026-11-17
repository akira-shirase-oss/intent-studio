//! The asset security case (`END_GOAL.md`): an SVG carrying script is sanitized, a malformed image or
//! font is refused, and a worker that crashes or hangs fails only its own job. Run through the
//! real `intent-worker` binary and `intent-render`'s driver, exactly as the server runs them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

use std::{io::Cursor, path::PathBuf, time::Duration};

use intent_render::{AssetKind, Driver, WorkerRefusal};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/assets")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn driver() -> Driver {
    Driver::new(
        PathBuf::from(env!("CARGO_BIN_EXE_intent-worker")),
        Duration::from_secs(20),
        2,
    )
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([31, 78, 121, 255]));
    let mut out = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn jpeg_with_comment() -> Vec<u8> {
    let image = image::RgbImage::from_pixel(16, 8, image::Rgb([200, 60, 40]));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&image)
        .unwrap();
    // A COM segment right after SOI: metadata the re-encode must not keep.
    let comment = b"GPS 53.8008 -1.5491 secret";
    let mut segment = vec![0xFF, 0xFE];
    segment.extend_from_slice(&u16::try_from(comment.len() + 2).unwrap().to_be_bytes());
    segment.extend_from_slice(comment);
    out.splice(2..2, segment);
    out
}

/// A minimal TrueType font built field by field: head, hhea, maxp, hmtx and a format-4 cmap.
fn font() -> Vec<u8> {
    fn be16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    fn be32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut head = Vec::new();
    be32(&mut head, 0x0001_0000); // version
    be32(&mut head, 0x0001_0000); // fontRevision
    be32(&mut head, 0); // checkSumAdjustment
    be32(&mut head, 0x5F0F_3CF5); // magicNumber
    be16(&mut head, 0); // flags
    be16(&mut head, 1000); // unitsPerEm
    head.extend_from_slice(&[0; 16]); // created, modified
    for v in [0_u16, 0, 500, 700] {
        be16(&mut head, v); // xMin, yMin, xMax, yMax
    }
    be16(&mut head, 0); // macStyle
    be16(&mut head, 8); // lowestRecPPEM
    be16(&mut head, 2); // fontDirectionHint
    be16(&mut head, 0); // indexToLocFormat
    be16(&mut head, 0); // glyphDataFormat
    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    for v in [
        800_u16,
        (-200_i16).cast_unsigned(),
        0,
        500,
        0,
        0,
        500,
        1,
        0,
        0,
    ] {
        be16(&mut hhea, v); // ascender, descender, lineGap, advanceWidthMax, minLSB, minRSB, xMaxExtent, caretSlopeRise, caretSlopeRun, caretOffset
    }
    hhea.extend_from_slice(&[0; 8]); // reserved
    be16(&mut hhea, 0); // metricDataFormat
    be16(&mut hhea, 1); // numberOfHMetrics
    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0000_5000);
    be16(&mut maxp, 1); // numGlyphs
    let mut hmtx = Vec::new();
    be16(&mut hmtx, 500);
    be16(&mut hmtx, 0);
    let mut cmap = Vec::new();
    be16(&mut cmap, 0); // version
    be16(&mut cmap, 1); // numTables
    be16(&mut cmap, 3); // platform: Windows
    be16(&mut cmap, 1); // encoding: Unicode BMP
    be32(&mut cmap, 12); // offset
    // Format 4 with the single terminating segment.
    for v in [4_u16, 24, 0, 2, 2, 0, 0, 0xFFFF, 0, 0xFFFF, 1, 0] {
        be16(&mut cmap, v);
    }
    let tables: [(&[u8; 4], Vec<u8>); 5] = [
        (b"cmap", cmap),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"maxp", maxp),
    ];
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    be16(&mut out, 5); // numTables
    be16(&mut out, 64);
    be16(&mut out, 2);
    be16(&mut out, 16);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        be32(&mut out, 0); // checksum: not checked by parsers
        be32(&mut out, u32::try_from(offset).unwrap());
        be32(&mut out, u32::try_from(data.len()).unwrap());
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend_from_slice(&body);
    out
}

#[tokio::test]
async fn an_svg_carrying_script_comes_out_with_none_of_it() {
    let clean = driver()
        .run(AssetKind::Svg, &fixture("hostile.svg"))
        .await
        .unwrap();
    assert_eq!(clean.media_type, "image/svg+xml");
    assert_eq!((clean.width, clean.height), (Some(120), Some(40)));
    let svg = String::from_utf8(clean.output).unwrap();
    let lower = svg.to_ascii_lowercase();
    for banned in [
        "<script",
        "alert",
        "javascript",
        "onload",
        "onclick",
        "onmouseover",
        "<style",
        "@import",
        "evil.example",
        "foreignobject",
        "<iframe",
        "<image",
        "data:",
        "<a ",
        "<animate",
        "<set",
        "inkscape",
        "style=",
        "cdata",
    ] {
        assert!(!lower.contains(banned), "{banned:?} survived:\n{svg}");
    }
    // What a picture needs is still there.
    for kept in [
        "<rect",
        "<circle",
        "<linearGradient id=\"g\"",
        "fill=\"url(#g)\"",
        "<use href=\"#g\">",
        "Lee &amp; Daughters",
        "viewBox=\"0 0 120 40\"",
    ] {
        assert!(svg.contains(kept), "{kept:?} was lost:\n{svg}");
    }
    // onload, inkscape:version, 2 scripts, style, the gradient's remote href, onclick, a style
    // attribute, a remote fill, 2 links, foreignObject, image, the remote use href, animate,
    // set and iframe.
    assert_eq!(clean.removed_parts, 17);
    // The output is itself an SVG that sanitizes to itself.
    let again = driver().run(AssetKind::Svg, svg.as_bytes()).await.unwrap();
    assert_eq!(again.output, svg.as_bytes());
    assert_eq!(again.removed_parts, 0);
}

#[tokio::test]
async fn a_clean_svg_keeps_everything() {
    let clean = driver()
        .run(AssetKind::Svg, &fixture("clean-logo.svg"))
        .await
        .unwrap();
    assert_eq!(clean.removed_parts, 0);
    assert_eq!((clean.width, clean.height), (Some(64), Some(64)));
}

#[tokio::test]
async fn hostile_and_malformed_files_are_refused_by_name() {
    let d = driver();
    assert_eq!(
        d.run(AssetKind::Svg, &fixture("entity-bomb.svg")).await,
        Err(WorkerRefusal::Malformed),
        "a DTD is never parsed"
    );
    assert_eq!(
        d.run(AssetKind::Svg, &fixture("not-svg.svg")).await,
        Err(WorkerRefusal::TypeMismatch)
    );
    assert_eq!(
        d.run(AssetKind::Png, &fixture("not-a-png.png")).await,
        Err(WorkerRefusal::TypeMismatch)
    );
    let good = png(4, 3);
    let truncated = &good[..good.len() / 2];
    assert_eq!(
        d.run(AssetKind::Png, truncated).await,
        Err(WorkerRefusal::Malformed)
    );
    // A PNG uploaded as a JPEG.
    assert_eq!(
        d.run(AssetKind::Jpeg, &good).await,
        Err(WorkerRefusal::TypeMismatch)
    );
    // Wider than the limit: refused before a full decode.
    assert_eq!(
        d.run(AssetKind::Png, &png(9_000, 1)).await,
        Err(WorkerRefusal::TooLarge)
    );
    // Fonts: a truncated face, WOFF2, and text.
    let font = font();
    assert_eq!(
        d.run(AssetKind::Font, &font[..40]).await,
        Err(WorkerRefusal::Malformed)
    );
    assert_eq!(
        d.run(AssetKind::Font, b"wOF2\0\x01\0\0rest").await,
        Err(WorkerRefusal::UnsupportedType)
    );
    assert_eq!(
        d.run(AssetKind::Font, b"<svg/>").await,
        Err(WorkerRefusal::TypeMismatch)
    );
}

#[tokio::test]
async fn good_images_and_fonts_pass_and_lose_their_metadata() {
    let d = driver();
    let png = d.run(AssetKind::Png, &png(4, 3)).await.unwrap();
    assert_eq!(
        (png.media_type.as_str(), png.width, png.height),
        ("image/png", Some(4), Some(3))
    );
    let jpeg_in = jpeg_with_comment();
    assert!(jpeg_in.windows(6).any(|w| w == b"secret"));
    let jpeg = d.run(AssetKind::Jpeg, &jpeg_in).await.unwrap();
    assert_eq!((jpeg.width, jpeg.height), (Some(16), Some(8)));
    assert!(
        !jpeg.output.windows(6).any(|w| w == b"secret"),
        "metadata survived"
    );
    let font = font();
    let checked = d.run(AssetKind::Font, &font).await.unwrap();
    assert_eq!(
        (checked.media_type.as_str(), checked.output),
        ("font/ttf", font)
    );
}

#[test]
fn an_unknown_kind_is_refused_before_anything_is_read() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_intent-worker"))
        .arg("gif")
        .env_clear()
        .stdin(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}

/// The worker's limits, read from the kernel while it waits for its input (Linux, where
/// `/proc` shows them and the address-space limit is enforced; CI runs on Linux).
#[cfg(target_os = "linux")]
#[test]
fn the_worker_confines_itself_before_reading_input() {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_intent-worker"))
        .arg("svg")
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let limits = std::fs::read_to_string(format!("/proc/{}/limits", child.id())).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    let row = |name: &str| {
        limits
            .lines()
            .find(|l| l.starts_with(name))
            .unwrap_or_else(|| panic!("no {name} in\n{limits}"))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(
        row("Max cpu time").contains("10 10 seconds"),
        "{}",
        row("Max cpu time")
    );
    assert!(
        row("Max file size").contains("0 0 bytes"),
        "{}",
        row("Max file size")
    );
    assert!(
        row("Max core file size").contains("0 0 bytes"),
        "{}",
        row("Max core file size")
    );
    assert!(
        row("Max open files").contains("16 16 files"),
        "{}",
        row("Max open files")
    );
    assert!(
        row("Max address space").contains("1073741824 1073741824 bytes"),
        "{}",
        row("Max address space")
    );
}

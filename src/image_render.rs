//! Pure image → ASCII-art kernel. Decodes nothing and touches no terminal;
//! callers pass an already-decoded `RgbaImage`. Mirrors `render`'s discipline:
//! plain inputs, plain cell outputs, exhaustively unit-tested.

use crate::ansi::{Color, Style};
use crate::render::Cell;
use image::RgbaImage;
use std::time::Duration;

/// Decoded animation: per-frame RGBA + the delay until the next frame, plus the
/// loop count (`None` = infinite).
pub struct Animation {
    pub frames: Vec<(RgbaImage, Duration)>,
    pub loop_count: Option<u32>,
}

/// Outcome of `decode_animation`.
pub enum AnimationDecode {
    /// Not an animation (single frame / static): the caller decodes and
    /// displays it normally, with no hint.
    Static,
    /// A decoded multi-frame animation to play.
    Animated(Animation),
    /// The source *is* an animation, but its frames could not be decoded
    /// (e.g. a 16-bit APNG — the `image` crate's APNG decoder rejects
    /// `Rgb16`). The caller should fall back to the static first frame and
    /// hint the user, rather than silently dropping the animation. Carries a
    /// short human-facing reason.
    Unsupported(String),
}

/// True if the PNG carries an `acTL` (animation-control) chunk — i.e. it is an
/// APNG, not a plain still PNG. Lets `decode_animation` distinguish "static
/// PNG" (even 16-bit) from "animated PNG that failed to decode", so a still
/// image is never reported as an animation failure.
fn png_has_actl(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|w| w == b"acTL")
}

/// Extract the loop count from a GIF's NETSCAPE2.0 application extension.
/// Returns `Some(0)` for infinite, `Some(n)` for a finite count, or `None`
/// when the extension is absent (callers treat that as infinite). The `image`
/// crate's high-level decoder does not expose this, so we read it directly.
pub fn parse_gif_loop_count(bytes: &[u8]) -> Option<u32> {
    let needle = b"\x21\xFF\x0BNETSCAPE2.0";
    let pos = bytes.windows(needle.len()).position(|w| w == needle)?;
    let sub = pos + needle.len();
    // After the 11-byte "NETSCAPE2.0" name comes the loop sub-block:
    // 0x03 (size=3), 0x01 (id=loop), then a u16 little-endian loop count.
    if bytes.len() >= sub + 4 && bytes[sub] == 0x03 && bytes[sub + 1] == 0x01 {
        let lo = bytes[sub + 2] as u32;
        let hi = bytes[sub + 3] as u32;
        return Some(lo | (hi << 8));
    }
    None
}

/// Decode an animated image. Returns `Static` for a single-frame / non-animated
/// source (caller falls back to `decode_image`), `Animated` for a playable
/// multi-frame animation, or `Unsupported` when the source is animated but its
/// frames can't be decoded (so the caller can show the first frame *and* hint).
/// GIF loop count comes from the NETSCAPE extension; other formats default to
/// infinite. GIF, 8-bit APNG, and animated WebP decode to `Animated`; a 16-bit
/// APNG (which the `image` crate rejects) decodes to `Unsupported` — both
/// covered by `tests/animation_decode.rs`.
pub fn decode_animation(bytes: &[u8]) -> AnimationDecode {
    use image::AnimationDecoder;
    let fmt = match image::guess_format(bytes) {
        Ok(f) => f,
        Err(_) => return AnimationDecode::Static,
    };
    // Decode the container's frames. A failed *decode* of an animated source
    // becomes `Unsupported`; "not an animation at all" becomes `Static`.
    let frames: Vec<image::Frame> = match fmt {
        image::ImageFormat::Gif => match image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)) {
            Ok(d) => match d.into_frames().collect_frames() {
                Ok(f) => f,
                Err(e) => return AnimationDecode::Unsupported(format!("GIF: {e}")),
            },
            Err(_) => return AnimationDecode::Static,
        },
        image::ImageFormat::WebP => match image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes)) {
            Ok(d) => match d.into_frames().collect_frames() {
                Ok(f) => f,
                Err(e) => return AnimationDecode::Unsupported(format!("WebP: {e}")),
            },
            Err(_) => return AnimationDecode::Static,
        },
        image::ImageFormat::Png => {
            // Only an APNG (acTL present) can be an animation; a plain still
            // PNG — even 16-bit — is `Static`, never an animation failure.
            if !png_has_actl(bytes) {
                return AnimationDecode::Static;
            }
            match image::codecs::png::PngDecoder::new(std::io::Cursor::new(bytes)) {
                Ok(d) => match d.apng() {
                    Ok(apng) => match apng.into_frames().collect_frames() {
                        Ok(f) => f,
                        Err(e) => return AnimationDecode::Unsupported(format!("APNG: {e}")),
                    },
                    Err(e) => return AnimationDecode::Unsupported(format!("APNG: {e}")),
                },
                Err(_) => return AnimationDecode::Static,
            }
        }
        _ => return AnimationDecode::Static,
    };
    if frames.len() <= 1 {
        return AnimationDecode::Static;
    }
    let loop_count = if fmt == image::ImageFormat::Gif {
        parse_gif_loop_count(bytes)
    } else {
        None
    };
    let frames = frames
        .into_iter()
        .map(|f| {
            let delay: Duration = f.delay().into();
            (f.into_buffer(), delay)
        })
        .collect();
    AnimationDecode::Animated(Animation { frames, loop_count })
}

/// Rendering aesthetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsciiStyle {
    /// Luminance → character ramp, one source block per cell.
    Ramp,
    /// Unicode half-block (▀): fg = top sub-block, bg = bottom sub-block.
    Blocks,
}

/// Luminance ramp, darkest → brightest. Index by `lum * (len-1) / 255`.
pub const RAMP: &[char] = &[' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

/// Block-shade ramp for `--blocks` under `--no-color` (no SGR available).
pub const BLOCK_SHADES: &[char] = &[' ', '░', '▒', '▓', '█'];

/// Terminal cells are about twice as tall as wide.
pub const CELL_ASPECT: u32 = 2;

/// Luminance of an RGB pixel (BT.601), 0..=255.
fn luminance(r: u8, g: u8, b: u8) -> u8 {
    ((77 * r as u32 + 150 * g as u32 + 29 * b as u32) >> 8) as u8
}

/// Number of source-pixel rows collapsed into one cell row for `style`.
fn pixels_per_cell_row(style: AsciiStyle, px_per_col: u32) -> u32 {
    match style {
        AsciiStyle::Ramp => (px_per_col * CELL_ASPECT).max(1),
        AsciiStyle::Blocks => (px_per_col * CELL_ASPECT).max(2),
    }
}

/// How many cell rows `render_image` produces for an image of the given pixel
/// dimensions at `cols` columns. Pure; used for scroll math.
pub fn output_rows(img_w: u32, img_h: u32, cols: u16, style: AsciiStyle) -> usize {
    let cols = (cols.max(1)) as u32;
    let img_w = img_w.max(1);
    let px_per_col = img_w.div_ceil(cols).max(1);
    let ppr = pixels_per_cell_row(style, px_per_col);
    (img_h.div_ceil(ppr)).max(1) as usize
}

/// Alpha-weighted average of an image block → one RGB triple. Transparent
/// pixels contribute proportionally less; a fully transparent block is black.
/// Uses u64 accumulators so large blocks (few columns, big image) can't overflow.
fn average_block(img: &RgbaImage, x0: u32, y0: u32, w: u32, h: u32) -> (u8, u8, u8) {
    let (iw, ih) = img.dimensions();
    let (mut r, mut g, mut b, mut sum_a) = (0u64, 0u64, 0u64, 0u64);
    for y in y0..(y0 + h).min(ih) {
        for x in x0..(x0 + w).min(iw) {
            let p = img.get_pixel(x, y).0;
            let a = p[3] as u64;
            r += p[0] as u64 * a;
            g += p[1] as u64 * a;
            b += p[2] as u64 * a;
            sum_a += a;
        }
    }
    if sum_a == 0 { return (0, 0, 0); }
    ((r / sum_a) as u8, (g / sum_a) as u8, (b / sum_a) as u8)
}

fn ramp_char(lum: u8) -> char {
    let idx = (lum as usize * (RAMP.len() - 1)) / 255;
    RAMP[idx]
}

fn cell_char(ch: char, fg: Option<Color>) -> Cell {
    Cell::Char { ch, width: 1, style: Style { fg, bg: None, ..Default::default() }, hyperlink: None }
}

/// Render the image to a grid of styled cells `cols` wide. `color` controls
/// whether per-cell foreground color is set (false ≈ `--no-color`).
pub fn render_image(img: &RgbaImage, cols: u16, style: AsciiStyle, color: bool) -> Vec<Vec<Cell>> {
    match style {
        AsciiStyle::Ramp => render_ramp(img, cols, color),
        AsciiStyle::Blocks => render_blocks(img, cols, color),
    }
}

fn render_ramp(img: &RgbaImage, cols: u16, color: bool) -> Vec<Vec<Cell>> {
    let (iw, ih) = img.dimensions();
    let cols_u = cols.max(1) as u32;
    let px_per_col = iw.max(1).div_ceil(cols_u).max(1);
    let ppr = pixels_per_cell_row(AsciiStyle::Ramp, px_per_col);
    let rows = output_rows(iw, ih, cols, AsciiStyle::Ramp);
    let mut grid = Vec::with_capacity(rows);
    for ry in 0..rows {
        let mut row = Vec::with_capacity(cols as usize);
        for cx in 0..cols_u {
            let (r, g, b) = average_block(img, cx * px_per_col, ry as u32 * ppr, px_per_col, ppr);
            let ch = ramp_char(luminance(r, g, b));
            let fg = if color { Some(Color::Rgb(r, g, b)) } else { None };
            row.push(cell_char(ch, fg));
        }
        grid.push(row);
    }
    grid
}

fn block_shade_char(lum: u8) -> char {
    let idx = (lum as usize * (BLOCK_SHADES.len() - 1)) / 255;
    BLOCK_SHADES[idx]
}

fn render_blocks(img: &RgbaImage, cols: u16, color: bool) -> Vec<Vec<Cell>> {
    let (iw, ih) = img.dimensions();
    let cols_u = cols.max(1) as u32;
    let px_per_col = iw.max(1).div_ceil(cols_u).max(1);
    let ppr = pixels_per_cell_row(AsciiStyle::Blocks, px_per_col); // even, >= 2
    let half = (ppr / 2).max(1);
    let rows = output_rows(iw, ih, cols, AsciiStyle::Blocks);
    let mut grid = Vec::with_capacity(rows);
    for ry in 0..rows {
        let mut row = Vec::with_capacity(cols as usize);
        let y_top = ry as u32 * ppr;
        for cx in 0..cols_u {
            let x0 = cx * px_per_col;
            let (tr, tg, tb) = average_block(img, x0, y_top, px_per_col, half);
            let (br, bg, bb) = average_block(img, x0, y_top + half, px_per_col, half);
            if color {
                row.push(Cell::Char {
                    ch: '▀',
                    width: 1,
                    style: Style {
                        fg: Some(Color::Rgb(tr, tg, tb)),
                        bg: Some(Color::Rgb(br, bg, bb)),
                        ..Default::default()
                    },
                    hyperlink: None,
                });
            } else {
                let lum = luminance(
                    ((tr as u16 + br as u16) / 2) as u8,
                    ((tg as u16 + bg as u16) / 2) as u8,
                    ((tb as u16 + bb as u16) / 2) as u8,
                );
                row.push(cell_char(block_shade_char(lum), None));
            }
        }
        grid.push(row);
    }
    grid
}

/// Identify an image by its leading bytes. Returns a short format name (for the
/// status line) or `None` if the bytes are not a supported image. Content-based
/// only — never guesses from a file extension — so text never misfires.
pub fn sniff_image_format(head: &[u8]) -> Option<&'static str> {
    match image::guess_format(head).ok()? {
        image::ImageFormat::Png => Some("png"),
        image::ImageFormat::Jpeg => Some("jpeg"),
        image::ImageFormat::Gif => Some("gif"),
        image::ImageFormat::Bmp => Some("bmp"),
        image::ImageFormat::WebP => Some("webp"),
        image::ImageFormat::Tiff => Some("tiff"),
        image::ImageFormat::Tga => Some("tga"),
        image::ImageFormat::Ico => Some("ico"),
        image::ImageFormat::Pnm => Some("pnm"),
        _ => None,
    }
}

/// Decode the full image bytes to RGBA8. For animated GIFs this yields the
/// first frame. Returns the decoder error string on failure.
pub fn decode_image(bytes: &[u8]) -> Result<RgbaImage, String> {
    image::load_from_memory(bytes)
        .map(|img| img.to_rgba8())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn sniff_detects_png_and_gif_and_rejects_text() {
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        assert_eq!(sniff_image_format(&png), Some("png"));
        let gif = b"GIF89a............";
        assert_eq!(sniff_image_format(gif), Some("gif"));
        assert_eq!(sniff_image_format(b"hello, world\n"), None);
        assert_eq!(sniff_image_format(b""), None);
    }

    #[test]
    fn decode_roundtrips_a_generated_png() {
        let src = RgbaImage::from_pixel(3, 2, Rgba([10, 20, 30, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(src.clone())
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let decoded = decode_image(buf.get_ref()).unwrap();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert_eq!(decoded.get_pixel(0, 0).0, [10, 20, 30, 255]);
    }

    fn solid(w: u32, h: u32, px: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(px))
    }

    #[test]
    fn output_rows_corrects_aspect_for_ramp() {
        let rows = output_rows(100, 100, 50, AsciiStyle::Ramp);
        assert_eq!(rows, 25);
    }

    #[test]
    fn output_rows_blocks_same_cell_rows_as_ramp() {
        let ramp = output_rows(100, 100, 50, AsciiStyle::Ramp);
        let blocks = output_rows(100, 100, 50, AsciiStyle::Blocks);
        assert_eq!(blocks, ramp);
    }

    #[test]
    fn ramp_white_pixel_is_densest_glyph() {
        let img = solid(4, 4, [255, 255, 255, 255]);
        let grid = render_image(&img, 4, AsciiStyle::Ramp, true);
        match &grid[0][0] {
            Cell::Char { ch, style, .. } => {
                assert_eq!(*ch, '@');
                assert_eq!(style.fg, Some(Color::Rgb(255, 255, 255)));
            }
            other => panic!("expected Char, got {other:?}"),
        }
    }

    #[test]
    fn ramp_black_pixel_is_space() {
        let img = solid(4, 4, [0, 0, 0, 255]);
        let grid = render_image(&img, 4, AsciiStyle::Ramp, true);
        match &grid[0][0] {
            Cell::Char { ch, .. } => assert_eq!(*ch, ' '),
            other => panic!("expected Char, got {other:?}"),
        }
    }

    #[test]
    fn ramp_no_color_sets_default_fg() {
        let img = solid(4, 4, [255, 255, 255, 255]);
        let grid = render_image(&img, 4, AsciiStyle::Ramp, false);
        match &grid[0][0] {
            Cell::Char { ch, style, .. } => {
                assert_eq!(*ch, '@');
                assert_eq!(style.fg, None);
            }
            other => panic!("expected Char, got {other:?}"),
        }
    }

    #[test]
    fn grid_width_matches_requested_cols() {
        let img = solid(40, 40, [128, 128, 128, 255]);
        let grid = render_image(&img, 20, AsciiStyle::Ramp, true);
        assert!(grid.iter().all(|row| row.len() == 20));
    }

    #[test]
    fn average_block_weights_by_alpha_not_pixel_count() {
        // 2x1: one opaque white, one fully transparent. Result must be ~white.
        let mut img = RgbaImage::new(2, 1);
        img.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        img.put_pixel(1, 0, Rgba([0, 0, 0, 0]));
        // Render at 1 col so both pixels fall in one cell block.
        let grid = render_image(&img, 1, AsciiStyle::Ramp, true);
        match &grid[0][0] {
            Cell::Char { style, .. } => {
                assert_eq!(style.fg, Some(Color::Rgb(255, 255, 255)),
                    "opaque white must dominate the transparent pixel");
            }
            other => panic!("expected Char, got {other:?}"),
        }
    }

    #[test]
    fn blocks_sets_fg_top_and_bg_bottom() {
        // 2px wide, 2px tall: top row white, bottom row black.
        let mut img = RgbaImage::new(2, 2);
        for x in 0..2 { img.put_pixel(x, 0, Rgba([255, 255, 255, 255])); }
        for x in 0..2 { img.put_pixel(x, 1, Rgba([0, 0, 0, 255])); }
        let grid = render_image(&img, 2, AsciiStyle::Blocks, true);
        match &grid[0][0] {
            Cell::Char { ch, style, .. } => {
                assert_eq!(*ch, '▀');
                assert_eq!(style.fg, Some(Color::Rgb(255, 255, 255)), "fg = top");
                assert_eq!(style.bg, Some(Color::Rgb(0, 0, 0)), "bg = bottom");
            }
            other => panic!("expected Char, got {other:?}"),
        }
    }

    #[test]
    fn blocks_no_color_uses_block_shades() {
        let img = RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]));
        let grid = render_image(&img, 2, AsciiStyle::Blocks, false);
        match &grid[0][0] {
            Cell::Char { ch, style, .. } => {
                assert_eq!(*ch, '█', "brightest → full block");
                assert_eq!(style.fg, None);
                assert_eq!(style.bg, None);
            }
            other => panic!("expected Char, got {other:?}"),
        }
    }

    #[test]
    fn gif_loop_count_parses_netscape_extension() {
        let mut g = Vec::new();
        g.extend_from_slice(b"GIF89a");
        g.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]); // logical screen descriptor (values irrelevant)
        g.extend_from_slice(&[0x21, 0xFF, 0x0B]);
        g.extend_from_slice(b"NETSCAPE2.0");
        g.extend_from_slice(&[0x03, 0x01, 0x00, 0x00, 0x00]); // loop count 0 = infinite
        assert_eq!(parse_gif_loop_count(&g), Some(0));

        let mut g3 = g.clone();
        let pos = g3.len() - 3; // the loop_lo byte
        g3[pos] = 3;
        assert_eq!(parse_gif_loop_count(&g3), Some(3));

        assert_eq!(parse_gif_loop_count(b"GIF89a not animated"), None);
    }

    fn make_two_frame_gif() -> Vec<u8> {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame};
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            for c in [0u8, 200] {
                let img = RgbaImage::from_pixel(2, 2, Rgba([c, c, c, 255]));
                let frame = Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1));
                enc.encode_frame(frame).unwrap();
            }
        }
        out
    }

    #[test]
    fn decode_animation_reads_frames_or_static() {
        // Static PNG → Static.
        let png = {
            let src = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255]));
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(src)
                .write_to(&mut buf, image::ImageFormat::Png)
                .unwrap();
            buf.into_inner()
        };
        assert!(
            matches!(decode_animation(&png), AnimationDecode::Static),
            "static image is not an animation"
        );

        // Two-frame GIF → Animated with 2 frames.
        let gif = make_two_frame_gif();
        match decode_animation(&gif) {
            AnimationDecode::Animated(anim) => assert_eq!(anim.frames.len(), 2),
            _ => panic!("two-frame GIF should decode as Animated"),
        }
    }

    #[test]
    fn png_has_actl_detects_apng_chunk() {
        // acTL anywhere in the byte stream → treated as an APNG.
        assert!(png_has_actl(b"\x89PNG....acTL....IDAT...."));
        // A plain PNG without the animation-control chunk → not an APNG.
        assert!(!png_has_actl(b"\x89PNG....IHDR....IDAT....IEND"));
    }
}

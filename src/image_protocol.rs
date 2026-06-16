//! Pure terminal-graphics encoders: Kitty graphics protocol and Sixel.
//! Decodes nothing and touches no terminal; callers pass a decoded `RgbaImage`.
//! Mirrors `render`/`image_render` discipline: plain inputs, byte outputs,
//! exhaustively unit-tested.

use crate::render::{rgb_to_256, color_256_to_rgb};
use image::RgbaImage;
use std::collections::BTreeMap;

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (RFC 4648) with `=` padding.
fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Encode an image as a Kitty graphics-protocol "transmit and display" command.
/// Format f=32 (RGBA), chunked at 4096 base64 chars with the m=1/m=0 convention.
pub fn encode_kitty(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let payload = base64_encode(img.as_raw());
    let bytes = payload.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let chunks: Vec<&[u8]> = bytes.chunks(4096).collect();
    let n = chunks.len().max(1);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 < n { 1 } else { 0 };
        out.extend_from_slice(b"\x1b_G");
        if i == 0 {
            out.extend_from_slice(format!("a=T,f=32,s={w},v={h},m={more}").as_bytes());
        } else {
            out.extend_from_slice(format!("m={more}").as_bytes());
        }
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Encode an image as a Sixel data stream. Quantizes each pixel to the xterm
/// 256-color palette via `rgb_to_256` (shared with `--truecolor` downsampling),
/// then emits palette definitions and run-length-compressed sixel bands.
pub fn encode_sixel(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 { return b"\x1bPq\x1b\\".to_vec(); }

    let quant: Vec<u8> = img.pixels().map(|p| rgb_to_256(p.0[0], p.0[1], p.0[2])).collect();

    let mut pal: BTreeMap<u8, usize> = BTreeMap::new();
    for &idx in &quant {
        let next = pal.len();
        pal.entry(idx).or_insert(next);
    }

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"\x1bPq"); // DCS sixel intro: ESC P <params> q
    for (&idx, &p) in &pal {
        let (r, g, b) = color_256_to_rgb(idx);
        let to100 = |v: u8| v as u32 * 100 / 255;
        out.extend_from_slice(
            format!("#{};2;{};{};{}", p, to100(r), to100(g), to100(b)).as_bytes());
    }

    let bands = (h as usize).div_ceil(6);
    for band in 0..bands {
        let y0 = (band * 6) as u32;
        let mut first_color = true;
        for (&idx, &p) in &pal {
            let mut values: Vec<u8> = Vec::with_capacity(w as usize);
            let mut any = false;
            for x in 0..w {
                let mut v = 0u8;
                for r in 0..6u32 {
                    let y = y0 + r;
                    if y < h && quant[(y * w + x) as usize] == idx {
                        v |= 1 << r;
                    }
                }
                if v != 0 { any = true; }
                values.push(v);
            }
            if !any { continue; }
            // carriage-return to band start so the next color overlays the same 6 rows
            if !first_color { out.push(b'$'); }
            first_color = false;
            out.extend_from_slice(format!("#{p}").as_bytes());
            let mut x = 0usize;
            while x < values.len() {
                let v = values[x];
                let mut run = 1usize;
                while x + run < values.len() && values[x + run] == v { run += 1; }
                // sixel byte = 0x3F + 6-bit column mask, keeping it printable ASCII
                let ch = (0x3F + v) as char;
                if run >= 3 {
                    out.extend_from_slice(format!("!{run}{ch}").as_bytes());
                } else {
                    for _ in 0..run { out.push(ch as u8); }
                }
                x += run;
            }
        }
        // graphics newline: advance to the next 6-row band
        if band + 1 < bands { out.push(b'-'); }
    }
    out.extend_from_slice(b"\x1b\\"); // ST: end the DCS string
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"hello, world"), "aGVsbG8sIHdvcmxk");
    }

    #[test]
    fn kitty_has_header_keys_and_terminator() {
        let img = RgbaImage::from_pixel(2, 1, Rgba([1, 2, 3, 255]));
        let out = encode_kitty(&img);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1b_G"));
        assert!(s.contains("a=T,f=32,s=2,v=1,m=0"));
        assert!(s.ends_with("\x1b\\"));
        let expected_payload = base64_encode(img.as_raw());
        assert!(s.contains(&expected_payload));
    }

    #[test]
    fn kitty_chunks_large_payload_with_more_flags() {
        let img = RgbaImage::from_pixel(1000, 1, Rgba([9, 9, 9, 9]));
        let out = encode_kitty(&img);
        let s = String::from_utf8_lossy(&out);
        assert!(s.matches("\x1b_G").count() >= 2, "should split into multiple APCs");
        assert!(s.contains("m=1"), "non-final chunks set m=1");
        assert!(s.contains("m=0"), "final chunk sets m=0");
    }

    #[test]
    fn sixel_has_intro_palette_and_terminator() {
        let img = RgbaImage::from_pixel(2, 6, Rgba([255, 0, 0, 255]));
        let out = encode_sixel(&img);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1bPq"), "DCS sixel intro");
        assert!(s.ends_with("\x1b\\"), "ST terminator");
        assert!(s.contains(";2;"), "RGB palette definition present");
    }

    #[test]
    fn sixel_full_height_uses_all_six_bits() {
        let img = RgbaImage::from_pixel(1, 6, Rgba([255, 255, 255, 255]));
        let out = encode_sixel(&img);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains('~'), "all six rows set → '~' sixel char (0x3F+63)");
    }

    #[test]
    fn sixel_emits_band_separator_for_tall_images() {
        let img = RgbaImage::from_pixel(1, 12, Rgba([10, 20, 30, 255]));
        let out = encode_sixel(&img);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains('-'), "two bands (12px = 2×6) separated by '-'");
    }

    #[test]
    fn sixel_emits_rle_for_long_runs() {
        // A solid 5px-wide, 6px-tall image → one color, a run of 5 identical sixel
        // chars → RLE `!5<char>` (run >= 3 triggers compression).
        let img = RgbaImage::from_pixel(5, 6, Rgba([255, 255, 255, 255]));
        let out = encode_sixel(&img);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("!5"), "a 5-wide solid run should emit RLE !5, got: {s:?}");
    }

    #[test]
    fn sixel_overlays_multiple_colors_in_a_band_with_dollar() {
        // Two distinct colors in the same 6px-tall band (side by side) must be
        // emitted as two color passes separated by `$` (carriage-return to band start).
        let mut img = RgbaImage::new(2, 6);
        for y in 0..6 { img.put_pixel(0, y, Rgba([255, 0, 0, 255])); }   // col 0 red
        for y in 0..6 { img.put_pixel(1, y, Rgba([0, 0, 255, 255])); }   // col 1 blue
        let out = encode_sixel(&img);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains('$'), "two colors in one band must be overlaid with `$`, got: {s:?}");
        // Two distinct palette entries defined.
        assert!(s.matches(";2;").count() >= 2, "two palette colors defined");
    }
}

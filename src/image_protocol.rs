//! Pure terminal-graphics encoders: Kitty graphics protocol and Sixel.
//! Decodes nothing and touches no terminal; callers pass a decoded `RgbaImage`.
//! Mirrors `render`/`image_render` discipline: plain inputs, byte outputs,
//! exhaustively unit-tested.

use image::RgbaImage;

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
}

#![cfg(feature = "image")]
use tess::image_render::{decode_image, render_image, sniff_image_format, AsciiStyle};

fn png_bytes(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba(px));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

#[test]
fn detect_decode_render_roundtrip() {
    let bytes = png_bytes(40, 40, [255, 255, 255, 255]);
    assert_eq!(sniff_image_format(&bytes), Some("png"));
    let img = decode_image(&bytes).unwrap();
    let grid = render_image(&img, 20, AsciiStyle::Ramp, true);
    assert!(grid.iter().all(|r| r.len() == 20));
    assert!(matches!(grid[0][0], tess::render::Cell::Char { ch: '@', .. }));
}

//! Pure image → ASCII-art kernel. Decodes nothing and touches no terminal;
//! callers pass an already-decoded `RgbaImage`. Mirrors `render`'s discipline:
//! plain inputs, plain cell outputs, exhaustively unit-tested.

use crate::ansi::{Color, Style};
use crate::render::Cell;
use image::RgbaImage;

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

// Temporary stub; the real half-block implementation lands in Task 4.
fn render_blocks(img: &RgbaImage, cols: u16, color: bool) -> Vec<Vec<Cell>> {
    render_ramp(img, cols, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

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
}

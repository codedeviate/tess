//! Serialize a rendered ASCII-art cell grid to a writer: ANSI-colored rows by
//! default, or plain text when color is off.

use crate::ansi::Color;
use crate::render::Cell;
use std::io::{self, Write};

/// Write `grid` to `out`. When `color` is true, emit truecolor SGR for fg (and
/// bg, for half-block cells); otherwise emit just the characters. One line per
/// row, reset (`\x1b[0m`) before each newline when colored.
pub fn write_grid<W: Write>(out: &mut W, grid: &[Vec<Cell>], color: bool) -> io::Result<()> {
    for row in grid {
        for cell in row {
            match cell {
                Cell::Char { ch, style, .. } => {
                    if color {
                        if let Some(Color::Rgb(r, g, b)) = style.fg {
                            write!(out, "\x1b[38;2;{r};{g};{b}m")?;
                        }
                        if let Some(Color::Rgb(r, g, b)) = style.bg {
                            write!(out, "\x1b[48;2;{r};{g};{b}m")?;
                        }
                    }
                    write!(out, "{ch}")?;
                }
                Cell::Continuation => {}
                Cell::Empty => write!(out, " ")?,
            }
        }
        if color { write!(out, "\x1b[0m")?; }
        writeln!(out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::Style;

    fn ch(c: char, fg: Option<Color>) -> Cell {
        Cell::Char { ch: c, width: 1, style: Style { fg, bg: None, ..Default::default() }, hyperlink: None }
    }

    #[test]
    fn plain_writes_chars_only() {
        let grid = vec![vec![ch('@', Some(Color::Rgb(1, 2, 3))), ch('.', None)]];
        let mut buf = Vec::new();
        write_grid(&mut buf, &grid, false).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "@.\n");
    }

    #[test]
    fn color_emits_truecolor_sgr_and_reset() {
        let grid = vec![vec![ch('@', Some(Color::Rgb(10, 20, 30)))]];
        let mut buf = Vec::new();
        write_grid(&mut buf, &grid, true).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "\x1b[38;2;10;20;30m@\x1b[0m\n");
    }
}

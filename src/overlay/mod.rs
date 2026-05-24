//! Overlay subsystem: full-screen popups that take over the body+status
//! area (file picker, help). App holds an `Option<Box<dyn Overlay>>`;
//! events route through it first when present.

use std::borrow::Cow;
use crossterm::event::{KeyEvent, MouseEvent};

use crate::input::Command;

pub mod picker;
pub mod help;
pub mod tag_picker;

#[derive(Debug)]
pub struct OverlayFrame {
    /// Body lines (no styling info — overlay renders are plain text in v1).
    pub body: Vec<String>,
    /// Status line (rendered as the last row, like the regular status).
    pub status: String,
}

#[derive(Debug)]
pub enum OverlayOutcome {
    /// Keep the overlay open, just redraw.
    Stay,
    /// Close the overlay; no follow-up command.
    Close,
    /// Close the overlay, then dispatch one command through the normal app loop.
    CloseAnd(Command),
    /// Stay open; dispatch one command, then call `refresh` on the overlay
    /// so it can re-derive its visible set from the new app state.
    Apply(Command),
    /// Stay open; flash a short message in the status row for ~1.5 seconds.
    Refuse(&'static str),
}

/// Borrowed slice of app state that overlays need at refresh time. Kept
/// narrow so the trait doesn't pull in the whole app.
pub struct OverlayContext<'a> {
    pub file_set: &'a crate::file_set::FileSet,
}

pub trait Overlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayOutcome;
    fn handle_mouse(&mut self, _ev: MouseEvent, _body_rows: u16) -> OverlayOutcome {
        OverlayOutcome::Stay
    }
    /// Render against a (width, height) viewport. Height includes the status row.
    fn render(&self, width: u16, height: u16) -> OverlayFrame;
    fn title(&self) -> Cow<'_, str>;
    /// Called after `Apply(cmd)` dispatches, so the overlay can re-derive
    /// state (e.g. picker rebuilds visible after a DropFileAt).
    fn refresh(&mut self, _ctx: OverlayContext) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    // A trivial overlay used only to verify the trait compiles and dispatch
    // path can box it.
    struct Noop;
    impl Overlay for Noop {
        fn handle_key(&mut self, _k: KeyEvent) -> OverlayOutcome { OverlayOutcome::Close }
        fn render(&self, _w: u16, _h: u16) -> OverlayFrame {
            OverlayFrame { body: vec![], status: String::new() }
        }
        fn title(&self) -> Cow<'_, str> { Cow::Borrowed("noop") }
    }

    #[test]
    fn overlay_trait_is_object_safe() {
        let _: Box<dyn Overlay> = Box::new(Noop);
    }
}

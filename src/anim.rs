//! Pure animation-playback state. No terminal, no clock — advanced by explicit
//! `Duration` deltas the caller computes from an `Instant`, so the transport
//! logic (frame advance, loop-count termination, pause/step/restart) is fully
//! unit-testable. Mirrors the project's pure-kernel discipline.

use image::RgbaImage;
use std::time::Duration;

/// Frames with a delay below this are treated as this long, matching common
/// viewer/browser behavior (0-delay or absurdly fast GIFs must not busy-loop).
const MIN_FRAME_DELAY: Duration = Duration::from_millis(100);
const TINY: Duration = Duration::from_millis(10);

pub struct AnimationState {
    frames: Vec<(RgbaImage, Duration)>,
    loop_count: Option<u32>, // None = infinite
    current: usize,
    elapsed_in_frame: Duration,
    playing: bool,
    loops_done: u32,
    finished: bool,
}

impl AnimationState {
    /// Build from decoded frames + loop count. Starts playing at frame 0.
    pub fn new(frames: Vec<(RgbaImage, Duration)>, loop_count: Option<u32>) -> Self {
        Self {
            frames, loop_count, current: 0, elapsed_in_frame: Duration::ZERO,
            playing: true, loops_done: 0, finished: false,
        }
    }

    pub fn current_frame(&self) -> &RgbaImage { &self.frames[self.current].0 }
    pub fn frame_index(&self) -> usize { self.current }
    pub fn frame_count(&self) -> usize { self.frames.len() }
    pub fn is_playing(&self) -> bool { self.playing }
    pub fn is_finished(&self) -> bool { self.finished }

    fn frame_delay(&self, i: usize) -> Duration {
        let d = self.frames[i].1;
        if d < TINY { MIN_FRAME_DELAY } else { d }
    }

    /// Accumulate `dt`, flipping to later frames as each delay elapses (a large
    /// dt may cross several frames). On wrapping past the last frame, count a
    /// loop; if a finite loop count is then reached, finish and rest on the last
    /// frame. No-op when paused, finished, or single-frame. Returns whether the
    /// displayed frame changed.
    pub fn advance(&mut self, dt: Duration) -> bool {
        if !self.playing || self.finished || self.frames.len() <= 1 { return false; }
        let start = self.current;
        self.elapsed_in_frame += dt;
        loop {
            let delay = self.frame_delay(self.current);
            if self.elapsed_in_frame < delay { break; }
            self.elapsed_in_frame -= delay;
            if self.current + 1 < self.frames.len() {
                self.current += 1;
            } else {
                self.loops_done += 1;
                if let Some(n) = self.loop_count {
                    if self.loops_done >= n {
                        self.finished = true;
                        self.playing = false;
                        self.current = self.frames.len() - 1;
                        self.elapsed_in_frame = Duration::ZERO;
                        break;
                    }
                }
                self.current = 0;
            }
        }
        self.current != start
    }

    /// Time until the next frame flip, or None when paused/finished/single-frame.
    pub fn next_deadline(&self) -> Option<Duration> {
        if !self.playing || self.finished || self.frames.len() <= 1 { return None; }
        Some(self.frame_delay(self.current).saturating_sub(self.elapsed_in_frame))
    }

    /// Play/pause toggle. Reviving a finished animation restarts it (so the
    /// play key brings a stopped GIF back to life).
    pub fn toggle_pause(&mut self) {
        if self.finished { self.restart(); return; }
        self.playing = !self.playing;
    }

    /// Pause and move ±1 frame (wrapping). Clears the finished state.
    pub fn step(&mut self, delta: i32) {
        self.playing = false;
        self.finished = false;
        self.elapsed_in_frame = Duration::ZERO;
        let n = self.frames.len() as i64;
        if n == 0 { return; }
        let next = ((self.current as i64 + delta as i64) % n + n) % n;
        self.current = next as usize;
    }

    /// Restart from frame 0, playing, loop counter reset.
    pub fn restart(&mut self) {
        self.current = 0;
        self.elapsed_in_frame = Duration::ZERO;
        self.loops_done = 0;
        self.finished = false;
        self.playing = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn frames(n: usize, delay_ms: u64) -> Vec<(RgbaImage, Duration)> {
        (0..n).map(|i| (RgbaImage::from_pixel(1, 1, Rgba([i as u8, 0, 0, 255])),
                        Duration::from_millis(delay_ms))).collect()
    }

    #[test]
    fn advances_at_delay_boundary() {
        let mut a = AnimationState::new(frames(3, 100), None);
        assert_eq!(a.frame_index(), 0);
        assert!(!a.advance(Duration::from_millis(50)));
        assert_eq!(a.frame_index(), 0);
        assert!(a.advance(Duration::from_millis(60)));
        assert_eq!(a.frame_index(), 1);
    }

    #[test]
    fn large_dt_crosses_multiple_frames() {
        let mut a = AnimationState::new(frames(4, 100), None);
        assert!(a.advance(Duration::from_millis(250)));
        assert_eq!(a.frame_index(), 2);
    }

    #[test]
    fn infinite_loop_never_finishes() {
        let mut a = AnimationState::new(frames(2, 100), None);
        a.advance(Duration::from_millis(1000));
        assert!(!a.is_finished());
        assert!(a.is_playing());
    }

    #[test]
    fn finite_loop_finishes_on_last_frame() {
        let mut a = AnimationState::new(frames(3, 100), Some(1));
        a.advance(Duration::from_millis(300));
        assert!(a.is_finished());
        assert!(!a.is_playing());
        assert_eq!(a.frame_index(), 2);
        assert_eq!(a.next_deadline(), None);
        assert!(!a.advance(Duration::from_millis(500)));
    }

    #[test]
    fn pause_blocks_advance_and_resume_restores() {
        let mut a = AnimationState::new(frames(3, 100), None);
        a.toggle_pause();
        assert!(!a.is_playing());
        assert!(!a.advance(Duration::from_millis(500)));
        assert_eq!(a.frame_index(), 0);
        a.toggle_pause();
        assert!(a.is_playing());
    }

    #[test]
    fn step_wraps_and_pauses() {
        let mut a = AnimationState::new(frames(3, 100), None);
        a.step(-1);
        assert_eq!(a.frame_index(), 2);
        assert!(!a.is_playing());
        a.step(1);
        assert_eq!(a.frame_index(), 0);
    }

    #[test]
    fn restart_resets() {
        let mut a = AnimationState::new(frames(3, 100), Some(1));
        a.advance(Duration::from_millis(300));
        a.restart();
        assert_eq!(a.frame_index(), 0);
        assert!(a.is_playing());
        assert!(!a.is_finished());
    }

    #[test]
    fn toggle_pause_revives_finished() {
        let mut a = AnimationState::new(frames(2, 100), Some(1));
        a.advance(Duration::from_millis(200));
        assert!(a.is_finished());
        a.toggle_pause();
        assert!(a.is_playing() && !a.is_finished() && a.frame_index() == 0);
    }

    #[test]
    fn zero_delay_floored_not_busy_loop() {
        let mut a = AnimationState::new(frames(2, 0), None);
        assert!(!a.advance(Duration::from_millis(50)));
        assert!(a.advance(Duration::from_millis(60)));
    }
}

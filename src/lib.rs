#![allow(rustdoc::broken_intra_doc_links)]
#![doc = include_str!("../README.md")]

pub mod ansi;
pub mod error;
pub mod render;
pub mod source;
pub mod line_index;
pub mod marks;
pub mod viewport;
pub mod input;
pub mod terminal;
pub mod app;
pub mod cli;
pub mod format;
pub mod file_set;
pub mod filter;
pub mod prompt;
pub mod grep;
pub mod hex;
pub mod prettify;
pub mod batch;
pub mod clipboard;
pub mod shell;
pub mod preprocess;
pub mod keys;
pub mod style_spec;
pub mod open;
pub mod or;
pub mod overlay;
pub mod pane;
pub mod tags;
pub mod keymap;
pub mod config_path;
pub mod charset;
pub mod diff;
pub mod git;
pub mod diff_view;
#[cfg(feature = "image")]
pub mod anim;
#[cfg(feature = "image")]
pub mod image_render;
#[cfg(feature = "image")]
pub mod image_export;
#[cfg(feature = "image")]
pub mod image_protocol;
#[cfg(feature = "image")]
pub mod term_query;

#[cfg(test)]
pub(crate) mod test_env {
    use std::sync::{Mutex, MutexGuard};

    /// Process-wide lock serializing every test that mutates the global
    /// `HOME` / `TESS_GLOBAL_CONFIG_DIR` env vars. Those vars are process
    /// global, so a per-module lock can't stop tests in *different* modules
    /// (format, keys, config_path, app) from trampling each other under
    /// cargo's parallel runner. One shared lock is the only thing that
    /// actually serializes them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Poison-tolerant acquire: a test that panics while holding the lock
    /// must not cascade `PoisonError` into every other env-mutating test.
    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}

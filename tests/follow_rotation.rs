//! Integration tests for follow-mode file rotation and truncation
//! detection. Exercises `FileSource::pump`'s rotation check and the
//! `take_rotated()` flag the app loop reacts to.

use std::io::Write;
use std::thread;
use std::time::Duration;

use tess::source::{FileSource, Source};

/// Helper: sleep past macOS / Linux filesystem mtime granularity (~1 s).
fn wait_for_mtime_tick() {
    thread::sleep(Duration::from_millis(1100));
}

#[test]
fn truncation_sets_rotation_flag_on_next_pump() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "first line").unwrap();
        writeln!(f, "second line").unwrap();
    }

    let src = FileSource::open(&path).unwrap();
    // Initial pump establishes baseline; nothing rotated yet.
    src.pump();
    assert!(!src.take_rotated(), "fresh source should not be flagged rotated");

    // Truncate to a smaller content. Mtime ticks past the load mtime
    // because we re-create the file fresh.
    wait_for_mtime_tick();
    {
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "tiny").unwrap();
    }

    src.pump();
    assert!(src.take_rotated(), "truncation should set the rotation flag");
    // Flag is one-shot.
    assert!(!src.take_rotated());
}

#[test]
fn rotation_sets_rotation_flag_on_next_pump() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "original content line").unwrap();
    }

    let src = FileSource::open(&path).unwrap();
    src.pump();
    assert!(!src.take_rotated());

    // Rotate: move the original aside, create a brand-new file at the
    // same path. This produces a new inode under POSIX semantics.
    wait_for_mtime_tick();
    let rotated_path = dir.path().join("log.1");
    std::fs::rename(&path, &rotated_path).unwrap();
    {
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "fresh content after rotation").unwrap();
    }

    src.pump();
    assert!(
        src.take_rotated(),
        "rotation (rename + recreate) should set the rotation flag",
    );
}

#[test]
fn appending_to_unchanged_file_does_not_flag_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "starter").unwrap();
    }

    let src = FileSource::open(&path).unwrap();
    src.pump();

    // Append more content using append-mode (no truncate, same inode).
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "second").unwrap();
        writeln!(f, "third").unwrap();
    }

    src.pump();
    assert!(!src.take_rotated(), "plain appends must not flag rotation");
}

#[test]
fn path_accessor_returns_open_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    std::fs::write(&path, b"hi\n").unwrap();
    let src = FileSource::open(&path).unwrap();
    let trait_obj: &dyn Source = &src;
    assert_eq!(trait_obj.path(), Some(path.as_path()));
}

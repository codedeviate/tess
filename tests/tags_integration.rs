//! End-to-end tag-jumping tests via the real tess binary.

use std::io::Write;
use std::time::Duration;

fn write_tags(tmpdir: &std::path::Path, entries: &[(&str, &str, &str)]) -> std::path::PathBuf {
    let path = tmpdir.join("tags");
    let mut f = std::fs::File::create(&path).unwrap();
    for (name, file, addr) in entries {
        writeln!(f, "{name}\t{file}\t{addr}").unwrap();
    }
    path
}

#[test]
fn startup_t_flag_jumps_to_tag() {
    let bin = env!("CARGO_BIN_EXE_tess");
    let tmpdir = tempfile::tempdir().unwrap();
    let src = tmpdir.path().join("src.txt");
    std::fs::write(&src, b"line 1\nline 2\nline 3\nfoo definition\nline 5\n").unwrap();
    let tags = write_tags(tmpdir.path(), &[("foo", "src.txt", "4")]);

    let mut command = std::process::Command::new(bin);
    command.arg("-T").arg(&tags).arg("-t").arg("foo").arg(&src);
    let mut s = expectrl::Session::spawn(command).expect("spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(400));
    s.send("q").unwrap();
    let _ = s.expect(expectrl::Eof);
    match s.get_process().wait().unwrap() {
        expectrl::WaitStatus::Exited(_, code) => assert_eq!(code, 0),
        other => panic!("unexpected wait status: {other:?}"),
    }
}

#[test]
fn colon_tag_jumps_at_runtime_and_ctrl_t_returns() {
    let bin = env!("CARGO_BIN_EXE_tess");
    let tmpdir = tempfile::tempdir().unwrap();
    let a = tmpdir.path().join("a.txt");
    let b = tmpdir.path().join("b.txt");
    std::fs::write(&a, b"alpha 1\nalpha 2\n").unwrap();
    std::fs::write(&b, b"beta 1\nbeta 2\nbeta 3\n").unwrap();
    let tags = write_tags(tmpdir.path(), &[("bar", "b.txt", "2")]);

    let mut command = std::process::Command::new(bin);
    command.arg("-T").arg(&tags).arg(&a);
    let mut s = expectrl::Session::spawn(command).expect("spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(300));
    s.send(":tag bar\r").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    s.send("\x14").unwrap();   // Ctrl-T
    std::thread::sleep(Duration::from_millis(300));
    s.send("q").unwrap();
    let _ = s.expect(expectrl::Eof);
    match s.get_process().wait().unwrap() {
        expectrl::WaitStatus::Exited(_, code) => assert_eq!(code, 0),
        other => panic!("unexpected wait status: {other:?}"),
    }
}

#[test]
fn tnext_cycles_through_multiple_matches() {
    let bin = env!("CARGO_BIN_EXE_tess");
    let tmpdir = tempfile::tempdir().unwrap();
    let a = tmpdir.path().join("a.txt");
    let b = tmpdir.path().join("b.txt");
    let c = tmpdir.path().join("c.txt");
    std::fs::write(&a, b"foo def 1\n").unwrap();
    std::fs::write(&b, b"foo def 2\n").unwrap();
    std::fs::write(&c, b"foo def 3\n").unwrap();
    let tags = write_tags(
        tmpdir.path(),
        &[
            ("foo", "a.txt", "1"),
            ("foo", "b.txt", "1"),
            ("foo", "c.txt", "1"),
        ],
    );

    let mut command = std::process::Command::new(bin);
    command.arg("-T").arg(&tags).arg("-t").arg("foo").arg(&a);
    let mut s = expectrl::Session::spawn(command).expect("spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(300));
    s.send(":tnext\r").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.send(":tnext\r").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.send(":tnext\r").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.send("q").unwrap();
    let _ = s.expect(expectrl::Eof);
    match s.get_process().wait().unwrap() {
        expectrl::WaitStatus::Exited(_, code) => assert_eq!(code, 0),
        other => panic!("unexpected wait status: {other:?}"),
    }
}

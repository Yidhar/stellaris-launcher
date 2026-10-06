//! A game started by `process::spawn` must not keep our standard pipes open: a script or a window that runs `stl launch` and reads its output
//! would otherwise wait for the game to exit. The test re-runs itself as a helper that spawns a short-lived program and returns at once.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn ping() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32").join("PING.EXE")
}

#[test]
fn spawned_process_does_not_keep_our_pipes_open() {
    if std::env::var_os("STL_SPAWN_HELPER").is_some() {
        let args: Vec<String> = ["-n", "8", "127.0.0.1"].iter().map(|s| s.to_string()).collect();
        if std::env::var_os("STL_SPAWN_NOFIX").is_some() {
            // what spawn() did before it cleared the inheritable flags: detached, but with the pipes still inheritable
            Command::new(ping()).args(&args).creation_flags(0x0000_0008).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        } else {
            stl_core::process::spawn(&ping(), &args, &std::env::temp_dir(), &[], None).unwrap();
        }
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args(["--exact", "spawned_process_does_not_keep_our_pipes_open", "--nocapture", "--test-threads=1"])
        .env("STL_SPAWN_HELPER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut err = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = err.read_to_end(&mut sink);
    });
    let start = Instant::now();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    reader.join().unwrap();
    let elapsed = start.elapsed();
    let _ = child.wait();
    if std::env::var_os("STL_EXPECT_HANG").is_some() {
        assert!(elapsed > Duration::from_secs(5), "the helper without the fix should hold the pipes ({elapsed:?})");
    } else {
        assert!(elapsed < Duration::from_secs(4), "the pipes stayed open for {elapsed:?}: the spawned program inherited them");
    }
}

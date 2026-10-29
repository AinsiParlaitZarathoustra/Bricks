//! Process stops, measured on the process itself: after every wait the exit
//! watcher thread (`bricks-exit-watch`) has ended, no descriptor is left
//! open, and no stopped process stays a zombie. Unix only. This file holds
//! one test on purpose: the thread and descriptor counts are per process,
//! and a second test running alongside would move them.

#![cfg(unix)]

use bricks_tools::shell::procs;
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

/// Threads of this process: on Linux the exit watchers alone, by name (the
/// runtime's other threads come and go and would move a total); elsewhere
/// every thread, one row of `ps -M` per thread after a header.
fn threads() -> usize {
    if cfg!(target_os = "linux") {
        // `comm` holds the first 15 bytes of the thread's name.
        return std::fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(|t| std::fs::read_to_string(t.ok()?.path().join("comm")).ok())
            .filter(|name| {
                let name = name.trim_end();
                !name.is_empty() && "bricks-exit-watch".starts_with(name)
            })
            .count();
    }
    let out = std::process::Command::new("ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .count()
        .saturating_sub(1)
}

/// Open descriptors of this process.
fn open_fds() -> usize {
    let dir = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    std::fs::read_dir(dir).unwrap().count()
}

/// Polls (bounded) until the thread count is back at `at_most`. Watcher
/// threads end as soon as their watch is dropped, not in the same instant.
async fn threads_back_to(at_most: usize) -> usize {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = threads();
        if now <= at_most || Instant::now() >= deadline {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn sleeper() -> Child {
    Command::new("sleep")
        .arg("30")
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

fn gone(pid: u32) -> bool {
    kill(Pid::from_raw(pid as i32), None) == Err(Errno::ESRCH)
}

#[tokio::test]
async fn every_wait_ends_its_watcher_and_keeps_no_descriptor_or_zombie() {
    // Warm-up: the first stop starts whatever the process needs once.
    let mut warm = sleeper();
    let wpid = warm.id().unwrap();
    procs::terminate(vec![wpid], Duration::from_secs(2)).await;
    let _ = warm.wait().await;
    let base_threads = threads_back_to(threads()).await;
    let base_fds = open_fds();

    // The instrument: while a wait is pending, one more thread exists.
    let mut live = sleeper();
    let lpid = live.id().unwrap();
    let waiter =
        tokio::spawn(async move { procs::wait_gone(&[lpid], Duration::from_secs(10)).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let during = threads();
    assert!(
        during > base_threads,
        "a pending wait has its watcher thread ({base_threads} before, {during} during)"
    );
    live.start_kill().unwrap();
    assert!(waiter.await.unwrap(), "the killed process is gone");
    let _ = live.wait().await;
    let after_instrument = threads_back_to(base_threads).await;
    assert!(
        after_instrument <= base_threads,
        "the watcher ended with its wait: {after_instrument} threads, {base_threads} before"
    );

    // Many stops: TERM-obeying ones, waits that hit their deadline with the
    // process still running (the watch is dropped then), and TERM-resistant
    // ones forced with KILL.
    let mut pids = Vec::new();
    for i in 0..30 {
        let mut c = sleeper();
        let pid = c.id().unwrap();
        pids.push(pid);
        if i % 3 == 0 {
            assert!(
                !procs::wait_gone(&[pid], Duration::from_millis(20)).await,
                "still running at the deadline"
            );
        }
        let t = procs::terminate(vec![pid], Duration::from_secs(2)).await;
        assert_eq!(t.targeted, vec![pid]);
        assert!(t.forced.is_empty(), "sleep obeys TERM: {t:?}");
        assert!(t.survivors.is_empty(), "{t:?}");
        let _ = c.wait().await;
    }
    for _ in 0..3 {
        let mut c = Command::new("sh")
            .args(["-c", "trap '' TERM; while :; do sleep 0.05; done"])
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = c.id().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let t = procs::terminate(vec![pid], Duration::from_millis(100)).await;
        assert_eq!(t.forced, vec![pid], "{t:?}");
        assert!(t.survivors.is_empty(), "{t:?}");
        let _ = c.wait().await;
        pids.push(pid);
    }

    let after = threads_back_to(base_threads).await;
    assert!(
        after <= base_threads,
        "{} stops left {after} threads, {base_threads} before",
        pids.len() + 1
    );
    let fds = open_fds();
    assert!(
        fds <= base_fds,
        "descriptors: {base_fds} before, {fds} after"
    );
    let alive: Vec<u32> = pids.iter().copied().filter(|&p| !gone(p)).collect();
    assert!(
        alive.is_empty(),
        "stopped and reaped, none left as zombie: {alive:?}"
    );
}

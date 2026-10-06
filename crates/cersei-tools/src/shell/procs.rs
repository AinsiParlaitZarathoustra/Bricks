//! Process supervision helpers.
//!
//! **Unix.** Supervised trees are found by walking parent links in the
//! process table (`ps -A -o pid= -o ppid=`), so processes that changed group
//! or session are still found while they remain descendants. Termination is
//! graceful first (`SIGTERM`), then forced (`SIGKILL`) after a bounded
//! delay, and verified: a process counts as stopped only when it is gone or a
//! zombie. Limits: a process that double-forks and is re-parented to init
//! (a daemon that detaches on purpose) is no longer a descendant and cannot be
//! found this way; neither can anything after Bricks itself is killed with
//! `SIGKILL`.
//!
//! **Windows.** Trees are terminated with `taskkill /T /F`, which follows
//! parent links in the same way. Job Objects are not used yet (see
//! `docs/shell.md`).

use std::time::Duration;

/// Outcome of terminating a set of processes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Termination {
    /// Processes that were running when termination started.
    pub targeted: Vec<u32>,
    /// Of those, the ones that needed the forced signal.
    pub forced: Vec<u32>,
    /// Still running after the forced signal (should be empty).
    pub survivors: Vec<u32>,
}

#[cfg(unix)]
mod imp {
    use super::Termination;
    use nix::sys::signal::{kill, killpg, Signal};
    use nix::unistd::Pid;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    /// Parent → children map of the whole process table.
    fn process_table() -> HashMap<u32, Vec<u32>> {
        let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
        let out = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=", "-o", "ppid="])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let Ok(out) = out else { return map };
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split_whitespace();
            if let (Some(pid), Some(ppid)) = (it.next(), it.next()) {
                if let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) {
                    map.entry(ppid).or_default().push(pid);
                }
            }
        }
        map
    }

    /// Every live descendant of `root` (not `root` itself), parents first.
    pub fn descendants(root: u32) -> Vec<u32> {
        let table = process_table();
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(p) = stack.pop() {
            if let Some(children) = table.get(&p) {
                for &c in children {
                    if c != root && !out.contains(&c) {
                        out.push(c);
                        stack.push(c);
                    }
                }
            }
        }
        out.retain(|&p| is_running(p));
        out
    }

    /// Running, and not a zombie.
    pub fn is_running(pid: u32) -> bool {
        if kill(Pid::from_raw(pid as i32), None).is_err() {
            return false;
        }
        let stat = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        match stat {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                let s = s.trim();
                !s.is_empty() && !s.starts_with('Z')
            }
            Err(_) => true,
        }
    }

    fn signal(pids: &[u32], sig: Signal) {
        for &p in pids {
            let _ = kill(Pid::from_raw(p as i32), sig);
        }
    }

    pub fn signal_group(pgid: u32, force: bool) {
        let sig = if force {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        let _ = killpg(Pid::from_raw(pgid as i32), sig);
    }

    /// `SIGTERM`, wait up to `grace`, `SIGKILL` the rest, verify.
    pub async fn terminate(pids: Vec<u32>, grace: Duration) -> Termination {
        let targeted: Vec<u32> = pids.into_iter().filter(|&p| is_running(p)).collect();
        if targeted.is_empty() {
            return Termination::default();
        }
        signal(&targeted, Signal::SIGTERM);
        let deadline = Instant::now() + grace;
        let mut alive: Vec<u32> = targeted.clone();
        while !alive.is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
            alive.retain(|&p| is_running(p));
        }
        let forced = alive.clone();
        if !forced.is_empty() {
            signal(&forced, Signal::SIGKILL);
            let deadline = Instant::now() + Duration::from_secs(2);
            while !alive.is_empty() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(25)).await;
                alive.retain(|&p| is_running(p));
            }
        }
        Termination {
            targeted,
            forced,
            survivors: alive,
        }
    }

    pub fn terminate_tree_now(root: u32) {
        let mut all = descendants(root);
        all.push(root);
        signal(&all, Signal::SIGKILL);
    }
}

#[cfg(windows)]
mod imp {
    use super::Termination;
    use std::time::Duration;

    /// Not enumerated on Windows; trees are terminated with `taskkill /T`.
    pub fn descendants(_root: u32) -> Vec<u32> {
        Vec::new()
    }

    pub fn is_running(pid: u32) -> bool {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }

    pub fn signal_group(root: u32, _force: bool) {
        terminate_tree_now(root);
    }

    pub async fn terminate(pids: Vec<u32>, _grace: Duration) -> Termination {
        let targeted: Vec<u32> = pids.into_iter().filter(|&p| is_running(p)).collect();
        for &p in &targeted {
            terminate_tree_now(p);
        }
        let survivors = targeted
            .iter()
            .copied()
            .filter(|&p| is_running(p))
            .collect();
        Termination {
            forced: targeted.clone(),
            targeted,
            survivors,
        }
    }

    pub fn terminate_tree_now(root: u32) {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &root.to_string()])
            .output();
    }
}

pub use imp::{descendants, is_running, signal_group, terminate, terminate_tree_now};

/// Wait until none of `pids` is running, up to `limit`.
pub async fn wait_gone(pids: &[u32], limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if pids.iter().all(|&p| !is_running(p)) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_tree_is_found_and_terminated() {
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "sleep 30 & sleep 30 & wait"])
            .spawn()
            .unwrap();
        let root = child.id().unwrap();
        // Wait until both sleeps exist.
        let mut kids = Vec::new();
        for _ in 0..100 {
            kids = descendants(root);
            if kids.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(kids.len(), 2, "{kids:?}");
        let t = terminate(kids.clone(), Duration::from_secs(2)).await;
        assert!(t.survivors.is_empty());
        assert!(wait_gone(&kids, Duration::from_secs(2)).await);
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn sigterm_resistant_processes_are_forced() {
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "trap '' TERM; while :; do sleep 0.05; done"])
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let t = terminate(vec![pid], Duration::from_millis(200)).await;
        assert_eq!(t.forced, vec![pid]);
        let _ = child.wait().await;
        assert!(!is_running(pid));
    }
}

//! Process supervision helpers.
//!
//! **Unix.** Supervised trees are found by walking parent links in the
//! process table (one `ps` pass), so processes that changed group or session
//! are still found while they remain descendants. Termination is graceful
//! first (`SIGTERM`), then forced (`SIGKILL`) after a bounded delay, and
//! verified: a process counts as stopped once it has exited (a zombie has).
//!
//! Waiting does not poll. A watcher thread waits in the kernel for the exits
//! of the watched processes: `kqueue` with `EVFILT_PROC`/`NOTE_EXIT` on macOS,
//! a `pidfd` per process in an `epoll` set on Linux. It writes each exit to a
//! pipe, and the wait is an `AsyncFd` readiness on that pipe, under the same
//! deadline as before; no blocking call runs on a tokio thread. Where no
//! notification is available (`ENOSYS`, other platforms, or [`Mode::Poll`]),
//! the wait checks `kill(pid, 0)` every 25 ms, within the same deadline. The
//! poll fallback counts a zombie as running until its parent reaps it.
//!
//! Limits: a process that double-forks and is re-parented to init (a daemon
//! that detaches on purpose) is no longer a descendant and cannot be found
//! this way; neither can anything after Bricks itself is killed with
//! `SIGKILL`. A PID recycled between the process-table pass and the
//! registration of its watch cannot be told from its previous owner: the
//! window is a few milliseconds, and no start-time identity is read.
//!
//! **Windows.** Trees are terminated with `taskkill /T /F`, which follows
//! parent links in the same way. Waiting still polls `tasklist` every 25 ms
//! (no notification is implemented there). Job Objects are not used yet (see
//! `docs/shell.md`).

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

/// How the exit of watched processes is learnt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The kernel's notification when there is one, else polling.
    Auto,
    /// Polling `kill(pid, 0)` only (tests, and platforms without
    /// notifications).
    Poll,
}

#[cfg(unix)]
mod imp {
    use super::{Mode, Termination};
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill, killpg};
    use nix::unistd::Pid;
    use std::collections::{BTreeSet, HashMap};
    use std::os::fd::OwnedFd;
    use std::time::{Duration, Instant};
    use tokio::io::{Interest, unix::AsyncFd};

    /// Interval of the poll fallback.
    const POLL: Duration = Duration::from_millis(25);
    /// Time given to a forced stop to be seen.
    const FORCED_WAIT: Duration = Duration::from_secs(2);

    /// One row of the process table.
    struct Row {
        pid: u32,
        ppid: u32,
        pgid: u32,
        zombie: bool,
    }

    /// The whole process table, from one `ps` run.
    fn process_table() -> Vec<Row> {
        let out = std::process::Command::new("ps")
            .args([
                "-A", "-o", "pid=", "-o", "ppid=", "-o", "pgid=", "-o", "stat=",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let Ok(out) = out else { return Vec::new() };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let mut it = line.split_whitespace();
                let pid = it.next()?.parse().ok()?;
                let ppid = it.next()?.parse().ok()?;
                let pgid = it.next()?.parse().ok()?;
                let zombie = it.next().is_some_and(|s| s.starts_with('Z'));
                Some(Row {
                    pid,
                    ppid,
                    pgid,
                    zombie,
                })
            })
            .collect()
    }

    /// Every live descendant of `root` (not `root` itself).
    pub fn descendants(root: u32) -> Vec<u32> {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for row in process_table().into_iter().filter(|r| !r.zombie) {
            children.entry(row.ppid).or_default().push(row.pid);
        }
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(p) = stack.pop() {
            if let Some(kids) = children.get(&p) {
                for &c in kids {
                    if c != root && !out.contains(&c) {
                        out.push(c);
                        stack.push(c);
                    }
                }
            }
        }
        out
    }

    /// Running, and not a zombie.
    pub fn is_running(pid: u32) -> bool {
        if pid == 0 || kill(Pid::from_raw(pid as i32), None).is_err() {
            return false;
        }
        process_table().iter().any(|r| r.pid == pid && !r.zombie)
    }

    /// Whether any process is left in the group `pgid` (cheap: no table read).
    pub fn group_alive(pgid: u32) -> bool {
        pgid != 0 && killpg(Pid::from_raw(pgid as i32), None).is_ok()
    }

    /// Live processes whose process group is `pgid` (the group outlives its
    /// leader while members remain: leftovers of a finished command).
    pub fn group_members(pgid: u32) -> Vec<u32> {
        if pgid == 0 {
            return Vec::new();
        }
        process_table()
            .into_iter()
            .filter(|r| !r.zombie && r.pgid == pgid && r.pid != pgid)
            .map(|r| r.pid)
            .collect()
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

    /// What `kill(pid, 0)` says about a process.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Probe {
        Running,
        Gone,
        /// Exists, but not ours to signal (`EPERM`).
        Foreign,
    }

    fn probe(pid: u32) -> Probe {
        match kill(Pid::from_raw(pid as i32), None) {
            Ok(()) => Probe::Running,
            Err(Errno::ESRCH) => Probe::Gone,
            Err(_) => Probe::Foreign,
        }
    }

    /// The pieces of one wait: the report pipe (its read end is what the
    /// wait reads), the wake pipe's write end (closing it ends the watcher),
    /// and the pids that are running now.
    pub(crate) type Opened = (OwnedFd, OwnedFd, Vec<u32>);

    /// Kernel notifications of process exits, with no blocking call on a
    /// tokio thread.
    ///
    /// A watcher thread waits in the kernel (`kevent` on macOS, `epoll_wait`
    /// on Linux) for the exits of the watched processes, and writes each one
    /// as a 4-byte record to a pipe. The wait reads that pipe through an
    /// `AsyncFd`. The watcher also watches the read end of a wake pipe: when
    /// the write end is closed (the watch was dropped), the watcher ends and
    /// closes what it owns.
    ///
    /// The kqueue is not wrapped in an `AsyncFd` directly: tokio cannot
    /// register it on macOS (`EINVAL`), so the thread waits on it.
    mod kernel {
        use super::Opened;
        use std::io;
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

        /// Marks the wake pipe in the epoll set (no pid is `u64::MAX`).
        #[cfg(target_os = "linux")]
        const WAKE: u64 = u64::MAX;

        /// What the watcher saw.
        enum Seen {
            Exited(u32),
            Wake,
        }

        fn cloexec(fd: RawFd) {
            // SAFETY: fcntl on a descriptor we own.
            unsafe {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }

        fn nonblocking(fd: RawFd) {
            // SAFETY: fcntl on a descriptor we own.
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }

        /// A pipe: (read end, write end). The read end is non-blocking; the
        /// write end blocks (a full pipe holds the watcher, never drops an exit).
        fn pipe_pair() -> io::Result<(OwnedFd, OwnedFd)> {
            let mut fds = [0 as libc::c_int; 2];
            // SAFETY: pipe writes two descriptors into the array.
            if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fresh descriptors, now owned.
            let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
            cloexec(r.as_raw_fd());
            cloexec(w.as_raw_fd());
            nonblocking(r.as_raw_fd());
            Ok((r, w))
        }

        /// The exits reported since the last call. `WouldBlock` when none.
        pub fn drain(reports: &OwnedFd) -> io::Result<Vec<u32>> {
            let mut buf = [0u8; 256];
            // SAFETY: the read is bounded by the buffer.
            let n = unsafe { libc::read(reports.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let e = io::Error::last_os_error();
                return Err(match e.kind() {
                    io::ErrorKind::Interrupted => io::ErrorKind::WouldBlock.into(),
                    _ => e,
                });
            }
            if n == 0 {
                // The watcher ended without a wake: the rest is polled.
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            Ok(buf[..n as usize]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect())
        }

        /// Writes one exit. False when the reader is gone.
        fn report(out: &OwnedFd, pid: u32) -> bool {
            let bytes = pid.to_le_bytes();
            // SAFETY: the write is bounded by the array.
            let n = unsafe { libc::write(out.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
            n == bytes.len() as isize
        }

        /// Starts the watcher for `pids`: the queue it waits on, the
        /// descriptors it must keep open (pidfds), and the targets.
        type Armed = (OwnedFd, Vec<OwnedFd>, Vec<u32>);

        #[cfg(target_os = "macos")]
        fn arm(pids: &[u32], wake: RawFd) -> Result<Armed, ()> {
            // SAFETY: kqueue returns a new descriptor or -1.
            let raw = unsafe { libc::kqueue() };
            if raw < 0 {
                return Err(());
            }
            // SAFETY: `raw` is a fresh descriptor, now owned.
            let queue = unsafe { OwnedFd::from_raw_fd(raw) };
            cloexec(raw);
            let kev = |ident: usize, filter: i16, flags: u16, fflags: u32| libc::kevent {
                ident,
                filter,
                flags,
                fflags,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let mut changes = vec![kev(
                wake as usize,
                libc::EVFILT_READ,
                libc::EV_ADD | libc::EV_RECEIPT,
                0,
            )];
            changes.extend(pids.iter().map(|&p| {
                kev(
                    p as usize,
                    libc::EVFILT_PROC,
                    libc::EV_ADD | libc::EV_ONESHOT | libc::EV_RECEIPT,
                    libc::NOTE_EXIT,
                )
            }));
            let mut receipts: Vec<libc::kevent> = (0..changes.len())
                .map(|_| {
                    // SAFETY: a zeroed kevent is a valid value.
                    unsafe { std::mem::zeroed() }
                })
                .collect();
            // SAFETY: both arrays are sized for the call; the timeout is null
            // because only the registration receipts are read here.
            let n = unsafe {
                libc::kevent(
                    raw,
                    changes.as_ptr(),
                    changes.len() as libc::c_int,
                    receipts.as_mut_ptr(),
                    receipts.len() as libc::c_int,
                    std::ptr::null(),
                )
            };
            if n < 0 {
                return Err(());
            }
            let mut wake_ok = false;
            let mut status = std::collections::HashMap::new();
            for r in &receipts[..n as usize] {
                let (ident, filter, flags, data) = (r.ident, r.filter, r.flags, r.data);
                if flags & libc::EV_ERROR == 0 {
                    continue;
                }
                // The filter tells the wake pipe's receipt from a pid's (a pid
                // may have the same number as the wake descriptor).
                if filter == libc::EVFILT_READ {
                    wake_ok = data == 0;
                } else {
                    // 0: registered. ESRCH: already gone. Other (EPERM): not ours.
                    status.insert(ident as u32, data == 0);
                }
            }
            if !wake_ok {
                return Err(());
            }
            let targets = pids
                .iter()
                .copied()
                .filter(|p| status.get(p).copied().unwrap_or(false))
                .collect();
            Ok((queue, Vec::new(), targets))
        }

        #[cfg(target_os = "macos")]
        fn wait(queue: &OwnedFd, wake: RawFd, seen: &mut Vec<Seen>) -> io::Result<()> {
            // SAFETY: a zeroed kevent is a valid value.
            let mut events: [libc::kevent; 64] = unsafe { std::mem::zeroed() };
            // SAFETY: the call writes at most `events.len()` entries and
            // blocks: it runs on the watcher thread, never on a tokio thread.
            let n = unsafe {
                libc::kevent(
                    queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    events.len() as libc::c_int,
                    std::ptr::null(),
                )
            };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            for e in &events[..n as usize] {
                let (filter, ident) = (e.filter, e.ident);
                if filter == libc::EVFILT_PROC {
                    seen.push(Seen::Exited(ident as u32));
                } else if filter == libc::EVFILT_READ && ident == wake as usize {
                    seen.push(Seen::Wake);
                }
            }
            Ok(())
        }

        #[cfg(target_os = "linux")]
        fn arm(pids: &[u32], wake: RawFd) -> Result<Armed, ()> {
            // SAFETY: epoll_create1 returns a new descriptor or -1.
            let raw = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
            if raw < 0 {
                return Err(());
            }
            // SAFETY: `raw` is a fresh descriptor, now owned.
            let queue = unsafe { OwnedFd::from_raw_fd(raw) };
            let mut ev = libc::epoll_event {
                events: libc::EPOLLIN as u32,
                u64: WAKE,
            };
            // SAFETY: `ev` is a valid epoll_event for the call.
            if unsafe { libc::epoll_ctl(raw, libc::EPOLL_CTL_ADD, wake, &mut ev) } < 0 {
                return Err(());
            }
            let mut pidfds = Vec::new();
            let mut targets = Vec::new();
            for &pid in pids {
                // SAFETY: pidfd_open takes a pid and flags; no pointers.
                let fd = unsafe {
                    libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0 as libc::c_uint)
                };
                if fd < 0 {
                    match nix::errno::Errno::last() {
                        // Gone already.
                        nix::errno::Errno::ESRCH => continue,
                        // No pidfd in this kernel: the caller polls.
                        nix::errno::Errno::ENOSYS => return Err(()),
                        // Not ours to watch.
                        _ => continue,
                    }
                }
                // SAFETY: a fresh descriptor from pidfd_open, now owned.
                let pidfd = unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) };
                // ONESHOT: one event per process, not one per wait.
                let mut ev = libc::epoll_event {
                    events: (libc::EPOLLIN | libc::EPOLLONESHOT) as u32,
                    u64: pid as u64,
                };
                // SAFETY: `ev` is a valid epoll_event for the call.
                if unsafe { libc::epoll_ctl(raw, libc::EPOLL_CTL_ADD, pidfd.as_raw_fd(), &mut ev) }
                    < 0
                {
                    return Err(());
                }
                pidfds.push(pidfd);
                targets.push(pid);
            }
            Ok((queue, pidfds, targets))
        }

        #[cfg(target_os = "linux")]
        fn wait(queue: &OwnedFd, _wake: RawFd, seen: &mut Vec<Seen>) -> io::Result<()> {
            // SAFETY: a zeroed epoll_event is a valid value.
            let mut events: [libc::epoll_event; 64] = unsafe { std::mem::zeroed() };
            // SAFETY: the call writes at most `events.len()` entries and
            // blocks: it runs on the watcher thread, never on a tokio thread.
            let n = unsafe {
                libc::epoll_wait(
                    queue.as_raw_fd(),
                    events.as_mut_ptr(),
                    events.len() as libc::c_int,
                    -1,
                )
            };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            for e in &events[..n as usize] {
                let data = e.u64;
                seen.push(if data == WAKE {
                    Seen::Wake
                } else {
                    Seen::Exited(data as u32)
                });
            }
            Ok(())
        }

        /// No kernel notification here: the caller polls.
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        fn arm(_pids: &[u32], _wake: RawFd) -> Result<Armed, ()> {
            Err(())
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        fn wait(_queue: &OwnedFd, _wake: RawFd, _seen: &mut Vec<Seen>) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }

        /// The watcher: waits in the kernel and reports, until the wake pipe
        /// ends. Owns everything it is given, and closes it on the way out.
        fn watch(queue: OwnedFd, _keep: Vec<OwnedFd>, wake: OwnedFd, out: OwnedFd) {
            let wake_fd = wake.as_raw_fd();
            let mut seen = Vec::new();
            loop {
                seen.clear();
                match wait(&queue, wake_fd, &mut seen) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
                for s in &seen {
                    match *s {
                        Seen::Exited(pid) => {
                            if !report(&out, pid) {
                                return;
                            }
                        }
                        Seen::Wake => return,
                    }
                }
            }
        }

        /// Starts a watcher for `pids`. `None` when no notification is
        /// available here (the caller polls).
        pub fn open(pids: &[u32]) -> Option<Opened> {
            let (wake_r, wake_w) = pipe_pair().ok()?;
            let (report_r, report_w) = pipe_pair().ok()?;
            let (queue, keep, targets) = arm(pids, wake_r.as_raw_fd()).ok()?;
            std::thread::Builder::new()
                .name("bricks-exit-watch".into())
                .spawn(move || watch(queue, keep, wake_r, report_w))
                .ok()?;
            Some((report_r, wake_w, targets))
        }
    }

    /// What a [`Watch`] waits on.
    enum Backend {
        /// The exits, as the watcher reports them. `_wake` is the write end
        /// of the wake pipe: closing it, with the watch, ends the watcher.
        Kernel {
            reports: AsyncFd<OwnedFd>,
            _wake: OwnedFd,
        },
        /// `kill(pid, 0)`, every [`POLL`].
        Poll,
    }

    /// The watched processes of one wait. Dropping it ends its watcher.
    pub(crate) struct Watch {
        backend: Backend,
        pending: BTreeSet<u32>,
        /// Times the wait was woken (for the tests).
        #[cfg(test)]
        pub(crate) wakeups: u32,
    }

    impl Watch {
        /// Watches `pids`. Returns the watch and the pids that are running
        /// now (the targets). A pid that is gone, or not ours to watch, is
        /// not a target. Pid 0 is never one (it names a group).
        pub(crate) fn register(pids: &[u32], mode: Mode) -> (Watch, Vec<u32>) {
            let pids: Vec<u32> = pids.iter().copied().filter(|&p| p > 0).collect();
            if mode == Mode::Auto
                && let Some((report, wake, targets)) = kernel::open(&pids)
                && let Ok(reports) = AsyncFd::new(report)
            {
                let pending = targets.iter().copied().collect();
                let watch = Watch {
                    backend: Backend::Kernel {
                        reports,
                        _wake: wake,
                    },
                    pending,
                    #[cfg(test)]
                    wakeups: 0,
                };
                return (watch, targets);
            }
            let targets: Vec<u32> = pids
                .into_iter()
                .filter(|&p| probe(p) == Probe::Running)
                .collect();
            let pending = targets.iter().copied().collect();
            let watch = Watch {
                backend: Backend::Poll,
                pending,
                #[cfg(test)]
                wakeups: 0,
            };
            (watch, targets)
        }

        /// Waits until every watched process has exited or `deadline`
        /// passes. Returns the pids still running (empty when all exited).
        pub(crate) async fn wait_until(&mut self, deadline: Instant) -> Vec<u32> {
            while !self.pending.is_empty() {
                let mut broken = false;
                if let Backend::Kernel { reports, .. } = &self.backend {
                    let until = tokio::time::Instant::from_std(deadline);
                    let r = tokio::time::timeout_at(
                        until,
                        reports.async_io(Interest::READABLE, kernel::drain),
                    )
                    .await;
                    match r {
                        // The deadline: the rest is still running.
                        Err(_) => break,
                        Ok(Ok(exited)) => {
                            self.note_wakeup();
                            for p in exited {
                                self.pending.remove(&p);
                            }
                        }
                        // The watcher ended without a wake: polled from now on.
                        Ok(Err(_)) => broken = true,
                    }
                } else {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    tokio::time::sleep(POLL.min(deadline - now)).await;
                    self.note_wakeup();
                    self.pending.retain(|&p| probe(p) == Probe::Running);
                }
                if broken {
                    self.backend = Backend::Poll;
                }
            }
            self.pending.iter().copied().collect()
        }

        #[cfg(test)]
        fn note_wakeup(&mut self) {
            self.wakeups += 1;
        }

        #[cfg(not(test))]
        fn note_wakeup(&mut self) {}

        /// Whether the exits come from the kernel (not the poll fallback).
        #[cfg(test)]
        pub(crate) fn is_kernel(&self) -> bool {
            matches!(self.backend, Backend::Kernel { .. })
        }

        /// The descriptor the exits are read from (for the tests).
        #[cfg(test)]
        pub(crate) fn report_fd(&self) -> Option<std::os::fd::RawFd> {
            match &self.backend {
                Backend::Kernel { reports, .. } => {
                    Some(std::os::fd::AsRawFd::as_raw_fd(reports.get_ref()))
                }
                Backend::Poll => None,
            }
        }
    }

    /// `SIGTERM`, wait up to `grace`, `SIGKILL` the rest, wait briefly, verify.
    pub async fn terminate(pids: Vec<u32>, grace: Duration) -> Termination {
        terminate_with(pids, grace, Mode::Auto).await
    }

    pub(crate) async fn terminate_with(pids: Vec<u32>, grace: Duration, mode: Mode) -> Termination {
        let (mut watch, targeted) = Watch::register(&pids, mode);
        if targeted.is_empty() {
            return Termination::default();
        }
        signal(&targeted, Signal::SIGTERM);
        let forced = watch.wait_until(Instant::now() + grace).await;
        if forced.is_empty() {
            return Termination {
                targeted,
                forced,
                survivors: Vec::new(),
            };
        }
        signal(&forced, Signal::SIGKILL);
        let survivors = watch.wait_until(Instant::now() + FORCED_WAIT).await;
        Termination {
            targeted,
            forced,
            survivors,
        }
    }

    /// Wait until none of `pids` is running, up to `limit`.
    pub async fn wait_gone(pids: &[u32], limit: Duration) -> bool {
        let (mut watch, _) = Watch::register(pids, Mode::Auto);
        watch.wait_until(Instant::now() + limit).await.is_empty()
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

    pub fn group_members(_pgid: u32) -> Vec<u32> {
        Vec::new()
    }

    pub fn group_alive(_pgid: u32) -> bool {
        false
    }

    pub fn terminate_tree_now(root: u32) {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &root.to_string()])
            .output();
    }
}

pub use imp::{
    descendants, group_alive, group_members, is_running, signal_group, terminate,
    terminate_tree_now, wait_gone,
};

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use imp::{Watch, terminate_with};
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn sh(script: &str) -> tokio::process::Child {
        tokio::process::Command::new("sh")
            .args(["-c", script])
            .spawn()
            .unwrap()
    }

    fn sleep30() -> tokio::process::Child {
        tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap()
    }

    fn send(pid: u32, sig: Signal) {
        let _ = kill(Pid::from_raw(pid as i32), sig);
    }

    /// Two sleeps that are children of a shell (grandchildren of the test),
    /// once they exist. The shell reaps them when they end.
    async fn two_sleeps() -> (tokio::process::Child, Vec<u32>) {
        let child = sh("sleep 30 & sleep 30 & wait");
        let root = child.id().unwrap();
        let mut kids = Vec::new();
        for _ in 0..200 {
            kids = descendants(root);
            if kids.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        (child, kids)
    }

    #[tokio::test]
    async fn a_tree_is_found_and_terminated() {
        let (mut child, kids) = two_sleeps().await;
        assert_eq!(kids.len(), 2, "{kids:?}");
        let t = terminate(kids.clone(), Duration::from_secs(2)).await;
        assert_eq!(t.targeted.len(), 2);
        assert!(t.survivors.is_empty());
        assert!(wait_gone(&kids, Duration::from_secs(2)).await);
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn sigterm_resistant_processes_are_forced() {
        let mut child = sh("trap '' TERM; while :; do sleep 0.05; done");
        let pid = child.id().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let t = terminate(vec![pid], Duration::from_millis(200)).await;
        assert_eq!(t.forced, vec![pid]);
        assert!(t.survivors.is_empty(), "{:?}", t.survivors);
        let _ = child.wait().await;
        assert!(!is_running(pid));
    }

    #[tokio::test]
    async fn a_process_that_exits_on_term_is_stopped_without_periodic_checks() {
        let mut child = sleep30();
        let pid = child.id().unwrap();
        let (mut watch, targets) = Watch::register(&[pid], Mode::Auto);
        assert_eq!(targets, vec![pid]);
        assert!(
            watch.is_kernel(),
            "the exit comes from the kernel, not a poll"
        );
        send(pid, Signal::SIGTERM);
        let started = Instant::now();
        let left = watch
            .wait_until(Instant::now() + Duration::from_secs(10))
            .await;
        assert!(left.is_empty(), "{left:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(watch.wakeups <= 2, "woken {} times", watch.wakeups);
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn an_already_finished_process_is_not_waited_for() {
        let mut child = tokio::process::Command::new("true").spawn().unwrap();
        let pid = child.id().unwrap();
        let _ = child.wait().await;
        let started = Instant::now();
        let t = terminate(vec![pid], Duration::from_secs(10)).await;
        assert_eq!(t, Termination::default());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(wait_gone(&[pid], Duration::from_millis(10)).await);
    }

    #[tokio::test]
    async fn a_zombie_is_stopped_not_waited_for() {
        // Exited, and not yet reaped by its parent (the test).
        let mut child = sh("exit 0");
        let pid = child.id().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let started = Instant::now();
        let t = terminate(vec![pid], Duration::from_secs(10)).await;
        assert!(t.survivors.is_empty(), "{t:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn a_wait_returns_at_its_deadline_with_the_survivors() {
        let mut child = sleep30();
        let pid = child.id().unwrap();
        let (mut watch, _) = Watch::register(&[pid], Mode::Auto);
        let started = Instant::now();
        let left = watch
            .wait_until(Instant::now() + Duration::from_millis(150))
            .await;
        let took = started.elapsed();
        assert_eq!(left, vec![pid]);
        assert!(took >= Duration::from_millis(140), "{took:?}");
        assert!(took < Duration::from_secs(3), "{took:?}");
        send(pid, Signal::SIGKILL);
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn a_cancelled_wait_closes_its_descriptor() {
        let mut child = sleep30();
        let pid = child.id().unwrap();
        let (mut watch, _) = Watch::register(&[pid], Mode::Auto);
        let fd = watch.report_fd().expect("a kernel watch");
        // Cancelled while it waits: the future is dropped at the timeout.
        let cancelled = tokio::time::timeout(
            Duration::from_millis(100),
            watch.wait_until(Instant::now() + Duration::from_secs(10)),
        )
        .await;
        assert!(cancelled.is_err());
        drop(watch);
        // SAFETY: fcntl only queries the descriptor number.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_eq!(flags, -1, "the report descriptor is closed");
        send(pid, Signal::SIGKILL);
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn another_task_keeps_running_while_a_stop_waits() {
        let mut child = sh("trap '' TERM; while :; do sleep 0.05; done");
        let pid = child.id().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticker = {
            let ticks = ticks.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    ticks.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        let stopped = terminate(vec![pid], Duration::from_millis(300)).await;
        assert_eq!(stopped.forced, vec![pid]);
        ticker.abort();
        assert!(
            ticks.load(Ordering::SeqCst) >= 5,
            "{}",
            ticks.load(Ordering::SeqCst)
        );
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn the_poll_fallback_stops_a_tree_too() {
        let (mut child, kids) = two_sleeps().await;
        assert_eq!(kids.len(), 2, "{kids:?}");
        let t = terminate_with(kids.clone(), Duration::from_secs(2), Mode::Poll).await;
        assert_eq!(t.targeted.len(), 2);
        assert!(t.survivors.is_empty());
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn an_unknown_pid_is_not_a_target() {
        assert_eq!(
            terminate(vec![0], Duration::from_secs(1)).await,
            Termination::default()
        );
        assert!(wait_gone(&[0], Duration::from_millis(10)).await);
    }
}

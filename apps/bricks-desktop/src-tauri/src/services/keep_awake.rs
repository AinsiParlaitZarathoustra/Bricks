//! Keeps the Mac from sleeping while Bricks asks for it: `caffeinate -i`
//! (idle system sleep only; the display may still turn off), tied to the
//! app's pid so it ends with the app, even after a crash. Elsewhere nothing
//! is held and the request is refused.

use parking_lot::Mutex;
use std::process::{Child, Command, Stdio};

#[derive(Default)]
pub struct KeepAwake {
    holder: Mutex<Option<Child>>,
}

impl KeepAwake {
    /// Holds (`true`) or releases (`false`) the assertion; repeating a state
    /// changes nothing. A holder that exited on its own is started again.
    pub fn set(&self, active: bool) -> std::io::Result<()> {
        let mut holder = self.holder.lock();
        if let Some(child) = holder.as_mut()
            && child.try_wait()?.is_some()
        {
            *holder = None;
        }
        match (active, holder.as_mut()) {
            (true, None) => *holder = Some(spawn()?),
            (false, Some(child)) => {
                let _ = child.kill();
                let _ = child.wait();
                *holder = None;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn active(&self) -> bool {
        self.holder.lock().is_some()
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        let _ = self.set(false);
    }
}

#[cfg(target_os = "macos")]
fn spawn() -> std::io::Result<Child> {
    Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(not(target_os = "macos"))]
fn spawn() -> std::io::Result<Child> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "seulement sur macOS",
    ))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn holds_once_and_releases() {
        let keep = KeepAwake::default();
        keep.set(true).unwrap();
        let pid = keep.holder.lock().as_ref().map(Child::id);
        keep.set(true).unwrap();
        assert_eq!(keep.holder.lock().as_ref().map(Child::id), pid, "held once");
        keep.set(false).unwrap();
        assert!(!keep.active());
        keep.set(false).unwrap();
        assert!(!keep.active());
    }
}

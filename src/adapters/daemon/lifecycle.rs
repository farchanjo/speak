//! Single-instance lifecycle: PID file + process control (ADR-0010).
//!
//! `speak daemon` is a single-instance service. On start it reads the configured
//! pidfile, and if it names a LIVE process that is actually a speak daemon
//! (verified by a liveness check AND a ping on the existing socket), it replaces
//! that instance — SIGTERM, wait up to `kill_grace_ms`, then SIGKILL — and clears
//! the leftover socket + pidfile. A dead/stale pidfile is simply cleaned. Once the
//! socket is bound, the daemon writes its own PID atomically (temp file + rename)
//! and removes it on graceful shutdown, so a clean exit never leaves a stale lock.
//!
//! Process signalling rides the `nix` safe wrappers (no `unsafe`) and is gated to
//! `cfg(unix)`; the pidfile read/write is portable.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Internal poll granularity (ms) while waiting for a signalled daemon to exit.
/// NOT a user knob — `kill_grace_ms` bounds the TOTAL wait; this only sets how
/// often liveness is re-checked inside that window.
const POLL_STEP_MS: u64 = 50;

/// Read the PID recorded in `path`, if it holds a single positive integer.
#[must_use]
pub(super) fn read_pid(path: &Path) -> Option<i32> {
    let text = std::fs::read_to_string(path).ok()?;
    text.trim().parse::<i32>().ok().filter(|&pid| pid > 0)
}

/// Atomically write `pid` to `path` (temp sibling + rename) so a reader never
/// observes a half-written pidfile.
pub(super) fn write_pid_atomic(path: &Path, pid: u32) -> Result<()> {
    let tmp = temp_sibling(path, pid);
    std::fs::write(&tmp, format!("{pid}\n"))
        .with_context(|| format!("writing pidfile temp {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming pidfile into place {}", path.display()))?;
    Ok(())
}

/// Remove the pidfile, ignoring a missing file.
pub(super) fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// The temp sibling used for the atomic pidfile write (`<name>.<pid>.tmp`).
fn temp_sibling(path: &Path, pid: u32) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| std::ffi::OsString::from("speak.pid"), ToOwned::to_owned);
    name.push(format!(".{pid}.tmp"));
    path.with_file_name(name)
}

/// Whether `pid` names a live process (EPERM counts as alive — it exists but is
/// owned by another user).
#[cfg(unix)]
#[must_use]
pub(super) fn is_alive(pid: i32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    matches!(
        kill(Pid::from_raw(pid), None::<Signal>),
        Ok(()) | Err(Errno::EPERM)
    )
}

#[cfg(not(unix))]
#[must_use]
pub fn is_alive(_pid: i32) -> bool {
    false
}

/// Send SIGTERM to `pid` (graceful shutdown request).
#[cfg(unix)]
pub(super) fn terminate(pid: i32) -> Result<()> {
    signal(pid, nix::sys::signal::Signal::SIGTERM)
}

/// Send SIGKILL to `pid` (forced kill after the grace window).
#[cfg(unix)]
pub(super) fn kill_hard(pid: i32) -> Result<()> {
    signal(pid, nix::sys::signal::Signal::SIGKILL)
}

#[cfg(unix)]
fn signal(pid: i32, sig: nix::sys::signal::Signal) -> Result<()> {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), sig)
        .with_context(|| format!("sending {sig:?} to pid {pid}"))
}

#[cfg(not(unix))]
pub fn terminate(_pid: i32) -> Result<()> {
    anyhow::bail!("process signalling is unix-only")
}

#[cfg(not(unix))]
pub fn kill_hard(_pid: i32) -> Result<()> {
    anyhow::bail!("process signalling is unix-only")
}

/// SIGTERM `pid`, poll up to `grace` for it to exit, then SIGKILL if it lingers.
pub(super) async fn terminate_and_wait(pid: i32, grace: Duration) -> Result<()> {
    if !is_alive(pid) {
        return Ok(());
    }
    terminate(pid)?;
    if wait_until(grace, || !is_alive(pid)).await {
        tracing::debug!(pid, "previous daemon exited on SIGTERM");
        return Ok(());
    }
    if is_alive(pid) {
        tracing::warn!(
            pid,
            "previous daemon ignored SIGTERM within grace; sending SIGKILL"
        );
        kill_hard(pid)?;
    }
    Ok(())
}

/// Start-time single-instance reconciliation (ADR-0010): kill/clean any previous
/// instance so this `speak daemon` becomes the sole owner of the socket.
pub(super) async fn replace_previous(socket: &Path, pidfile: &Path, grace: Duration) -> Result<()> {
    match read_pid(pidfile) {
        Some(pid) if is_alive(pid) && super::is_running(socket).await => {
            tracing::info!(pid, "replacing the previous speak daemon");
            terminate_and_wait(pid, grace).await?;
        }
        Some(pid) if is_alive(pid) => {
            // Alive but silent on the socket: most likely PID reuse, not our
            // daemon — never signal an unrelated process; treat as a stale lock.
            tracing::warn!(
                pid,
                "pidfile names a live non-daemon process; leaving it untouched"
            );
        }
        Some(pid) => tracing::debug!(pid, "stale pidfile (process gone); cleaning up"),
        None if super::is_running(socket).await => {
            tracing::info!("stopping an orphan daemon (no pidfile) over its socket");
            let _ = super::stop_over_socket(socket).await;
            wait_for_socket_gone(socket, grace).await;
        }
        None => {}
    }
    remove(pidfile);
    let _ = std::fs::remove_file(socket);
    Ok(())
}

/// Poll `done` every [`POLL_STEP_MS`] until it returns true or `grace` elapses;
/// returns whether `done` became true within the window.
async fn wait_until(grace: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let step = Duration::from_millis(POLL_STEP_MS);
    while start.elapsed() < grace {
        if done() {
            return true;
        }
        tokio::time::sleep(step).await;
    }
    done()
}

/// Poll the socket until no daemon answers or `grace` elapses (orphan shutdown).
async fn wait_for_socket_gone(socket: &Path, grace: Duration) {
    let start = Instant::now();
    let step = Duration::from_millis(POLL_STEP_MS);
    while start.elapsed() < grace {
        if !super::is_running(socket).await {
            return;
        }
        tokio::time::sleep(step).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("speak-pidfile-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("speak.pid")
    }

    #[test]
    fn write_then_read_round_trips_the_pid() {
        let path = scratch("round-trip");
        write_pid_atomic(&path, 4242).unwrap();
        assert_eq!(read_pid(&path), Some(4242));
        remove(&path);
        assert_eq!(read_pid(&path), None);
    }

    #[test]
    fn read_pid_rejects_missing_and_garbage() {
        let path = scratch("garbage");
        remove(&path);
        assert_eq!(read_pid(&path), None, "missing file");
        std::fs::write(&path, "not-a-pid\n").unwrap();
        assert_eq!(read_pid(&path), None, "non-numeric");
        std::fs::write(&path, "0\n").unwrap();
        assert_eq!(read_pid(&path), None, "non-positive");
        remove(&path);
    }

    #[test]
    fn is_alive_recognises_self_and_rejects_a_phantom_pid() {
        let me = i32::try_from(std::process::id()).unwrap();
        assert!(is_alive(me), "the test process is alive");
        // i32::MAX is far above any real PID on supported platforms.
        assert!(!is_alive(i32::MAX), "a phantom pid is not alive");
    }

    #[tokio::test]
    async fn terminate_and_wait_is_a_noop_for_a_dead_pid() {
        // A phantom pid is already gone, so there is nothing to signal.
        assert!(
            terminate_and_wait(i32::MAX, Duration::from_millis(10))
                .await
                .is_ok()
        );
    }
}

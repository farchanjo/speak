//! macOS TCC responsibility disclaim (ADR-0016).
//!
//! Host-output capture needs the audio-capture (`kTCCServiceAudioCapture`) grant,
//! and TCC attributes a CLI's request to the *responsible process* — normally the
//! launching terminal. So a signed `speak` run directly from a shell is muted
//! unless that terminal is granted. To make `speak` its **own** TCC subject (so
//! the grant on its code-signing identity applies regardless of who launched it),
//! it re-execs itself once via `posix_spawn` with the private
//! `responsibility_spawnattrs_setdisclaim` attribute (the pattern terminal
//! emulators use), which disclaims the parent's responsibility. The parent then
//! supervises (waits + forwards the exit code); the disclaimed child does the
//! real work as its own responsible process.
//!
//! Only meaningful for a code-signed binary (the bundle from `make app`): an
//! ad-hoc binary disclaims to an identity with no grant and still falls back to
//! the silence warning.

use std::ffi::CString;
use std::os::raw::c_int;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::ptr;

use anyhow::{Context, Result, bail};

// Private libSystem SPI (no public header): mark the spawned child to disclaim
// the parent's TCC responsibility. Stable symbol used by terminal emulators.
unsafe extern "C" {
    fn responsibility_spawnattrs_setdisclaim(
        attr: *mut libc::posix_spawnattr_t,
        disclaim: c_int,
    ) -> c_int;
}

/// Sentinel env var marking the already-disclaimed child (prevents a re-exec loop).
const DISCLAIMED: &str = "SPEAK_TCC_DISCLAIMED";

/// Re-exec this process as its own TCC-responsible subject, supervise it, and
/// exit with its status. Returns `Ok(())` (a no-op) when already disclaimed — the
/// child continues to the real work.
pub fn reexec_disclaimed() -> Result<()> {
    if std::env::var_os(DISCLAIMED).is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe().context("current_exe for TCC disclaim")?; // current_exe self-spawn
    let exe_c = CString::new(exe.as_os_str().as_bytes()).context("exe path has interior NUL")?;

    // argv = [exe, original args…, NULL].
    let arg_cstrs = args_as_cstrings()?;
    let mut argv: Vec<*mut libc::c_char> = Vec::with_capacity(arg_cstrs.len() + 2);
    argv.push(exe_c.as_ptr().cast_mut());
    argv.extend(arg_cstrs.iter().map(|c| c.as_ptr().cast_mut()));
    argv.push(ptr::null_mut());

    // envp = current environment + the disclaim sentinel, NULL-terminated.
    let env_cstrs = env_as_cstrings()?;
    let mut envp: Vec<*mut libc::c_char> =
        env_cstrs.iter().map(|c| c.as_ptr().cast_mut()).collect();
    envp.push(ptr::null_mut());

    // SAFETY: standard `posix_spawn` FFI; argv/envp/exe outlive the call.
    let pid = unsafe { spawn_disclaimed(&exe_c, &argv, &envp)? };

    // Supervisor: ignore the terminal signals (the child reset them to default via
    // SETSIGDEF, so Ctrl-C still stops it) and forward the child's exit code.
    // SAFETY: signal disposition + waitpid on our own child.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        let mut status: c_int = 0;
        libc::waitpid(pid, &raw mut status, 0);
        let code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else {
            1
        };
        std::process::exit(code);
    }
}

/// The current process arguments (excluding argv[0]) as C strings.
fn args_as_cstrings() -> Result<Vec<CString>> {
    std::env::args_os()
        .skip(1)
        .map(|a| CString::new(a.into_vec()).context("argument has interior NUL"))
        .collect()
}

/// The current environment as `KEY=VALUE` C strings, plus the disclaim sentinel.
fn env_as_cstrings() -> Result<Vec<CString>> {
    let mut out: Vec<CString> = std::env::vars_os()
        .map(|(k, v)| {
            let mut bytes = k.into_vec();
            bytes.push(b'=');
            bytes.extend(v.into_vec());
            CString::new(bytes).context("environment entry has interior NUL")
        })
        .collect::<Result<_>>()?;
    out.push(CString::new(format!("{DISCLAIMED}=1")).expect("sentinel is NUL-free"));
    Ok(out)
}

/// `posix_spawn` the child with the disclaim attribute + default terminal signals.
///
/// # Safety
/// `exe`/`argv`/`envp` must be valid and NULL-terminated for the call.
unsafe fn spawn_disclaimed(
    exe: &CString,
    argv: &[*mut libc::c_char],
    envp: &[*mut libc::c_char],
) -> Result<libc::pid_t> {
    unsafe {
        let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
        if libc::posix_spawnattr_init(&raw mut attr) != 0 {
            bail!("posix_spawnattr_init failed");
        }
        // The child resets the terminal signals to their default so Ctrl-C stops
        // it even though the supervisor parent ignores them.
        let mut sigs: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut sigs);
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
            libc::sigaddset(&raw mut sigs, sig);
        }
        libc::posix_spawnattr_setsigdefault(&raw mut attr, &raw const sigs);
        libc::posix_spawnattr_setflags(&raw mut attr, libc::POSIX_SPAWN_SETSIGDEF as i16);

        if responsibility_spawnattrs_setdisclaim(&raw mut attr, 1) != 0 {
            let _ = libc::posix_spawnattr_destroy(&raw mut attr);
            bail!("responsibility_spawnattrs_setdisclaim failed");
        }

        let mut pid: libc::pid_t = 0;
        let rc = libc::posix_spawn(
            &raw mut pid,
            exe.as_ptr(),
            ptr::null(),
            &raw const attr,
            argv.as_ptr(),
            envp.as_ptr(),
        );
        let _ = libc::posix_spawnattr_destroy(&raw mut attr);
        if rc != 0 {
            bail!("posix_spawn (TCC disclaim) failed: errno {rc}");
        }
        Ok(pid)
    }
}

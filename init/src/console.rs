//! The terminal init and its children talk through.
//!
//! The kernel starts init on `/dev/console`, which is only an alias: a shell
//! on it gets no job control (no Ctrl-C), because it can't be a process's
//! controlling terminal. The real device behind it — `ttyS0` for the serial
//! line, `tty1` for the screen — is named in `/sys/class/tty/console/active`.

use std::ffi::CString;

/// Point init's stdin/stdout/stderr at the console, in case the kernel
/// started us without one (no `/dev/console` in the initramfs).
pub fn attach() {
    if let Some(fd) = open("/dev/console") {
        redirect(fd);
    }
}

/// The real terminal behind `/dev/console`, e.g. `/dev/ttyS0`.
pub fn device() -> String {
    let active = std::fs::read_to_string("/sys/class/tty/console/active").unwrap_or_default();

    // The last one listed is the one `/dev/console` writes to.
    match active.split_whitespace().last() {
        Some(name) => format!("/dev/{name}"),
        None => "/dev/console".into(),
    }
}

/// Every console the kernel writes to, e.g. `/dev/ttyS0` and `/dev/tty1`
/// for `console=ttyS0 console=tty1`. Empty if `/sys` can't say.
pub fn devices() -> Vec<String> {
    std::fs::read_to_string("/sys/class/tty/console/active")
        .unwrap_or_default()
        .split_whitespace()
        .map(|name| format!("/dev/{name}"))
        .collect()
}

/// Write `text` to every console, so what init says is on the screen and
/// on the serial line alike. Before `/sys` can name them, to stderr.
pub fn broadcast(text: &str) {
    use std::io::Write;

    let consoles = devices();
    if consoles.is_empty() {
        eprint!("{text}");
        return;
    }
    for path in consoles {
        if let Ok(mut console) = std::fs::OpenOptions::new().write(true).open(&path) {
            let _ = console.write_all(text.as_bytes());
        }
    }
}

/// Run in a forked child before `exec`: start a new session and make the
/// console its controlling terminal, so Ctrl-C reaches the program.
///
/// Only async-signal-safe calls in here — it runs between fork and exec.
pub fn take_over(path: &CString) -> std::io::Result<()> {
    // SAFETY: plain syscalls on file descriptors this child owns.
    unsafe {
        if libc::setsid() < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let fd = libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Failing to become the controlling terminal is not fatal — the
        // program still runs, just without Ctrl-C.
        libc::ioctl(fd, libc::TIOCSCTTY, 0);

        for target in 0..3 {
            libc::dup2(fd, target);
        }

        if fd > 2 {
            libc::close(fd);
        }
    }

    Ok(())
}

fn open(path: &str) -> Option<libc::c_int> {
    let path = CString::new(path).ok()?;

    // SAFETY: `path` is NUL-terminated and outlives the call.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };

    (fd >= 0).then_some(fd)
}

fn redirect(fd: libc::c_int) {
    // SAFETY: `fd` is open; dup2 onto 0..2 replaces whatever was there.
    unsafe {
        for target in 0..3 {
            libc::dup2(fd, target);
        }

        if fd > 2 {
            libc::close(fd);
        }
    }
}

/// Put the screen's virtual terminal back to text with a working keyboard.
///
/// A session that draws on the bare screen (Shine's console backend)
/// switches the terminal to graphics mode and turns its keyboard off, and
/// switches both back when it closes. One that crashes doesn't, which would
/// leave whatever runs next — the restarted session, the debug shell — on a
/// blank screen that ignores the keyboard. So init resets it whenever a
/// session ends. Harmless when there is no virtual terminal.
pub fn reset_vt() {
    const KDSETMODE: libc::Ioctl = 0x4B3A;
    const KDSKBMODE: libc::Ioctl = 0x4B45;
    const KD_TEXT: libc::c_ulong = 0;
    const K_UNICODE: libc::c_ulong = 3;

    if let Some(fd) = open("/dev/tty0") {
        // SAFETY: both requests take the mode by value; `fd` is ours.
        unsafe {
            libc::ioctl(fd, KDSETMODE, KD_TEXT);
            libc::ioctl(fd, KDSKBMODE, K_UNICODE);
            libc::close(fd);
        }
    }
}

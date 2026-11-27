//! Power off and reboot, and the signals that ask for them.
//!
//! The conventions BusyBox's `poweroff`/`reboot` follow, so those work too:
//!
//!   SIGUSR2  → power off
//!   SIGTERM  → reboot
//!   SIGINT   → reboot (the kernel sends it for Ctrl-Alt-Del)

use std::sync::atomic::{AtomicI32, Ordering};

const NONE: i32 = 0;

/// The last power signal received, or `NONE`. A handler may only touch
/// atomics, so it records the signal and `waitpid` returning `EINTR` lets
/// the main loop look at it.
static REQUESTED: AtomicI32 = AtomicI32::new(NONE);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    PowerOff,
    Reboot,
}

pub fn install_signal_handlers() {
    // Ctrl-Alt-Del sends SIGINT to init instead of resetting the machine.
    // SAFETY: a plain syscall with a constant argument.
    unsafe {
        libc::reboot(libc::LINUX_REBOOT_CMD_CAD_OFF);
    }

    for signal in [libc::SIGUSR2, libc::SIGTERM, libc::SIGINT] {
        // SAFETY: `on_signal` only stores to an atomic. No SA_RESTART, so a
        // blocking `waitpid` is interrupted and the request seen at once.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

extern "C" fn on_signal(signal: libc::c_int) {
    REQUESTED.store(signal, Ordering::SeqCst);
}

/// A power request that came in by signal, if any.
pub fn requested() -> Option<Request> {
    match REQUESTED.load(Ordering::SeqCst) {
        libc::SIGUSR2 => Some(Request::PowerOff),
        libc::SIGTERM | libc::SIGINT => Some(Request::Reboot),
        _ => None,
    }
}

/// Stop every process, flush the disks, and turn the machine off or restart
/// it. Never returns.
pub fn shutdown(request: Request) -> ! {
    crate::log::info(match request {
        Request::PowerOff => "powering off",
        Request::Reboot => "rebooting",
    });

    // SAFETY: plain syscalls; -1 means every process but init itself.
    unsafe {
        libc::kill(-1, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_millis(500));

    // SAFETY: as above.
    unsafe {
        libc::kill(-1, libc::SIGKILL);
    }
    // Every process gone, the data disk can be let go of cleanly.
    crate::storage::unmount();
    crate::disk::detach();

    // SAFETY: as above.
    unsafe {
        libc::sync();

        libc::reboot(match request {
            Request::PowerOff => libc::LINUX_REBOOT_CMD_POWER_OFF,
            Request::Reboot => libc::LINUX_REBOOT_CMD_RESTART,
        });
    }

    // reboot() only returns on failure. PID 1 must not exit, so wait forever.
    crate::log::warn(&format!(
        "{request:?} failed: {}",
        std::io::Error::last_os_error()
    ));
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

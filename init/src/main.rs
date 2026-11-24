//! saule-init — PID 1 of Saule Linux.
//!
//! The kernel runs `/init` from the initramfs as the first process. Its jobs:
//!
//! 1. mount the kernel's file systems (`/proc`, `/sys`, `/dev`, …);
//! 2. give the machine its name and print the banner, and mount the data
//!    disk at `/data`, where what the session saves is kept across a
//!    reboot (`storage.rs`);
//! 3. start the **session** — `/bin/session`, a Saule program built with
//!    `saule build` — as the unprivileged `display` account, with the screen
//!    and input devices handed to it (`seat.rs`), and keep it going;
//! 4. reap every orphaned process (PID 1 inherits them all);
//! 5. power off or reboot cleanly when asked.
//!
//! PID 1 must never exit: the kernel panics when it does. Every error here is
//! logged and survived.
//!
//! Kernel command line options (see `cmdline.rs`):
//!
//!   saule.session=PATH      the executable to run as the session
//!   saule.user=NAME         the account it runs as (default: display;
//!                           `root` only when asked for)
//!   saule.on_exit=shell     what to do when the session returns:
//!                           poweroff | reboot | restart | shell
//!                           (default: shell when /bin/sh exists, else poweroff)

mod account;
mod cmdline;
mod console;
mod log;
mod mounts;
mod output;
mod power;
mod seat;
mod session;
mod storage;

use cmdline::Cmdline;

fn main() {
    if std::process::id() != 1 {
        eprintln!("saule-init: this is the system's first process and must run as PID 1");
        std::process::exit(1);
    }

    mounts::mount_system();
    console::attach();
    power::install_signal_handlers();
    seat::start();

    let cmdline = Cmdline::read();
    set_hostname();
    log::banner();
    storage::mount_data();

    session::supervise(&cmdline);
}

/// Name the machine from `/etc/hostname`; the kernel default stays otherwise.
fn set_hostname() {
    let Ok(name) = std::fs::read_to_string("/etc/hostname") else {
        return;
    };

    let name = name.trim();
    if name.is_empty() {
        return;
    }

    // SAFETY: the pointer and length describe `name`, which outlives the call.
    let result = unsafe { libc::sethostname(name.as_ptr().cast(), name.len()) };
    if result != 0 {
        log::warn(&format!(
            "could not set the hostname: {}",
            std::io::Error::last_os_error()
        ));
    }
}

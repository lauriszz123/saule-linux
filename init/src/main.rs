//! saule-init — PID 1 of Saule Linux.
//!
//! The kernel runs `/init` from the initramfs as the first process. Its jobs:
//!
//! 1. mount the kernel's file systems (`/proc`, `/sys`, `/dev`, …);
//! 2. give the machine its name and print the banner, and mount the data
//!    disk at `/data`, where what the session saves is kept across a
//!    reboot (`storage.rs`), and with it the name and time zone the person
//!    chose for the machine (`system.rs`);
//! 3. start the **session** — `/bin/session`, a Saule program built with
//!    `saule build` — as the unprivileged `display` account, with the screen
//!    and input devices handed to it (`seat.rs`), and keep it going;
//!    and beside it the programs that do what it may ask for but not do
//!    itself (`services.rs`), and the apps it asks for, which run as the
//!    person's account and not its own (`launcher.rs`, `person.rs`);
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
//!   saule.installer=1       started from the stick: the session is the
//!                           installer, as root, and no disk is mounted

mod account;
mod cmdline;
mod console;
mod installer;
mod launcher;
mod log;
mod mounts;
mod output;
mod person;
mod power;
mod seat;
mod services;
mod session;
mod storage;
mod system;
mod zone;

use cmdline::Cmdline;

fn main() {
    // A second copy of init, asked what a time zone is: see `zone.rs`.
    let args: Vec<String> = std::env::args().collect();
    if let [_, mode, name, at] = args.as_slice() {
        if mode == "zone" {
            zone::answer(name, at);
        }
    }

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
    // From the stick, the disks are the installer's to decide about: a
    // blank one is not made a data disk behind its back.
    if cmdline.installer {
        installer::prepare();
    } else {
        storage::mount_data();
        system::start();
    }
    // The person's account is called after whoever the machine is for,
    // before anything looks it up by name; and a home it once shared with
    // the session's account is made its own.
    let person = if cmdline.installer {
        person::ACCOUNT.to_string()
    } else {
        person::adopt(person::ACCOUNT)
    };
    if let (Some(person), Some(session)) = (
        account::Account::find(&person),
        account::Account::find(cmdline::USER),
    ) {
        person::settle(&person, &session);
    }

    // From the stick the session is the installer's, as root; what is
    // tried from there afterwards is the ordinary account's.
    let account = if cmdline.installer {
        cmdline::USER
    } else {
        cmdline.user.as_str()
    };
    services::start(account, &person);
    launcher::start(&person, &cmdline.env);

    session::supervise(&cmdline);
}

/// Name the machine until somebody names it: after what it is, as its
/// maker's firmware says (an "Inspiron 7559" is `Inspiron-7559`), so that
/// the first-start setup offers the computer's own name to keep or change.
/// A machine whose firmware says nothing of use is called what
/// `/etc/hostname` says; without that the kernel's default stays.
fn set_hostname() {
    let name = system::model_name().or_else(|| {
        let name = std::fs::read_to_string("/etc/hostname").ok()?;
        let name = name.trim();
        (!name.is_empty()).then(|| name.to_string())
    });
    let Some(name) = name else {
        return;
    };

    // SAFETY: the pointer and length describe `name`, which outlives the call.
    let result = unsafe { libc::sethostname(name.as_ptr().cast(), name.len()) };
    if result != 0 {
        log::warn(&format!(
            "could not set the hostname: {}",
            std::io::Error::last_os_error()
        ));
    }
}

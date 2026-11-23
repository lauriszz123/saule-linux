//! Starting the session and keeping it alive.
//!
//! The session is the one program the machine exists to run: a Saule
//! project built into one executable with `saule build` — today a
//! hello-world, later the display server. Init starts it, waits, and decides
//! what happens when it ends:
//!
//! - it **returned** (exit code 0)  → follow `saule.on_exit`;
//! - it **crashed** (non-zero/signal) → start it again after a second; if it
//!   keeps crashing straight away, stop and fall back to a shell or power off.

use std::ffi::CString;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::cmdline::{Cmdline, OnExit};
use crate::power::{self, Request};
use crate::{console, log};

const SHELL: &str = "/bin/sh";

/// A crash this soon after starting counts towards giving up.
const QUICK_CRASH: Duration = Duration::from_secs(5);
const MAX_QUICK_CRASHES: u32 = 5;

enum Outcome {
    Exited(i32),
    Killed(i32),
    Power(Request),
    NotStarted(std::io::Error),
}

pub fn has_shell() -> bool {
    Path::new(SHELL).exists()
}

pub fn supervise(cmdline: &Cmdline) -> ! {
    let mut quick_crashes = 0;

    loop {
        log::info(&format!("starting session: {}", cmdline.session));

        let started = Instant::now();
        let outcome = run(&cmdline.session, &[], "/root");
        console::reset_vt();

        let failure = match outcome {
            Outcome::Power(request) => power::shutdown(request),
            Outcome::Exited(0) => {
                log::info("session ended");
                quick_crashes = 0;
                after_exit(cmdline.on_exit);
                continue;
            }
            Outcome::Exited(code) => format!("session exited with code {code}"),
            Outcome::Killed(signal) => format!("session killed by signal {signal}"),
            Outcome::NotStarted(error) => {
                format!("could not start {}: {error}", cmdline.session)
            }
        };

        log::warn(&failure);

        if started.elapsed() < QUICK_CRASH {
            quick_crashes += 1;
        } else {
            quick_crashes = 0;
        }

        if quick_crashes >= MAX_QUICK_CRASHES {
            log::warn("the session keeps failing; giving up on it");
            quick_crashes = 0;

            if has_shell() {
                shell();
            } else {
                power::shutdown(Request::PowerOff);
            }
        } else {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn after_exit(on_exit: OnExit) {
    match on_exit {
        OnExit::PowerOff => power::shutdown(Request::PowerOff),
        OnExit::Reboot => power::shutdown(Request::Reboot),
        OnExit::Restart => {}
        OnExit::Shell if has_shell() => shell(),
        OnExit::Shell => power::shutdown(Request::PowerOff),
    }
}

/// A debugging shell (debug images only). Leaving it restarts the session.
fn shell() {
    log::info("shell — `exit` restarts the session, `poweroff` turns the machine off");

    match run(SHELL, &["-l"], "/root") {
        Outcome::Power(request) => power::shutdown(request),
        Outcome::NotStarted(error) => log::warn(&format!("could not start {SHELL}: {error}")),
        _ => {}
    }
}

/// Start `program` on the console and wait for it, reaping any other process
/// that dies meanwhile. Returns early if a power signal arrives.
fn run(program: &str, args: &[&str], dir: &str) -> Outcome {
    let tty = match CString::new(console::device()) {
        Ok(tty) => tty,
        Err(error) => return Outcome::NotStarted(error.into()),
    };

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(if Path::new(dir).is_dir() { dir } else { "/" })
        .env_clear()
        .env("PATH", "/bin:/usr/bin")
        .env("HOME", "/root")
        .env("TERM", "linux")
        .env("LANG", "C.UTF-8");

    // SAFETY: `take_over` only makes async-signal-safe syscalls.
    unsafe {
        command.pre_exec(move || console::take_over(&tty));
    }

    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Outcome::NotStarted(error),
    };

    let pid = child.id() as libc::pid_t;

    // Not `child.wait()`: init must also reap orphans, so it waits for any
    // process and only stops when the one it started is among them.
    loop {
        let mut status = 0;

        // SAFETY: `status` is a valid out-pointer.
        let reaped = unsafe { libc::waitpid(-1, &mut status, 0) };

        if reaped < 0 {
            let error = std::io::Error::last_os_error();

            if error.raw_os_error() == Some(libc::EINTR) {
                if let Some(request) = power::requested() {
                    return Outcome::Power(request);
                }

                continue;
            }

            // ECHILD: nothing left to wait for, so the child is gone too.
            return Outcome::Exited(0);
        }

        if reaped != pid {
            continue;
        }

        if libc::WIFEXITED(status) {
            return Outcome::Exited(libc::WEXITSTATUS(status));
        }

        if libc::WIFSIGNALED(status) {
            return Outcome::Killed(libc::WTERMSIG(status));
        }
    }
}

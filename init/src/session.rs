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

use crate::account::{self, Account, Switch};
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

/// What a session asks for by how it exits: the window manager's Shut
/// Down, Restart and Log Out. It runs as `display`, which may not signal
/// init or reboot the machine; the one thing it can always do is end, and
/// how it ends is init's to act on. Only the session init started can ask
/// this way, so no other program can turn the machine off.
///
/// Exit code 0 is a session that simply ended: `saule.on_exit` decides.
pub const EXIT_POWER_OFF: i32 = 80;
pub const EXIT_REBOOT: i32 = 81;
pub const EXIT_LOG_OUT: i32 = 82;

pub fn has_shell() -> bool {
    Path::new(SHELL).exists()
}

pub fn supervise(cmdline: &Cmdline) -> ! {
    let mut quick_crashes = 0;

    loop {
        log::info(&format!(
            "starting session: {} as {}",
            cmdline.session, cmdline.user
        ));

        let started = Instant::now();
        let outcome = match session_account(&cmdline.user) {
            Ok(account) => run(&cmdline.session, &[], &account),
            Err(error) => Outcome::NotStarted(error),
        };
        console::reset_vt();

        let failure = match outcome {
            Outcome::Power(request) => power::shutdown(request),
            Outcome::Exited(0) => {
                log::info("session ended");
                quick_crashes = 0;
                after_exit(cmdline.on_exit);
                continue;
            }
            Outcome::Exited(EXIT_POWER_OFF) => power::shutdown(Request::PowerOff),
            Outcome::Exited(EXIT_REBOOT) => power::shutdown(Request::Reboot),
            Outcome::Exited(EXIT_LOG_OUT) => {
                log::info("logged out");
                quick_crashes = 0;
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

/// The account the session runs as, ready to start in. Fails rather than
/// falling back to root: running the session as root has to be asked for
/// (`saule.user=root`).
fn session_account(name: &str) -> std::io::Result<Account> {
    let account = Account::find(name)
        .ok_or_else(|| std::io::Error::other(format!("no account `{name}` in /etc/passwd")))?;

    if !account.is_root() {
        account.prepare()?;
        lend_terminals(&account);
    }

    Ok(account)
}

/// Let the session's account use the terminals it draws over, the way a
/// login hands a user their terminal: the console it runs on, and `tty0`,
/// through which Shine switches the screen out of text mode.
fn lend_terminals(account: &Account) {
    for tty in [console::device(), "/dev/tty0".into()] {
        let path = Path::new(&tty);
        if path.exists() {
            let _ = account::chown(path, account.uid, account.gid);
            let _ = account::chmod(path, 0o620);
        }
    }
}

/// A debugging shell (debug images only), as root. Leaving it restarts the
/// session.
fn shell() {
    log::info("shell — `exit` restarts the session, `poweroff` turns the machine off");

    let root = Account::find("root").unwrap_or(Account {
        name: "root".into(),
        uid: 0,
        gid: 0,
        groups: Vec::new(),
        home: "/root".into(),
    });

    match run(SHELL, &["-l"], &root) {
        Outcome::Power(request) => power::shutdown(request),
        Outcome::NotStarted(error) => log::warn(&format!("could not start {SHELL}: {error}")),
        _ => {}
    }
}

/// Start `program` on the console as `account` and wait for it, reaping any
/// other process that dies meanwhile. Returns early if a power signal
/// arrives.
fn run(program: &str, args: &[&str], account: &Account) -> Outcome {
    let device = console::device();
    let tty = match CString::new(device.clone()) {
        Ok(tty) => tty,
        Err(error) => return Outcome::NotStarted(error.into()),
    };

    // With more than one console — the screen and a serial line, as QEMU
    // and a debugging setup have — what the session prints goes to all of
    // them. It runs on the screen's terminal, and a session that draws
    // covers that terminal: its errors, the one thing needed when it
    // fails, were on a console nobody could read. So its output goes
    // through a pipe that init copies to every console. With one console
    // it writes to its terminal directly, as before.
    let consoles = console::devices();
    let output = if consoles.len() > 1 {
        match Output::new(consoles) {
            Ok(output) => Some(output),
            Err(error) => return Outcome::NotStarted(error),
        }
    } else {
        None
    };
    let write_end = output.as_ref().map(|o| o.write_end);

    let home = if account.home.is_dir() {
        account.home.as_path()
    } else {
        Path::new("/")
    };

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(home)
        .env_clear()
        .env("PATH", "/bin:/usr/bin")
        .env("HOME", home)
        .env("USER", &account.name)
        .env("LOGNAME", &account.name)
        .env("TERM", "linux")
        .env("LANG", "C.UTF-8");

    if !account.is_root() {
        command.env("XDG_RUNTIME_DIR", account.runtime_dir());
    }

    // Root stays root; anyone else is switched to in the child, after it
    // has taken the terminal (which needs root).
    let switch = (!account.is_root()).then(|| Switch::new(account));

    // SAFETY: `take_over` and `Switch::apply` only make async-signal-safe
    // syscalls, and everything they use was prepared before the fork.
    unsafe {
        command.pre_exec(move || {
            console::take_over(&tty)?;
            // The terminal stays stdin and the controlling terminal, for
            // Ctrl-C; only the output goes to the pipe.
            if let Some(fd) = write_end {
                if libc::dup2(fd, 1) < 0 || libc::dup2(fd, 2) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            match &switch {
                Some(switch) => switch.apply(),
                None => Ok(()),
            }
        });
    }

    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            if let Some(output) = output {
                output.close();
            }
            return Outcome::NotStarted(error);
        }
    };
    if let Some(output) = output {
        output.copy_in_background();
    }

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

/// The pipe a session's output goes through on its way to every console.
struct Output {
    read_end: libc::c_int,
    write_end: libc::c_int,
    consoles: Vec<String>,
}

impl Output {
    fn new(consoles: Vec<String>) -> std::io::Result<Output> {
        let mut fds = [0; 2];
        // Close-on-exec: the session gets the write end as its stdout and
        // stderr (`dup2` clears the flag on those), and nothing else it
        // runs inherits the originals.
        // SAFETY: `fds` has room for the two descriptors pipe2 writes.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Output {
            read_end: fds[0],
            write_end: fds[1],
            consoles,
        })
    }

    /// The session never started: nothing will use the pipe.
    fn close(self) {
        // SAFETY: both were made by `new` and nothing else closes them.
        unsafe {
            libc::close(self.read_end);
            libc::close(self.write_end);
        }
    }

    /// Once the session has started: let go of init's write end, so the
    /// pipe ends when the session and the apps it started have closed
    /// theirs, and copy what arrives to every console until then.
    fn copy_in_background(self) {
        use std::io::{Read, Write};
        use std::os::fd::FromRawFd;

        // SAFETY: both descriptors were made by `new` and are owned here;
        // each is closed exactly once (`File` closes the read end on drop).
        unsafe { libc::close(self.write_end) };
        let mut pipe = unsafe { std::fs::File::from_raw_fd(self.read_end) };

        let mut consoles: Vec<std::fs::File> = self
            .consoles
            .iter()
            .filter_map(|path| std::fs::OpenOptions::new().write(true).open(path).ok())
            .collect();

        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        for console in &mut consoles {
                            let _ = console.write_all(&buffer[..n]);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
    }
}

//! Starting the session and keeping it alive.
//!
//! The session is the one program the machine exists to run: a Saule
//! project built into one executable with `saule build` — the window
//! manager, or the hello-world in `session/`. Init starts it, waits, and
//! decides what happens when it ends:
//!
//! - it **returned** (exit code 0)  → follow `saule.on_exit`;
//! - it **asked** for something (exit codes 80–83) → power off, reboot,
//!   start a new session, or, from the installer, start the system itself;
//! - it **crashed** (non-zero/signal) → start it again after a second; if it
//!   keeps crashing straight away, stop and fall back to a shell or power off.

use std::ffi::CString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::account::{self, Account, Switch};
use crate::cmdline::{self, Cmdline, OnExit};
use crate::output::Output;
use crate::power::{self, Request};
use crate::{console, launcher, log, person, storage, system};

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
/// From the installer only: don't install now, start Saule Linux as it
/// runs from the stick. The ordinary session, as its ordinary account, for
/// the rest of this start; nothing is kept, as no disk was mounted.
pub const EXIT_TRY: i32 = 83;

pub fn has_shell() -> bool {
    Path::new(SHELL).exists()
}

pub fn supervise(cmdline: &Cmdline) -> ! {
    let mut quick_crashes = 0;
    let mut session = cmdline.session.clone();
    let mut user = cmdline.user.clone();

    loop {
        log::info(&format!("starting session: {session} as {user}"));

        let started = Instant::now();
        let outcome = match session_account(&user) {
            Ok(account) => run(&session, &[], &cmdline.env, &account),
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
            Outcome::Exited(EXIT_TRY) if cmdline.installer => {
                log::info("trying the system without installing it");
                session = cmdline::SESSION.into();
                user = cmdline::USER.into();
                quick_crashes = 0;
                continue;
            }
            Outcome::Exited(code) => format!("session exited with code {code}"),
            Outcome::Killed(signal) => format!("session killed by signal {signal}"),
            Outcome::NotStarted(error) => {
                format!("could not start {session}: {error}")
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
        system::lend_to(&account);
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

    match run(SHELL, &["-l"], &[], &root) {
        Outcome::Power(request) => power::shutdown(request),
        Outcome::NotStarted(error) => log::warn(&format!("could not start {SHELL}: {error}")),
        _ => {}
    }
}

/// Start `program` on the console as `account` and wait for it, reaping any
/// other process that dies meanwhile. Returns early if a power signal
/// arrives.
fn run(program: &str, args: &[&str], env: &[(String, String)], account: &Account) -> Outcome {
    let tty = match CString::new(console::device()) {
        Ok(tty) => tty,
        Err(error) => return Outcome::NotStarted(error.into()),
    };
    let output = match Output::for_consoles(console::devices()) {
        Ok(output) => output,
        Err(error) => return Outcome::NotStarted(error),
    };
    let write_end = output.as_ref().map(Output::write_end);

    let mut command = command_for(program, args, env, account);

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
    // The apps it asks for print where it does: through a write end of
    // the launcher's own, taken before init lets go of this one.
    if !account.is_root() {
        // SAFETY: duplicates a descriptor that is open; the copy is
        // closed on exec, as the original is.
        let copy = write_end
            .map(|fd| unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) })
            .filter(|fd| *fd >= 0);
        launcher::session_started(account, copy);
    }
    if let Some(output) = output {
        output.copy_in_background();
    }

    let outcome = wait_for(child.id() as libc::pid_t);
    if !account.is_root() {
        launcher::session_ended();
    }
    outcome
}

/// `program` with the environment a session starts in: clean, but for who
/// it is, where its home is, and `saule.env.NAME=value` from the kernel
/// command line.
fn command_for(
    program: &str,
    args: &[&str],
    env: &[(String, String)],
    account: &Account,
) -> Command {
    let home = home_of(account);

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&home)
        .env_clear()
        .env("PATH", "/bin:/usr/bin")
        .env("HOME", &home)
        .env("USER", &account.name)
        .env("LOGNAME", &account.name)
        .env("TERM", "linux")
        .env("LANG", "C.UTF-8");

    if !account.is_root() {
        command.env("XDG_RUNTIME_DIR", account.runtime_dir());
        // Its services, where its apps can reach them, and who to ask to
        // start an app (`launcher.rs`).
        command.env("SAULE_SERVICES_DIR", launcher::SERVICES);
        command.env("SAULE_LAUNCHER", launcher::FOLDER);
        // Whose machine it is: the session's own account is nobody's.
        if let Some(person) = person_of_the_machine() {
            command.env("SAULE_PERSON", &person.name);
            command.env(
                "SAULE_PERSON_HOME",
                person::home_of(&person, Some(account.gid)),
            );
        }
    }
    if storage::mounted() {
        command.env("SAULE_DATA", storage::MOUNT);
    }
    // For programs that ask the C library for local time. Saule's own
    // `Os.date` doesn't: see `system.rs`.
    if let Some(zone) = system::zone() {
        command.env("TZ", zone);
    }
    // With the home on the data disk, caches stay behind in memory. A cache
    // is rebuilt when it is missing, but one a power cut left half-written
    // on the disk is found, and trusted, on every start after: a built
    // Saule app unpacks its libraries into one, and a session that can't
    // load them never starts again.
    if home != account.home && account.home.is_dir() {
        command.env("XDG_CACHE_HOME", account.home.join(".cache"));
    }
    command.envs(env.iter().map(|(name, value)| (name, value)));

    command
}

/// The person's account, as it is called now: the one account in
/// `/etc/passwd` with the person's number.
fn person_of_the_machine() -> Option<Account> {
    let named = Account::find(person::ACCOUNT);
    named.or_else(|| {
        let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
        let uid = person::UID.to_string();
        let name = passwd.lines().find_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            (fields.get(2) == Some(&uid.as_str())).then(|| fields[0].to_string())
        })?;
        Account::find(&name)
    })
}

/// Where `account` starts: its home on the data disk when there is one, so
/// what it keeps there outlives a reboot; else its home in memory.
fn home_of(account: &Account) -> PathBuf {
    if let Some(home) = storage::home_for(&account.name, account.uid, account.gid) {
        return home;
    }
    if account.home.is_dir() {
        return account.home.clone();
    }
    PathBuf::from("/")
}

/// Wait until process `pid` ends. Not `Child::wait`: init must also reap
/// orphans, so it waits for any process and only stops when the one it
/// started is among them.
fn wait_for(pid: libc::pid_t) -> Outcome {
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
            // An app, perhaps: the launcher says so to who asked for it.
            launcher::reaped(reaped, status);
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

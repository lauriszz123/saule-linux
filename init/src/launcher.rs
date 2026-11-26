//! Starting apps, for the session.
//!
//! The window manager draws the screen and reads the keyboard, so its
//! account (`display`) is in the groups those devices belong to. An app it
//! started itself would be its child, the same account with the same
//! groups, and could read the keyboard behind its back. So it doesn't start
//! apps: it asks init to, and init starts them as the person's account
//! (`person.rs`), which is in none of those groups, with nothing of the
//! window manager's about them but the environment a program needs.
//!
//! It asks the way `saule-system` is asked: a file dropped in a folder,
//! [`FOLDER`]`/requests`, which is the session's account's alone, mode
//! 0700, so who is asking is the kernel's to say and not the request's.
//! A request is one line, its fields set apart by tabs ([`fields`]):
//!
//!   launch  ID  PROGRAM  [a:ARGUMENT]…  [e:NAME=VALUE]…
//!   kill    PID                 end an app started here, at once
//!   forget  ID                  an app that ended has been seen to
//!
//! `ID` is the asker's own number for the app, by which it finds it in the
//! answer: [`FOLDER`]`/apps`, a line for each app asked for and not yet
//! forgotten,
//!
//!   ID  PID  running
//!   ID  PID  ended   CODE       -1 when a signal ended it
//!   ID  0    refused REASON     never started
//!
//! Only a program in [`APPS`] is started: an app that is installed, not
//! whatever path was asked for.
//!
//! The apps are init's children. Its main loop, which reaps every process,
//! says when one has ended ([`reaped`]); they are ended with the session
//! ([`session_ended`]), and what they print goes where the session's does
//! ([`session_started`]).
//!
//! The apps reach the window manager through its services, sockets in a
//! folder ([`SERVICES`]) that Saule makes for their owner alone. Here it is
//! the session's to publish in and the person's to connect to, which init
//! sees to: see [`open_services`].

use std::ffi::CString;
use std::fs;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::account::{self, Account, Switch};
use crate::{log, person, storage, system};

pub const FOLDER: &str = "/run/saule/launcher";
/// Where the session's services are published, and its apps find them.
pub const SERVICES: &str = "/run/saule/services";
/// The apps that are installed: the only programs started.
const APPS: &str = "/bin/apps";

/// The longest request read. A file's path and a few settings fit in far
/// less.
const LONGEST: u64 = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Stands {
    Running,
    Ended(i32),
    Refused(String),
}

struct App {
    id: String,
    pid: libc::pid_t,
    stands: Stands,
}

struct State {
    /// The person's account, as it is called on this machine.
    person: String,
    /// `saule.env.NAME=value` from the kernel's command line: the apps'
    /// too.
    env: Vec<(String, String)>,
    apps: Vec<App>,
    /// Where what the apps print goes: the session's own pipe, while there
    /// is a session.
    output: Option<fs::File>,
    /// The session's group, which may look in the person's Pictures.
    session_gid: Option<libc::gid_t>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Take requests from now on. `person` is the account apps run as, `env`
/// what the kernel's command line set for every program.
pub fn start(person: &str, env: &[(String, String)]) {
    let requests = requests();
    if let Err(error) = fs::create_dir_all(&requests) {
        log::warn(&format!("apps can't be started: {FOLDER}: {error}"));
        return;
    }
    for (folder, mode) in [
        (Path::new("/run/saule"), 0o755),
        (Path::new(FOLDER), 0o755),
        (requests.as_path(), 0o700),
    ] {
        let _ = account::chmod(folder, mode);
    }
    if let Ok(mut state) = STATE.lock() {
        *state = Some(State {
            person: person.to_string(),
            env: env.to_vec(),
            apps: Vec::new(),
            output: None,
            session_gid: None,
        });
    }
    say();

    let spawned = std::thread::Builder::new()
        .name("launcher".into())
        .spawn(serve);
    if let Err(error) = spawned {
        log::warn(&format!("apps can't be started: {error}"));
    }
}

fn requests() -> PathBuf {
    Path::new(FOLDER).join("requests")
}

/// A session starts as `account`: the requests are its to make, the
/// services folder its to publish in, and what apps print goes to
/// `output`, its own pipe (a descriptor of init's own, taken over here).
pub fn session_started(account: &Account, output: Option<libc::c_int>) {
    let requests = requests();
    // What a session before it left unasked is not this one's.
    for entry in fs::read_dir(&requests).into_iter().flatten().flatten() {
        let _ = fs::remove_file(entry.path());
    }
    let _ = account::chown(&requests, account.uid, account.gid);

    let person = with(|state| {
        // SAFETY: the descriptor is init's own, handed over for good.
        state.output = output.map(|fd| unsafe { fs::File::from_raw_fd(fd) });
        state.session_gid = Some(account.gid);
        Account::find(&state.person)
    })
    .flatten();

    // The services: published by the session, connected to by the person.
    let services = Path::new(SERVICES);
    let _ = fs::create_dir_all(services);
    for entry in fs::read_dir(services).into_iter().flatten().flatten() {
        let _ = fs::remove_file(entry.path());
    }
    let group = person.as_ref().map_or(account.gid, |person| person.gid);
    let _ = account::chown(services, account.uid, group);
    let _ = account::chmod(services, 0o750);
}

/// The session has ended: so do the apps it asked for, and nothing more
/// is written where it wrote.
pub fn session_ended() {
    let running: Vec<libc::pid_t> = with(|state| {
        state.output = None;
        let running = state
            .apps
            .iter()
            .filter(|app| app.stands == Stands::Running)
            .map(|app| app.pid)
            .collect();
        state.apps.clear();
        running
    })
    .unwrap_or_default();
    for pid in running {
        // SAFETY: a signal to a process init started and has not reaped,
        // so the number is still its.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    say();
}

/// Init's main loop reaped process `pid`, which ended with `status` (as
/// `waitpid` gives it). Most are not apps: those are ignored.
pub fn reaped(pid: libc::pid_t, status: libc::c_int) {
    let code = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else {
        -1
    };
    let ours = with(|state| {
        let app = state
            .apps
            .iter_mut()
            .find(|app| app.pid == pid && app.stands == Stands::Running);
        match app {
            Some(app) => {
                app.stands = Stands::Ended(code);
                true
            }
            None => false,
        }
    });
    if ours == Some(true) {
        say();
    }
}

/// Run `f` on the state, when there is one.
fn with<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    let mut state = STATE.lock().ok()?;
    state.as_mut().map(f)
}

/// Act on every request, as it comes, for as long as the machine runs.
fn serve() {
    // Signals are for the main thread: its `waitpid` must be the one they
    // interrupt (see `power.rs`).
    // SAFETY: a full set, applied to this thread only.
    unsafe {
        let mut all: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut all);
        libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
    }

    let folder = requests();
    let watch = watch(&[folder.as_path(), Path::new(SERVICES)]);
    let mut buffer = [0u8; 4096];
    loop {
        open_services();
        for request in take(&folder) {
            act(&request);
        }
        match &watch {
            // Blocks until something is dropped in a folder. What the
            // events say is not looked at: the folders are.
            // SAFETY: reads into a buffer of that length.
            Some(file) => unsafe {
                libc::read(file.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len());
            },
            None => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

/// Be told when something is put in one of `folders`, or changed there.
/// The services folder is made here if it isn't there yet, so that it can
/// be watched from the start.
fn watch(folders: &[&Path]) -> Option<fs::File> {
    // SAFETY: plain system calls; the paths are NUL-terminated.
    unsafe {
        let fd = libc::inotify_init1(libc::IN_CLOEXEC);
        if fd < 0 {
            return None;
        }
        let file = fs::File::from_raw_fd(fd);
        let events = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_CREATE | libc::IN_ATTRIB;
        for folder in folders {
            let _ = fs::create_dir_all(folder);
            let path = CString::new(folder.as_os_str().as_encoded_bytes()).ok()?;
            if libc::inotify_add_watch(fd, path.as_ptr(), events) < 0 {
                return None;
            }
        }
        Some(file)
    }
}

/// Let the person's account connect to the session's services.
///
/// Saule publishes a service as a socket only its owner may connect to,
/// in a folder only its owner may enter: right where everything is one
/// account's. Here the display server is one account and its apps
/// another, and who may reach it is init's to say. The folder is the
/// session's to write and the person's group's to look in
/// ([`session_started`]); each socket made there is given to that group
/// too, as it appears. The person's apps can then connect, and still not
/// publish: a service there is the window manager's, and nothing else's.
fn open_services() {
    let Some(Some(person)) = with(|state| Account::find(&state.person)) else {
        return;
    };
    for entry in fs::read_dir(SERVICES).into_iter().flatten().flatten() {
        let path = entry.path();
        let Ok(now) = fs::symlink_metadata(&path) else {
            continue;
        };
        if now.file_type().is_symlink() || now.is_dir() {
            continue;
        }
        if now.gid() != person.gid {
            let _ = std::os::unix::fs::lchown(&path, None, Some(person.gid));
        }
        if now.mode() & 0o777 != 0o660 {
            let _ = account::chmod(&path, 0o660);
        }
    }
}

/// Every request waiting in `folder`, oldest first, each removed as it is
/// read. A file still being written (`.part`, renamed when whole) is left.
fn take(folder: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut waiting: Vec<_> = entries
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".part"))
        .map(|entry| {
            let made = entry.metadata().and_then(|m| m.modified()).ok();
            (made, entry.path())
        })
        .collect();
    waiting.sort();

    let mut requests = Vec::new();
    for (_, path) in waiting {
        use std::io::Read;
        let mut text = String::new();
        if let Ok(file) = fs::File::open(&path) {
            let _ = file.take(LONGEST).read_to_string(&mut text);
        }
        let _ = fs::remove_file(&path);
        if let Some(line) = text.lines().next().filter(|line| !line.is_empty()) {
            requests.push(line.to_string());
        }
    }
    requests
}

/// A request's fields: set apart by tabs, with a tab, a new line or a
/// backslash inside one written `\t`, `\n`, `\\`.
fn fields(line: &str) -> Vec<String> {
    line.split('\t')
        .map(|field| {
            let mut out = String::with_capacity(field.len());
            let mut chars = field.chars();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    Some('t') => out.push('\t'),
                    Some('n') => out.push('\n'),
                    Some(other) => out.push(other),
                    None => {}
                }
            }
            out
        })
        .collect()
}

/// A field as it is written in the answer: the other way.
fn written(field: &str) -> String {
    field
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn act(request: &str) {
    let fields = fields(request);
    let word = fields.first().map(String::as_str).unwrap_or("");
    match (word, fields.get(1)) {
        ("launch", Some(id)) => {
            let program = fields.get(2).cloned().unwrap_or_default();
            let stands = match launch(&program, &fields[fields.len().min(3)..]) {
                Ok(pid) => {
                    log::info(&format!("app {program} started as process {pid}"));
                    (pid, Stands::Running)
                }
                Err(reason) => {
                    log::warn(&format!("app {program} not started: {reason}"));
                    (0, Stands::Refused(reason))
                }
            };
            with(|state| {
                state.apps.retain(|app| &app.id != id);
                state.apps.push(App {
                    id: id.clone(),
                    pid: stands.0,
                    stands: stands.1,
                });
            });
            say();
        }
        ("kill", Some(pid)) => {
            let Ok(pid) = pid.parse::<libc::pid_t>() else {
                return;
            };
            let ours = with(|state| {
                state
                    .apps
                    .iter()
                    .any(|app| app.pid == pid && app.stands == Stands::Running)
            });
            if ours == Some(true) {
                // SAFETY: a signal to a child init has not reaped yet.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        ("forget", Some(id)) => {
            with(|state| {
                state
                    .apps
                    .retain(|app| &app.id != id || app.stands == Stands::Running)
            });
            say();
        }
        _ => log::warn("an unknown request to start an app"),
    }
}

/// The installed app at `asked`, as the file it is: `None` when it is not
/// a program in [`APPS`], through whatever links.
fn installed(asked: &str, apps: &Path) -> Option<PathBuf> {
    let program = fs::canonicalize(asked).ok()?;
    let apps = fs::canonicalize(apps).ok()?;
    (program.parent() == Some(apps.as_path()) && program.is_file()).then_some(program)
}

/// Start the app `asked` as the person, with what `rest` of the request
/// says: `a:` an argument, `e:` a setting of its environment.
fn launch(asked: &str, rest: &[String]) -> Result<libc::pid_t, String> {
    let program = installed(asked, Path::new(APPS))
        .ok_or_else(|| format!("{asked} is not an installed app"))?;

    let (person, env, output, session_gid) = with(|state| {
        let output = state.output.as_ref().and_then(|file| file.try_clone().ok());
        (
            Account::find(&state.person),
            state.env.clone(),
            output,
            state.session_gid,
        )
    })
    .ok_or("no launcher")?;
    let person = person.ok_or("the person has no account")?;
    person.prepare().map_err(|error| error.to_string())?;
    let home = person::home_of(&person, session_gid);
    // Before its first instruction: the sockets it will connect to.
    open_services();

    let mut command = Command::new(&program);
    command
        .current_dir(&home)
        .env_clear()
        .env("PATH", "/bin:/usr/bin")
        .env("HOME", &home)
        .env("USER", &person.name)
        .env("LOGNAME", &person.name)
        .env("TERM", "linux")
        .env("LANG", "C.UTF-8")
        .env("XDG_RUNTIME_DIR", person.runtime_dir())
        .env("SAULE_SERVICES_DIR", SERVICES)
        .stdin(Stdio::null());
    if storage::mounted() {
        command.env("SAULE_DATA", storage::MOUNT);
    }
    if let Some(zone) = system::zone() {
        command.env("TZ", zone);
    }
    // Caches stay in memory when the home is on the disk: see
    // `session::command_for`.
    if home != person.home && person.home.is_dir() {
        command.env("XDG_CACHE_HOME", person.home.join(".cache"));
    }
    command.envs(env.iter().map(|(name, value)| (name, value)));
    for field in rest {
        if let Some(argument) = field.strip_prefix("a:") {
            command.arg(argument);
        } else if let Some((name, value)) = field
            .strip_prefix("e:")
            .and_then(|pair| pair.split_once('='))
        {
            if asked_for(name) {
                command.env(name, value);
            }
        }
    }
    match &output {
        Some(output) => {
            if let (Ok(out), Ok(err)) = (output.try_clone(), output.try_clone()) {
                command.stdout(out).stderr(err);
            }
        }
        None => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }

    let switch = Switch::new(&person);
    // SAFETY: only async-signal-safe calls, on what was made before the
    // fork.
    unsafe {
        command.pre_exec(move || {
            // A session of its own: nothing typed at init's terminal is a
            // signal to it. And this thread's blocked signals are not the
            // app's to inherit.
            libc::setsid();
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            switch.apply()
        });
    }
    let child = command.spawn().map_err(|error| error.to_string())?;
    Ok(child.id() as libc::pid_t)
}

/// Whether the session may set `name` for an app: what Saule and Shine
/// read, not who the app is or where it looks for programs and libraries.
fn asked_for(name: &str) -> bool {
    (name.starts_with("SAULE_") || name.starts_with("SHINE_")) && name != "SAULE_SERVICES_DIR"
}

/// Write how the apps stand, beside and renamed over, so a reader never
/// sees half.
fn say() {
    let text = with(|state| {
        let mut text = String::new();
        for app in &state.apps {
            let stands = match &app.stands {
                Stands::Running => "running".to_string(),
                Stands::Ended(code) => format!("ended\t{code}"),
                Stands::Refused(reason) => format!("refused\t{}", written(reason)),
            };
            text.push_str(&format!("{}\t{}\t{stands}\n", written(&app.id), app.pid));
        }
        text
    })
    .unwrap_or_default();
    let path = Path::new(FOLDER).join("apps");
    let part = Path::new(FOLDER).join(".apps.part");
    if fs::write(&part, text).is_ok() {
        let _ = fs::set_permissions(&part, fs::Permissions::from_mode(0o644));
        let _ = fs::rename(&part, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_s_fields_are_set_apart_by_tabs_and_may_hold_one() {
        assert_eq!(
            fields("launch\t7\t/bin/apps/editor\ta:/home/ada/a\\tb.txt\te:SHINE_SCALE=1.5"),
            [
                "launch",
                "7",
                "/bin/apps/editor",
                "a:/home/ada/a\tb.txt",
                "e:SHINE_SCALE=1.5"
            ]
        );
        assert_eq!(fields("a\\\\b\\nc"), ["a\\b\nc"]);
        // And back, as an answer is written.
        for field in ["plain", "a\tb", "two\nlines", "back\\slash", ""] {
            assert_eq!(fields(&written(field)), [field]);
        }
    }

    #[test]
    fn only_an_installed_app_is_started() {
        let root = std::env::temp_dir().join(format!("saule-launcher-{}", std::process::id()));
        let apps = root.join("apps");
        fs::create_dir_all(apps.join("folder")).unwrap();
        fs::write(apps.join("editor"), "").unwrap();
        fs::write(root.join("elsewhere"), "").unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), apps.join("link")).unwrap();

        let asked = |path: PathBuf| installed(&path.to_string_lossy(), &apps);
        assert!(asked(apps.join("editor")).is_some());
        // Not a path that leaves the folder, a link out of it, a folder,
        // or nothing at all.
        assert!(asked(apps.join("../elsewhere")).is_none());
        assert!(asked(apps.join("link")).is_none());
        assert!(asked(apps.join("folder")).is_none());
        assert!(asked(apps.join("missing")).is_none());
        assert!(asked(root.join("elsewhere")).is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_session_sets_what_saule_reads_and_nothing_else() {
        assert!(asked_for("SHINE_SCALE"));
        assert!(asked_for("SAULE_WM_KEY"));
        assert!(!asked_for("PATH"));
        assert!(!asked_for("LD_PRELOAD"));
        assert!(!asked_for("HOME"));
        assert!(!asked_for("SAULE_SERVICES_DIR"));
    }
}

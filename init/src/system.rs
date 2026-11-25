//! What the person chose for the machine: its name and its time zone.
//!
//! They are kept on the data disk, in `/data/system`, one small file each,
//! written by the session (the window manager's first-start setup) and
//! acted on here, since naming the machine and setting its clock take
//! root:
//!
//!   hostname   the machine's name
//!   timezone   the zone it is in: `Europe/Vilnius`
//!   clock      `local` when the PC's own clock keeps local time, as
//!              Windows sets it; the installer writes this when it erased
//!              a Windows. Anything else, or no file: UTC.
//!
//! The folder is watched, so a choice takes effect as it is made, without
//! a restart.
//!
//! What a zone means right now is written to `/run/saule/time` for the
//! session to read, one line: the zone, how many seconds ahead of UTC it
//! is, when that next changes (Unix seconds, 0 for never) and to what.
//! Saule's `Os.date` is UTC only, so a program that shows the time adds
//! the seconds itself.
//!
//! The kernel takes the PC's clock for UTC when it starts. With
//! `clock = local` and a zone, the system's clock is put back by the
//! zone's distance from UTC, once, here.

use std::ffi::CString;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::Mutex;

use crate::account::{self, Account};
use crate::zone::{self, Offset};
use crate::{log, storage};

const FOLDER: &str = "/data/system";
const TIME: &str = "/run/saule/time";

/// What was last put into effect, so that a change of zone can undo the
/// clock's correction before making the new one.
struct Applied {
    hostname: String,
    zone: String,
    /// Seconds the system's clock was put back by, for `clock = local`.
    correction: i64,
}

static APPLIED: Mutex<Applied> = Mutex::new(Applied {
    hostname: String::new(),
    zone: String::new(),
    correction: 0,
});

/// Put what is kept into effect, and keep doing so as it changes. Without
/// a data disk there is nothing kept: the machine keeps the name it was
/// built with and runs in UTC.
pub fn start() {
    write_time("UTC", Offset::default());
    if !storage::mounted() {
        return;
    }
    if let Err(error) = fs::create_dir_all(FOLDER) {
        log::warn(&format!("could not make {FOLDER}: {error}"));
        return;
    }

    apply();
    std::thread::spawn(watch);
}

/// The folder is the session's to write in: the one person of the machine
/// names it and says where it is.
pub fn lend_to(account: &Account) {
    let folder = Path::new(FOLDER);
    if storage::mounted() && folder.is_dir() && !account.is_root() {
        let _ = account::chown(folder, account.uid, account.gid);
        let _ = account::chmod(folder, 0o755);
        // And what the installer left in it (the keyboard's layout), which
        // is root's as the installer wrote it.
        for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ = account::chown(&entry.path(), account.uid, account.gid);
            }
        }
    }
}

/// The zone the session should start in (its `TZ`), if one was chosen.
pub fn zone() -> Option<String> {
    let zone = read("timezone")?;
    zone::known(&zone).then_some(zone)
}

fn read(name: &str) -> Option<String> {
    let text = fs::read_to_string(Path::new(FOLDER).join(name)).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

fn apply() {
    let Ok(mut applied) = APPLIED.lock() else {
        return;
    };

    if let Some(name) = read("hostname").and_then(|name| host_name(&name)) {
        if name != applied.hostname && set_hostname(&name) {
            log::info(&format!("this machine is called {name}"));
            applied.hostname = name;
        }
    }

    let zone = zone().unwrap_or_else(|| "UTC".to_string());
    let note = read("clock");
    let local = note.as_deref() == Some("local");
    // `synced`: the clock was set from a time server, and the PC's own
    // clock with it, in UTC (saule-system's `clock.rs`). It is right as
    // it stands: whatever was added for a clock kept in local time is no
    // longer owed, and nothing is to be moved.
    if note.as_deref() == Some("synced") {
        applied.correction = 0;
    }

    // What the zone is could not be found out (it was a moment ago, when
    // it was chosen from the list of them): nothing about the time is
    // touched, rather than the clock set by a guess.
    let ask = |at: i64| -> Option<Offset> {
        if zone == "UTC" {
            Some(Offset::default())
        } else {
            zone::ask(&zone, at)
        }
    };

    // The clock as the kernel read it: with any earlier correction undone.
    let read_as_utc = now() + applied.correction;
    let correction = if local {
        // The PC's clock shows the zone's time. Which instant that is
        // depends on the zone's offset then, found from a first guess.
        let Some(guess) = ask(read_as_utc) else {
            return;
        };
        let Some(then) = ask(read_as_utc - guess.seconds) else {
            return;
        };
        then.seconds
    } else {
        0
    };
    if correction != applied.correction && set_clock(read_as_utc - correction) {
        applied.correction = correction;
    }

    let Some(offset) = ask(now()) else {
        return;
    };
    write_time(&zone, offset);
    if zone != applied.zone {
        log::info(&format!(
            "time zone: {zone} ({:+} h)",
            offset.seconds as f64 / 3600.0
        ));
        applied.zone = zone;
    }
}

/// `name` as a host name: letters, digits and hyphens, at most 63, with
/// whatever else a person typed turned into hyphens. `None` when nothing
/// is left.
fn host_name(name: &str) -> Option<String> {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    let out: String = out.chars().take(63).collect();
    let out = out.trim_end_matches('-').to_string();
    (!out.is_empty()).then_some(out)
}

/// What the firmware calls the machine (DMI's product name), as a host
/// name; `None` where it says nothing, or one of the things a board says
/// when its maker never filled the name in.
pub fn model_name() -> Option<String> {
    let product = fs::read_to_string("/sys/class/dmi/id/product_name").ok()?;
    model_from(&product)
}

fn model_from(product: &str) -> Option<String> {
    const NOTHING: [&str; 8] = [
        "o.e.m",
        "system product name",
        "system name",
        "default string",
        "not specified",
        "not applicable",
        "unknown",
        "none",
    ];
    let said = product.trim().to_lowercase();
    if said.is_empty() || NOTHING.iter().any(|nothing| said.contains(nothing)) {
        return None;
    }
    host_name(product)
}

fn set_hostname(name: &str) -> bool {
    // SAFETY: the pointer and length describe `name`, which outlives the call.
    unsafe { libc::sethostname(name.as_ptr().cast(), name.len()) == 0 }
}

fn now() -> i64 {
    // SAFETY: a plain system call with a null argument.
    unsafe { libc::time(std::ptr::null_mut()) }
}

fn set_clock(seconds: i64) -> bool {
    let time = libc::timespec {
        tv_sec: seconds,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a valid timespec for the length of the call.
    unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &time) == 0 }
}

/// Written beside and renamed over, so a reader never sees half a line.
fn write_time(zone: &str, offset: Offset) {
    let line = format!(
        "{zone} {} {} {}\n",
        offset.seconds, offset.next_at, offset.next_seconds
    );
    let part = format!("{TIME}.part");
    if let Some(folder) = Path::new(TIME).parent() {
        let _ = fs::create_dir_all(folder);
    }
    if fs::write(&part, line).is_ok() {
        let _ = account::chmod(Path::new(&part), 0o644);
        let _ = fs::rename(&part, TIME);
    }
}

/// Wait for the folder to change and apply what is in it, for as long as
/// the machine runs. Also once a day unasked: a zone's next change is only
/// looked up a year ahead.
fn watch() {
    const EVENTS: u32 = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_DELETE;
    const DAY_MS: libc::c_int = 86_400_000;

    let Ok(folder) = CString::new(FOLDER) else {
        return;
    };
    // SAFETY: plain system calls; the path is NUL-terminated.
    let fd = unsafe {
        let fd = libc::inotify_init1(libc::IN_CLOEXEC);
        if fd < 0 || libc::inotify_add_watch(fd, folder.as_ptr(), EVENTS) < 0 {
            log::warn("could not watch the machine's settings: they apply at the next start");
            return;
        }
        fd
    };
    // Owned, so that it is closed if this ever returns.
    // SAFETY: `fd` was just opened and is used nowhere else.
    let file = unsafe { <fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };

    let mut buffer = [0u8; 4096];
    loop {
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd; the buffer is this function's own.
        unsafe {
            if libc::poll(&mut poll, 1, DAY_MS) > 0 {
                // Whatever changed, all of it is read again: drain the
                // events, and let a burst of them settle.
                libc::read(file.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len());
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        apply();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_is_called_after_what_it_is() {
        assert_eq!(
            model_from("Inspiron 7559\n").as_deref(),
            Some("Inspiron-7559")
        );
        assert_eq!(
            model_from("HP Mini 210-1000").as_deref(),
            Some("HP-Mini-210-1000")
        );
        assert_eq!(
            model_from("Standard PC (Q35 + ICH9, 2009)").as_deref(),
            Some("Standard-PC-Q35-ICH9-2009")
        );
        // What a board says when nobody named it is no name.
        assert_eq!(model_from("To Be Filled By O.E.M."), None);
        assert_eq!(model_from("System Product Name"), None);
        assert_eq!(model_from("Default string"), None);
        assert_eq!(model_from("  "), None);
    }

    #[test]
    fn a_name_as_typed_becomes_a_host_name() {
        assert_eq!(host_name("netbook").as_deref(), Some("netbook"));
        assert_eq!(
            host_name("  Lauris's Netbook ").as_deref(),
            Some("Lauris-s-Netbook")
        );
        assert_eq!(host_name("a  b__c").as_deref(), Some("a-b-c"));
        assert_eq!(host_name("-x-").as_deref(), Some("x"));
        assert_eq!(host_name("!!!"), None);
        assert_eq!(host_name(""), None);
        assert_eq!(host_name(&"a".repeat(80)).map(|n| n.len()), Some(63));
    }
}

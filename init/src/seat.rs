//! Who may use the screen and the input devices.
//!
//! The kernel creates every device node in `/dev` as `root`, mode 0600. The
//! display server doesn't run as root, so init hands these to a group,
//! mode 0660:
//!
//!   /dev/dri/card*, /dev/fb*      → `video`  (drawing on the screen)
//!   /dev/dri/renderD*             → `render` (drawing with the graphics
//!                                   chip, to nowhere anybody sees)
//!   /dev/input/event*, mice, …    → `input`  (keyboard, mouse, touchpad)
//!   /dev/snd/*                    → `audio`  (playing sound)
//!
//! and the accounts in those groups can open them: the session's,
//! `display`, in all of them, and the person's, whose apps draw and play,
//! in `render` and `audio`. Devices that
//! appear later — a USB mouse plugged in — are fixed as the kernel announces
//! them (a uevent), by a thread that does nothing else.

use std::path::Path;

use crate::account::{self, group_id};
use crate::log;

struct Rule {
    /// A path under `/dev` starting with this…
    prefix: &'static str,
    /// …belongs to this group.
    group: &'static str,
}

const RULES: &[Rule] = &[
    // Before the rest of `dri/`: the first rule that fits is the one.
    Rule {
        prefix: "dri/renderD",
        group: "render",
    },
    Rule {
        prefix: "dri/",
        group: "video",
    },
    Rule {
        prefix: "fb",
        group: "video",
    },
    Rule {
        prefix: "input/",
        group: "input",
    },
    Rule {
        prefix: "snd/",
        group: "audio",
    },
];

/// Fix every device there is now, and keep fixing the ones that come later.
pub fn start() {
    fix_all(Path::new("/dev"), "");

    if let Err(error) = std::thread::Builder::new().name("seat".into()).spawn(watch) {
        log::warn(&format!(
            "devices plugged in later won't be usable: {error}"
        ));
    }
}

/// Walk `/dev` (only the directories a rule can match) and fix what matches.
fn fix_all(dir: &Path, relative: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("{relative}{name}");
        let Ok(kind) = entry.file_type() else {
            continue;
        };

        if kind.is_dir() {
            let as_dir = format!("{relative}/");
            if RULES
                .iter()
                .any(|rule| rule.prefix.starts_with(&as_dir) || as_dir.starts_with(rule.prefix))
            {
                fix_all(&entry.path(), &as_dir);
            }
        } else if !kind.is_symlink() {
            fix(&relative);
        }
    }
}

/// Give `/dev/<relative>` to its group, if a rule names one.
fn fix(relative: &str) {
    let Some(rule) = RULES.iter().find(|rule| relative.starts_with(rule.prefix)) else {
        return;
    };
    let Some(gid) = group_id(rule.group) else {
        return;
    };

    let path = Path::new("/dev").join(relative);
    if let Err(error) = account::chown(&path, 0, gid).and_then(|_| account::chmod(&path, 0o660)) {
        log::warn(&format!(
            "could not hand {} to {}: {error}",
            path.display(),
            rule.group
        ));
    }
}

/// Listen to the kernel's device announcements, and fix each new device.
fn watch() {
    // Signals are for the main thread: its `waitpid` must be the one they
    // interrupt (see `power.rs`).
    // SAFETY: a full set, applied to this thread only.
    unsafe {
        let mut all: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut all);
        libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
    }

    let socket = match uevent_socket() {
        Ok(socket) => socket,
        Err(error) => {
            log::warn(&format!(
                "devices plugged in later won't be usable: {error}"
            ));
            return;
        }
    };

    let mut buffer = vec![0u8; 8192];
    loop {
        // SAFETY: the buffer is ours and its length is passed.
        let read = unsafe { libc::recv(socket, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
        if read <= 0 {
            continue;
        }

        if let Some(devname) = added_device(&buffer[..read as usize]) {
            fix(&devname);
        }
    }
}

/// A netlink socket that receives the kernel's uevents.
fn uevent_socket() -> std::io::Result<libc::c_int> {
    // SAFETY: plain syscalls; `address` lives across `bind`.
    unsafe {
        let socket = libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        );
        if socket < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut address: libc::sockaddr_nl = std::mem::zeroed();
        address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        address.nl_groups = 1; // the kernel's own announcements

        let bound = libc::bind(
            socket,
            (&address as *const libc::sockaddr_nl).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        );
        if bound != 0 {
            let error = std::io::Error::last_os_error();
            libc::close(socket);
            return Err(error);
        }

        Ok(socket)
    }
}

/// The `DEVNAME` of an `add` uevent, e.g. `input/event4`. A uevent is a
/// header (`add@/devices/…`) and `KEY=value` lines, all NUL-separated. The
/// node already exists when it arrives: devtmpfs makes it first.
fn added_device(message: &[u8]) -> Option<String> {
    let mut fields = message.split(|&b| b == 0).map(String::from_utf8_lossy);

    if !fields.next()?.starts_with("add@") {
        return None;
    }

    fields.find_map(|field| field.strip_prefix("DEVNAME=").map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_device_of_an_add_event() {
        let message = b"add@/devices/platform/i8042/serio1/input/input3/event3\0ACTION=add\0DEVNAME=input/event3\0SUBSYSTEM=input\0";
        assert_eq!(added_device(message).as_deref(), Some("input/event3"));
    }

    #[test]
    fn ignores_other_events() {
        let message = b"remove@/devices/x/event3\0ACTION=remove\0DEVNAME=input/event3\0";
        assert_eq!(added_device(message), None);
        assert_eq!(added_device(b"add@/devices/x\0ACTION=add\0"), None);
    }

    #[test]
    fn rules_match_by_prefix() {
        let group = |path: &str| {
            RULES
                .iter()
                .find(|r| path.starts_with(r.prefix))
                .map(|r| r.group)
        };
        assert_eq!(group("dri/card0"), Some("video"));
        assert_eq!(group("fb0"), Some("video"));
        assert_eq!(group("input/event2"), Some("input"));
        assert_eq!(group("snd/pcmC0D0p"), Some("audio"));
        assert_eq!(group("tty1"), None);
    }
}

//! Sleep: when asked, and when the lid closes.
//!
//! Suspending to memory is writing `mem` to `/sys/power/state`; the write
//! returns when the machine has woken again. The lid is a switch among the
//! kernel's input devices (the ACPI button driver's "Lid Switch").
//!
//! How it stands, in `power`:
//!
//!   sleeps N       how many times it has been put to sleep. The window
//!                  manager locks the screen when this goes up, so what
//!                  wakes is a locked screen
//!   lid open|closed|none

use std::fs::{self, File};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::sync::Mutex;
use std::time::Duration;

use crate::files;

const STATE: &str = "/sys/power/state";

/// How long the session has to see `sleeps` go up and draw its lock
/// screen before the machine stops.
const NOTICE: Duration = Duration::from_millis(700);

struct Power {
    sleeps: u64,
    lid: &'static str,
}

static POWER: Mutex<Power> = Mutex::new(Power {
    sleeps: 0,
    lid: "none",
});

fn say(power: &Power) {
    files::say(
        "power",
        &[
            format!("sleeps {}", power.sleeps),
            format!("lid {}", power.lid),
        ],
    );
}

/// Put the machine to sleep, and come back when it has woken. One at a
/// time: asked again while it is going down, the second is the first.
pub fn sleep() {
    let Ok(mut power) = POWER.try_lock() else {
        return;
    };
    power.sleeps += 1;
    say(&power);
    std::thread::sleep(NOTICE);

    // SAFETY: a plain system call.
    unsafe { libc::sync() };
    if let Err(error) = fs::write(STATE, "mem") {
        eprintln!("saule-system: could not sleep: {error}");
    }
}

/// Watch the lid, in a thread: closing it is asking for sleep.
pub fn watch_lid() {
    if let Ok(power) = POWER.lock() {
        say(&power);
    }
    std::thread::spawn(|| {
        // The lid's device is there from the start; a moment is allowed
        // for it all the same.
        let lid = (0..10).find_map(|attempt| {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(1));
            }
            lid_switch()
        });
        let Some(mut lid) = lid else {
            return;
        };
        set_lid("open");

        // One `struct input_event` on 64-bit Linux: a `timeval`, then
        // type, code and value.
        let mut event = [0u8; 24];
        while lid.read_exact(&mut event).is_ok() {
            let kind = u16::from_ne_bytes([event[16], event[17]]);
            let code = u16::from_ne_bytes([event[18], event[19]]);
            let value = i32::from_ne_bytes([event[20], event[21], event[22], event[23]]);
            if kind == EV_SW && code == SW_LID {
                if value != 0 {
                    set_lid("closed");
                    sleep();
                } else {
                    set_lid("open");
                }
            }
        }
    });
}

fn set_lid(lid: &'static str) {
    if let Ok(mut power) = POWER.lock() {
        power.lid = lid;
        say(&power);
    }
}

const EV_SW: u16 = 0x05;
const SW_LID: u16 = 0x00;

/// `EVIOCGBIT(ev, len)`: which codes of event type `ev` a device has.
fn eviocgbit(ev: u16, len: usize) -> libc::Ioctl {
    ((2u64 << 30) | ((len as u64) << 16) | (0x45 << 8) | (0x20 + u64::from(ev))) as libc::Ioctl
}

/// The input device that has a lid switch, opened to read.
fn lid_switch() -> Option<File> {
    let entries = fs::read_dir("/dev/input").ok()?;
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("event") {
            continue;
        }
        let Ok(file) = File::open(entry.path()) else {
            continue;
        };
        let mut switches = [0u8; 8];
        // SAFETY: the request writes at most the length encoded in it into
        // the array passed, which is that long.
        let got = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                eviocgbit(EV_SW, switches.len()),
                switches.as_mut_ptr(),
            )
        };
        if got >= 0 && switches[0] & (1 << SW_LID) != 0 {
            return Some(file);
        }
    }
    None
}

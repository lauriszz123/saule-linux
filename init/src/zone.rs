//! A time zone's distance from UTC, from the zone files in the image.
//!
//! The C library knows how to read `/usr/share/zoneinfo`, but only for the
//! zone named in the `TZ` environment variable, and init has threads: one
//! of them changing the environment under the others is not safe. So the
//! question is asked of a second copy of init, started for the purpose
//! (`/init zone NAME SECONDS`), which sets `TZ`, answers on its standard
//! output and ends. [`answer`] is that copy; [`ask`] starts it.

use std::process::{Command, Stdio};

/// What a zone is at some instant: how far ahead of UTC, in seconds, and
/// when that next changes (summer time starting or ending) and to what.
/// `next_at` is 0 for a zone that doesn't change in the year ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Offset {
    pub seconds: i64,
    pub next_at: i64,
    pub next_seconds: i64,
}

/// Where the zone files are.
const ZONES: &str = "/usr/share/zoneinfo";

/// Whether `name` is a zone this image has: `Europe/Vilnius`, `UTC`.
/// Letters, digits, and `_ + - /` only, and nothing that climbs out of the
/// folder: the name comes from a file the session's account can write.
pub fn known(name: &str) -> bool {
    let plain = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'/'))
        && !name.starts_with('/')
        && !name.contains("//")
        && !name.split('/').any(|part| part.is_empty() || part == "..");
    plain && std::path::Path::new(ZONES).join(name).is_file()
}

/// What zone `name` is at `at` (Unix seconds); `None` when it can't be
/// found out.
pub fn ask(name: &str, at: i64) -> Option<Offset> {
    if !known(name) {
        return None;
    }
    let output = Command::new("/init")
        .args(["zone", name, &at.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse(&String::from_utf8_lossy(&output.stdout))
}

fn parse(text: &str) -> Option<Offset> {
    let mut numbers = text.split_whitespace().map(|word| word.parse::<i64>());
    Some(Offset {
        seconds: numbers.next()?.ok()?,
        next_at: numbers.next()?.ok()?,
        next_seconds: numbers.next()?.ok()?,
    })
}

/// The second copy of init: print zone `name` at `at` and end. Called
/// from `main` before anything else, when init is not the first process.
pub fn answer(name: &str, at: &str) -> ! {
    let Ok(at) = at.parse::<i64>() else {
        std::process::exit(2);
    };
    // SAFETY: this process has one thread and does nothing but this.
    unsafe {
        std::env::set_var("TZ", name);
    }

    let offset = work_out(at, offset_at);
    println!(
        "{} {} {}",
        offset.seconds, offset.next_at, offset.next_seconds
    );
    std::process::exit(0);
}

/// The zone in `TZ` at `instant`, in seconds ahead of UTC. The C library
/// reads `TZ`, and the zone's file, as it is asked.
// `time_t`: the libc crate warns that musl's will change width one day; on
// this machine's 64 bits it is an `i64` before and after.
#[allow(deprecated)]
fn offset_at(instant: i64) -> i64 {
    let time: libc::time_t = instant;
    // SAFETY: `tm` is filled in by the call, from a valid time.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&time, &mut tm);
        tm
    };
    tm.tm_gmtoff
}

/// How far to look ahead for a change, and in what steps: summer time
/// changes at most twice a year, and never for less than a day.
const AHEAD: i64 = 400 * 86_400;
const STEP: i64 = 6 * 3_600;

/// The offset at `at`, and the next change after it, given what the
/// offset is at any instant: stepped forward to the first step where it
/// differs, then halved down to the second it changes.
fn work_out(at: i64, offset_at: impl Fn(i64) -> i64) -> Offset {
    let seconds = offset_at(at);

    let mut before = at;
    while before - at < AHEAD {
        let after = before + STEP;
        if offset_at(after) != seconds {
            let (mut low, mut high) = (before, after);
            while high - low > 1 {
                let middle = low + (high - low) / 2;
                if offset_at(middle) == seconds {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            return Offset {
                seconds,
                next_at: high,
                next_seconds: offset_at(high),
            };
        }
        before = after;
    }

    Offset {
        seconds,
        next_at: 0,
        next_seconds: seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zone_that_never_changes_has_no_next() {
        let offset = work_out(1_000_000, |_| 3_600);
        assert_eq!(
            offset,
            Offset {
                seconds: 3_600,
                next_at: 0,
                next_seconds: 3_600
            }
        );
    }

    #[test]
    fn the_next_change_is_found_to_the_second() {
        // Two hours ahead until the instant 5,000,123, three from then on.
        let change = 5_000_123;
        let zone = |instant: i64| if instant < change { 7_200 } else { 10_800 };

        let offset = work_out(1_000_000, zone);
        assert_eq!(offset.seconds, 7_200);
        assert_eq!(offset.next_at, change);
        assert_eq!(offset.next_seconds, 10_800);

        // Asked at the change itself, it has happened.
        assert_eq!(work_out(change, zone).seconds, 10_800);
    }

    #[test]
    fn what_the_second_copy_prints_is_read_back() {
        assert_eq!(
            parse("7200 1774141200 10800\n"),
            Some(Offset {
                seconds: 7_200,
                next_at: 1_774_141_200,
                next_seconds: 10_800
            })
        );
        assert_eq!(parse(""), None);
        assert_eq!(parse("7200 x 1"), None);
    }

    #[test]
    fn a_name_that_is_not_a_zone_is_not_known() {
        for name in [
            "",
            "/etc/passwd",
            "../etc/passwd",
            "Europe/../../etc/passwd",
            "a b",
            "Europe//Vilnius",
        ] {
            assert!(!known(name), "{name}");
        }
    }
}

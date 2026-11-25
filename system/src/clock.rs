//! The clock, set from the network.
//!
//! A PC keeps time with a small clock of its own, which drifts, and which
//! a machine that had Windows on it keeps in local time (see init's
//! `system.rs`). Once the machine is on a network the time is asked of a
//! time server instead (SNTP, RFC 4330: one small packet each way), the
//! system's clock is set by the answer, and so is the PC's own clock, in
//! UTC, so that the next start is right before any network is.
//!
//! Init is told by a note in the settings (`/data/system/clock` becomes
//! `synced`): the clock is right as it stands, and what the PC's clock
//! says from now on is UTC.
//!
//! How it stands, in `clock`:
//!
//!   synced UNIXTIME|never      when the time was last set from a server
//!
//! Asked again every six hours, and at once on `clock sync`.

use std::fs;
use std::net::{ToSocketAddrs, UdpSocket};
use std::os::fd::AsRawFd;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use crate::files;

const SERVERS: [&str; 3] = [
    "pool.ntp.org:123",
    "time.cloudflare.com:123",
    "time.google.com:123",
];

/// Seconds from 1900, where a time server counts from, to 1970.
const TO_UNIX: u64 = 2_208_988_800;

/// The earliest a server can be telling the truth: when this was written.
const EARLIEST: u64 = 1_750_000_000;

const AGAIN: Duration = Duration::from_secs(6 * 3600);
const RETRY: Duration = Duration::from_secs(20);

const NOTE: &str = "/data/system/clock";

pub struct Clock {
    now: Sender<()>,
}

pub fn start() -> Clock {
    files::say("clock", &["synced never".to_string()]);
    let (now, asked) = mpsc::channel::<()>();
    std::thread::spawn(move || loop {
        let wait = match SERVERS.iter().find_map(|server| ask(server)) {
            Some(seconds) => {
                set(seconds);
                AGAIN
            }
            // No network yet, or no answer: soon again.
            None => RETRY,
        };
        let _ = asked.recv_timeout(wait);
    });
    Clock { now }
}

impl Clock {
    pub fn ask(&self, rest: &str) {
        if rest.trim() == "sync" {
            let _ = self.now.send(());
        }
    }
}

/// What a server says the time is, in Unix seconds, with half the time
/// the question and answer took added on.
fn ask(server: &str) -> Option<u64> {
    let address = server.to_socket_addrs().ok()?.find(|a| a.is_ipv4())?;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let sent = Instant::now();
    socket.send_to(&question(), address).ok()?;
    let mut answer = [0u8; 48];
    let (length, _) = socket.recv_from(&mut answer).ok()?;
    let seconds = read(&answer[..length])?;
    let travelled = sent.elapsed().as_millis() as u64 / 2;
    Some(seconds + (travelled + 500) / 1000)
}

/// A client's question: version 4, mode 3, and nothing else.
fn question() -> [u8; 48] {
    let mut packet = [0u8; 48];
    packet[0] = 0x23;
    packet
}

/// The time in a server's answer; `None` for one that isn't an answer,
/// or says it doesn't know the time (stratum 0), or says a time before
/// this was written.
fn read(answer: &[u8]) -> Option<u64> {
    if answer.len() < 48 {
        return None;
    }
    let mode = answer[0] & 0x07;
    let stratum = answer[1];
    if mode != 4 || stratum == 0 {
        return None;
    }
    let since_1900 = u64::from(u32::from_be_bytes([
        answer[40], answer[41], answer[42], answer[43],
    ]));
    let fraction = u32::from_be_bytes([answer[44], answer[45], answer[46], answer[47]]);
    let rounded = since_1900 + u64::from(fraction >> 31);
    let unix = rounded.checked_sub(TO_UNIX)?;
    (unix >= EARLIEST).then_some(unix)
}

fn set(seconds: u64) {
    let time = libc::timespec {
        tv_sec: seconds as libc::time_t,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a valid timespec for the length of the call.
    if unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &time) } != 0 {
        eprintln!("saule-system: could not set the clock");
        return;
    }
    // The PC's own clock in UTC, and the note that says so: without the
    // clock set, the note would have the next start read local time as
    // UTC.
    if set_rtc(seconds) && fs::metadata("/data/system").is_ok() {
        let _ = fs::write(format!("{NOTE}.part"), "synced\n")
            .and_then(|()| fs::rename(format!("{NOTE}.part"), NOTE));
    }
    files::say("clock", &[format!("synced {seconds}")]);
}

/// `struct rtc_time`: a `struct tm`'s nine numbers.
#[repr(C)]
#[derive(Debug, Default, PartialEq, Eq)]
struct RtcTime {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
}

/// `RTC_SET_TIME`: `_IOW('p', 0x0a, struct rtc_time)`.
const RTC_SET_TIME: libc::Ioctl = 0x4024_700a_u32 as libc::Ioctl;

fn set_rtc(seconds: u64) -> bool {
    let Ok(clock) = fs::OpenOptions::new().write(true).open("/dev/rtc0") else {
        return false;
    };
    let time = broken_down(seconds);
    // SAFETY: the request reads one `struct rtc_time`, which `time` is.
    unsafe { libc::ioctl(clock.as_raw_fd(), RTC_SET_TIME, &time) == 0 }
}

/// Unix seconds as a date and time in UTC, the way the PC's clock is set:
/// the month from 0, the year from 1900.
fn broken_down(seconds: u64) -> RtcTime {
    let days = (seconds / 86_400) as i64;
    let rest = (seconds % 86_400) as i32;

    // Days since 1970 to a civil date (Howard Hinnant's algorithm): years
    // are taken to start in March, so the leap day is the last of one.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    RtcTime {
        sec: rest % 60,
        min: rest / 60 % 60,
        hour: rest / 3_600,
        mday: day as i32,
        mon: month as i32 - 1,
        year: year as i32 - 1900,
        wday: ((days + 4).rem_euclid(7)) as i32,
        yday: 0,
        isdst: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(mode: u8, stratum: u8, since_1900: u32, fraction: u32) -> [u8; 48] {
        let mut packet = [0u8; 48];
        packet[0] = 0x20 | mode;
        packet[1] = stratum;
        packet[40..44].copy_from_slice(&since_1900.to_be_bytes());
        packet[44..48].copy_from_slice(&fraction.to_be_bytes());
        packet
    }

    #[test]
    fn a_server_s_answer_is_the_time_in_unix_seconds() {
        // 2026-11-25 12:00:00 UTC.
        let unix = 1_795_608_000u64;
        let sent = (unix + TO_UNIX) as u32;
        assert_eq!(read(&answer(4, 2, sent, 0)), Some(unix));
        // More than half a second past is the next second.
        assert_eq!(read(&answer(4, 2, sent, 0x8000_0001)), Some(unix + 1));
    }

    #[test]
    fn what_is_not_an_answer_is_not_the_time() {
        let sent = (1_795_608_000u64 + TO_UNIX) as u32;
        assert_eq!(read(&answer(3, 2, sent, 0)), None);
        assert_eq!(read(&answer(4, 0, sent, 0)), None);
        assert_eq!(read(&answer(4, 2, 0, 0)), None);
        assert_eq!(read(&[0u8; 20]), None);
        assert_eq!(question()[0], 0x23);
    }

    #[test]
    fn seconds_become_the_date_the_pc_s_clock_is_set_to() {
        // 2026-11-25 12:34:56 UTC, a Wednesday.
        let time = broken_down(1_795_610_096);
        assert_eq!((time.year + 1900, time.mon + 1, time.mday), (2026, 11, 25));
        assert_eq!((time.hour, time.min, time.sec), (12, 34, 56));
        assert_eq!(time.wday, 3);
        // The last day of a leap February, and the first of 1970.
        let leap = broken_down(1_709_164_800);
        assert_eq!((leap.year + 1900, leap.mon + 1, leap.mday), (2024, 2, 29));
        let first = broken_down(0);
        assert_eq!(
            (first.year + 1900, first.mon + 1, first.mday, first.wday),
            (1970, 1, 1, 4)
        );
    }
}

//! The programs that run beside the session for as long as the machine
//! does, as root: what the session may ask for but not do itself.
//!
//! One so far: `saule-system` (../system), which puts the machine to sleep,
//! sets the volume and brings up the network. Init starts it and, since a
//! machine that can no longer be put to sleep or heard is a broken one,
//! starts it again if it ever ends.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::log;

const SYSTEM: &str = "/sbin/saule-system";

/// Start them, and keep them running. `account` is the session's: the one
/// they take requests from. `person` is the person's: whose the drives
/// plugged in are.
pub fn start(account: &str, person: &str) {
    if !Path::new(SYSTEM).exists() {
        return;
    }
    let account = account.to_string();
    let person = person.to_string();
    std::thread::spawn(move || loop {
        let child = Command::new(SYSTEM)
            .arg(&account)
            .arg(&person)
            .stdin(Stdio::null())
            .spawn();
        let pid = match child {
            Ok(child) => child.id(),
            Err(error) => {
                log::warn(&format!("could not start {SYSTEM}: {error}"));
                return;
            }
        };

        // Not `Child::wait`: init's own loop reaps every process that
        // ends, this one too. That it has ended is seen by its entry in
        // /proc going.
        let alive = format!("/proc/{pid}");
        while Path::new(&alive).exists() {
            std::thread::sleep(Duration::from_secs(2));
        }
        log::warn(&format!("{SYSTEM} ended; starting it again"));
        std::thread::sleep(Duration::from_secs(1));
    });
}

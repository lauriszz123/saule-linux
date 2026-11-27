//! Messages from init, on every console (see `console::broadcast`), and
//! kept in [`KEPT`] for a report: a machine started with `quiet` shows
//! none of them, and whether the apps were found is said nowhere else.

use std::io::Write;

use crate::console;

/// What init has said since the machine started, a line each.
pub const KEPT: &str = "/run/saule/init.log";

const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

pub fn info(message: &str) {
    console::broadcast(&format!("{CYAN}[init]{RESET} {message}\n"));
    keep(message);
}

pub fn warn(message: &str) {
    console::broadcast(&format!("{YELLOW}[init]{RESET} {message}\n"));
    keep(&format!("warning: {message}"));
}

/// Add `message` to [`KEPT`], with how long the machine had been on.
/// Nothing is said if it can't be: before `/run` is there, it can't.
fn keep(message: &str) {
    let up = std::fs::read_to_string("/proc/uptime").unwrap_or_default();
    let up = up.split_whitespace().next().unwrap_or("?");
    let _ = std::fs::create_dir_all("/run/saule");
    if let Ok(mut kept) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(KEPT)
    {
        let _ = writeln!(kept, "[{up:>8}] {message}");
    }
}

/// `/etc/motd`, or a plain title when the image has none.
pub fn banner() {
    let text = std::fs::read_to_string("/etc/motd").unwrap_or_else(|_| "Saule Linux\n".into());

    console::broadcast(&format!("\n{BOLD}{text}{RESET}\n"));
}

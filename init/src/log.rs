//! Messages from init, on every console (see `console::broadcast`).

use crate::console;

const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

pub fn info(message: &str) {
    console::broadcast(&format!("{CYAN}[init]{RESET} {message}\n"));
}

pub fn warn(message: &str) {
    console::broadcast(&format!("{YELLOW}[init]{RESET} {message}\n"));
}

/// `/etc/motd`, or a plain title when the image has none.
pub fn banner() {
    let text = std::fs::read_to_string("/etc/motd").unwrap_or_else(|_| "Saule Linux\n".into());

    console::broadcast(&format!("\n{BOLD}{text}{RESET}\n"));
}

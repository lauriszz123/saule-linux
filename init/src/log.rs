//! Messages from init, on the console.

const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

pub fn info(message: &str) {
    eprintln!("{CYAN}[init]{RESET} {message}");
}

pub fn warn(message: &str) {
    eprintln!("{YELLOW}[init]{RESET} {message}");
}

/// `/etc/motd`, or a plain title when the image has none.
pub fn banner() {
    let text = std::fs::read_to_string("/etc/motd").unwrap_or_else(|_| "Saule Linux\n".into());

    eprintln!();
    eprint!("{BOLD}{text}{RESET}");
    eprintln!();
}

//! Bluetooth: the radio, the devices near it, and the ones paired.
//!
//! BlueZ does Bluetooth on Linux: its daemon, `bluetoothd`, finds, pairs
//! and connects, with the kernel's half under it. It is spoken to over
//! D-Bus, so a D-Bus daemon runs for it, on a bus only root may use.
//! Both are started here, when the machine has a radio and the image has
//! them (`scripts/fetch-bluetooth.sh`), and asked through BlueZ's own
//! command-line client, `bluetoothctl`, one command at a time.
//!
//! What was paired is BlueZ's to keep (`/var/lib/bluetooth`), which is
//! made a folder on the data disk, so that a mouse paired once is there
//! after a restart.
//!
//! How it stands, in `bluetooth`, tabs between a line's fields:
//!
//!   radio none|off|on
//!   scanning 0|1
//!   passkey  ADDRESS  DIGITS     a keyboard being paired: these are to
//!                                be typed on it, and Enter pressed
//!   device   ADDRESS  PAIRED 0|1  CONNECTED 0|1  NAME
//!
//! The requests:
//!
//!   bluetooth on|off
//!   bluetooth scan                    look for devices for a while
//!   bluetooth pair ADDRESS            pair, trust and connect
//!   bluetooth connect|disconnect|forget ADDRESS

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::files;

const CLIENT: &str = "/usr/bin/bluetoothctl";
const DAEMON: &str = "/usr/sbin/bluetoothd";
const BUS: &str = "/usr/bin/dbus-daemon";
const BUS_SOCKET: &str = "/run/dbus/system_bus_socket";
const KEPT: &str = "/data/bluetooth";
const STATE: &str = "/var/lib/bluetooth";

/// How long a search for devices goes on, and how long pairing may take
/// (somebody has to type digits on a keyboard).
const SEARCH: Duration = Duration::from_secs(15);
const PAIRING: u64 = 60;

#[derive(Default)]
struct Stands {
    bus: Option<Child>,
    daemon: Option<Child>,
    scanning_until: Option<Instant>,
    /// The device being paired, and the digits to type on it, if any.
    passkey: Option<(String, String)>,
}

#[derive(Clone)]
pub struct Bluetooth(Arc<Mutex<Stands>>);

pub fn start() -> Bluetooth {
    let bluetooth = Bluetooth(Arc::new(Mutex::new(Stands::default())));
    files::say("bluetooth", &["radio none".to_string()]);
    if !Path::new(CLIENT).exists() || !Path::new(DAEMON).exists() {
        return bluetooth;
    }

    let watching = bluetooth.clone();
    std::thread::spawn(move || loop {
        watching.look();
        std::thread::sleep(Duration::from_secs(3));
    });
    bluetooth
}

impl Bluetooth {
    pub fn ask(&self, rest: &str) {
        let (word, address) = rest.split_once(' ').unwrap_or((rest, ""));
        let address = address.trim();
        match word.trim() {
            "on" => run(&["power", "on"]),
            "off" => run(&["power", "off"]),
            "scan" => self.scan(),
            "pair" if is_address(address) => self.pair(address),
            "connect" if is_address(address) => run(&["connect", address]),
            "disconnect" if is_address(address) => run(&["disconnect", address]),
            "forget" if is_address(address) => run(&["remove", address]),
            _ => return,
        }
        self.say();
    }

    /// Look for devices for a while, in a client of its own that ends
    /// the search when it ends.
    fn scan(&self) {
        let seconds = SEARCH.as_secs().to_string();
        let started = Command::new(CLIENT)
            .args(["--timeout", &seconds, "scan", "on"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let (Ok(mut child), Ok(mut stands)) = (started, self.0.lock()) {
            stands.scanning_until = Some(Instant::now() + SEARCH);
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    /// Pair with `address`, in a thread: it takes as long as somebody
    /// takes to type. Then trust it (it may connect by itself from now
    /// on) and connect.
    fn pair(&self, address: &str) {
        let address = address.to_string();
        let this = self.clone();
        std::thread::spawn(move || {
            let timeout = PAIRING.to_string();
            let started = Command::new(CLIENT)
                .args([
                    "--agent",
                    "KeyboardDisplay",
                    "--timeout",
                    &timeout,
                    "pair",
                    &address,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let Ok(mut child) = started else {
                return;
            };
            let (Some(mut output), Some(mut input)) = (child.stdout.take(), child.stdin.take())
            else {
                return;
            };
            // What the client prints is read as it comes: its questions
            // end in no new line.
            let mut said = String::new();
            let mut buffer = [0u8; 512];
            while let Ok(length) = output.read(&mut buffer) {
                if length == 0 {
                    break;
                }
                said.push_str(&String::from_utf8_lossy(&buffer[..length]));
                match asked(&said) {
                    Some(Asked::Show(digits)) => {
                        if let Ok(mut stands) = this.0.lock() {
                            stands.passkey = Some((address.clone(), digits));
                        }
                        this.say();
                        said.clear();
                    }
                    // The same digits on both: nobody to compare them
                    // here but the person who asked to pair.
                    Some(Asked::Confirm) => {
                        let _ = input.write_all(b"yes\n");
                        said.clear();
                    }
                    // An old device with a fixed code.
                    Some(Asked::Pin) => {
                        let _ = input.write_all(b"0000\n");
                        said.clear();
                    }
                    None => {}
                }
            }
            let _ = child.wait();
            if let Ok(mut stands) = this.0.lock() {
                stands.passkey = None;
            }
            run(&["trust", &address]);
            run(&["connect", &address]);
            this.say();
        });
    }

    /// Start what is not running, where there is a radio to run it for.
    fn look(&self) {
        let radio = fs::read_dir("/sys/class/bluetooth").is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("hci"))
        });
        if !radio {
            files::say("bluetooth", &["radio none".to_string()]);
            return;
        }
        if let Ok(mut stands) = self.0.lock() {
            keep_running(&mut stands);
        }
        self.say();
    }

    fn say(&self) {
        let powered = output(&["show"]).is_some_and(|show| value(&show, "Powered") == "yes");
        let mut lines = vec![format!("radio {}", if powered { "on" } else { "off" })];
        if let Ok(mut stands) = self.0.lock() {
            let scanning = stands
                .scanning_until
                .is_some_and(|until| Instant::now() < until);
            if !scanning {
                stands.scanning_until = None;
            }
            lines.push(format!("scanning {}", u8::from(scanning)));
            if let Some((address, digits)) = &stands.passkey {
                lines.push(format!("passkey\t{address}\t{digits}"));
            }
        }
        if powered {
            for (address, name) in devices(&output(&["devices"]).unwrap_or_default()) {
                let info = output(&["info", &address]).unwrap_or_default();
                lines.push(format!(
                    "device\t{address}\t{}\t{}\t{name}",
                    u8::from(value(&info, "Paired") == "yes"),
                    u8::from(value(&info, "Connected") == "yes"),
                ));
            }
        }
        files::say("bluetooth", &lines);
    }
}

/// The bus and the daemon, started if they aren't running.
fn keep_running(stands: &mut Stands) {
    let alive = |child: &mut Option<Child>| {
        child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    };

    if !alive(&mut stands.bus) {
        let _ = fs::create_dir_all("/run/dbus");
        let _ = fs::remove_file(BUS_SOCKET);
        // The bus wants the machine to have a number of its own.
        if !Path::new("/etc/machine-id").exists() {
            let _ = fs::write("/etc/machine-id", machine_id());
        }
        stands.bus = Command::new(BUS)
            .args(["--system", "--nofork", "--nopidfile"])
            .stdin(Stdio::null())
            .spawn()
            .ok();
        stands.daemon = None;
        // The daemon is started on the next look, once the bus is up.
        return;
    }
    if !alive(&mut stands.daemon) && Path::new(BUS_SOCKET).exists() {
        keep_pairings();
        stands.daemon = Command::new(DAEMON)
            .arg("--nodetach")
            .stdin(Stdio::null())
            .spawn()
            .ok();
    }
}

/// Where BlueZ keeps what was paired: on the data disk when there is
/// one, for root alone, since it holds the keys devices are trusted by.
fn keep_pairings() {
    if Path::new(STATE).exists() || fs::symlink_metadata(STATE).is_ok() {
        return;
    }
    let _ = fs::create_dir_all("/var/lib");
    if Path::new("/data/system").is_dir() && fs::create_dir_all(KEPT).is_ok() {
        let _ = fs::set_permissions(KEPT, fs::Permissions::from_mode(0o700));
        let _ = std::os::unix::fs::symlink(KEPT, STATE);
    } else {
        let _ = fs::create_dir_all(STATE);
    }
}

/// 32 hexadecimal digits, from the kernel's randomness.
fn machine_id() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut random) = fs::File::open("/dev/urandom") {
        let _ = random.read_exact(&mut bytes);
    }
    let mut id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    id.push('\n');
    id
}

/// Run one of the client's commands and wait for it.
fn run(arguments: &[&str]) {
    let _ = output(arguments);
}

/// What one of the client's commands prints; `None` when it could not be
/// run or took too long (the client waits for a daemon that isn't there).
fn output(arguments: &[&str]) -> Option<String> {
    let mut all = vec!["--timeout", "8"];
    all.extend_from_slice(arguments);
    let output = Command::new(CLIENT)
        .args(all)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some(strip_colours(&String::from_utf8_lossy(&output.stdout)))
}

/// The client colours what it prints: without the escape sequences.
fn strip_colours(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Up to the letter that ends the sequence.
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if c != '\u{1}' && c != '\u{2}' {
            out.push(c);
        }
    }
    out
}

/// `\tPowered: yes` in what `show` or `info` prints: the `yes`.
fn value(text: &str, name: &str) -> String {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(name)?.strip_prefix(':'))
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// `Device AA:BB:CC:DD:EE:FF Name of it`, a line each: the address and
/// the name.
fn devices(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("Device ")?;
            let (address, name) = rest.split_once(' ').unwrap_or((rest, ""));
            is_address(address).then(|| (address.to_string(), name.trim().replace('\t', " ")))
        })
        .collect()
}

/// Whether `text` is a device's address: six pairs of hexadecimal digits
/// with colons between. Checked before it is handed to a command.
fn is_address(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_hexdigit()))
}

/// What the client is asking, while pairing.
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    /// Digits to type on the device being paired.
    Show(String),
    /// Whether the digits both show are the same.
    Confirm,
    /// A code for an old device.
    Pin,
}

fn asked(said: &str) -> Option<Asked> {
    let said = strip_colours(said);
    if said.contains("Confirm passkey") || said.contains("(yes/no)") {
        return Some(Asked::Confirm);
    }
    if said.contains("Enter PIN code") || said.contains("Enter passkey") {
        return Some(Asked::Pin);
    }
    for marker in ["Passkey: ", "PIN code: "] {
        if let Some(at) = said.rfind(marker) {
            let digits: String = said[at + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            // The whole number, once the line it is on has ended.
            if !digits.is_empty() && said[at..].contains('\n') {
                return Some(Asked::Show(digits));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_client_prints_is_read_without_its_colours() {
        let show = "Controller 00:1A:7D:DA:71:13 (public)\n\tName: saule\n\t\u{1b}[0;94mPowered: yes\u{1b}[0m\n\tDiscoverable: no\n";
        let plain = strip_colours(show);
        assert_eq!(value(&plain, "Powered"), "yes");
        assert_eq!(value(&plain, "Name"), "saule");
        assert_eq!(value(&plain, "Pairable"), "");
    }

    #[test]
    fn devices_are_an_address_and_a_name() {
        let listed = "Device AA:BB:CC:DD:EE:FF MX Master 3\nDevice 11:22:33:44:55:66 11-22-33-44-55-66\nNo default controller available\n";
        assert_eq!(
            devices(listed),
            [
                ("AA:BB:CC:DD:EE:FF".to_string(), "MX Master 3".to_string()),
                (
                    "11:22:33:44:55:66".to_string(),
                    "11-22-33-44-55-66".to_string()
                ),
            ]
        );
    }

    #[test]
    fn only_an_address_is_handed_to_a_command() {
        assert!(is_address("AA:BB:CC:dd:ee:0f"));
        assert!(!is_address("AA:BB:CC:DD:EE"));
        assert!(!is_address("AA:BB:CC:DD:EE:FG"));
        assert!(!is_address("AA:BB:CC:DD:EE:FF; reboot"));
        assert!(!is_address(""));
    }

    #[test]
    fn pairing_s_questions_are_told_apart() {
        assert_eq!(
            asked("[agent] Passkey: 123456\n"),
            Some(Asked::Show("123456".into()))
        );
        // Not until the line is whole.
        assert_eq!(asked("[agent] Passkey: 12"), None);
        assert_eq!(
            asked("\u{1b}[0;94m[agent]\u{1b}[0m Confirm passkey 654321 (yes/no): "),
            Some(Asked::Confirm)
        );
        assert_eq!(asked("[agent] Enter PIN code: "), Some(Asked::Pin));
        assert_eq!(asked("Attempting to pair with AA:BB:CC:DD:EE:FF\n"), None);
    }

    #[test]
    fn a_machine_s_number_is_thirty_two_digits() {
        let id = machine_id();
        assert_eq!(id.trim().len(), 32);
        assert!(id.trim().chars().all(|c| c.is_ascii_hexdigit()));
    }
}

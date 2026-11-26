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
const RADIO_SWITCHES: &str = "/sys/class/rfkill";
/// Whether the radio was last turned on or off, kept on the data disk, and
/// the daemon's settings, where it is told which to start as.
const CHOICE: &str = "/data/system/bluetooth";
const SETTINGS: &str = "/etc/bluetooth/main.conf";

/// How long a search for devices goes on, and how long pairing may take
/// (somebody has to type digits on a keyboard).
const SEARCH: Duration = Duration::from_secs(15);
const PAIRING: u64 = 60;
/// How long the client may take to answer, and to connect a device.
const ANSWER: Duration = Duration::from_secs(8);
/// How long the bus is given to open its socket, once started.
const BUS_STARTS: Duration = Duration::from_secs(2);
const CONNECTING: Duration = Duration::from_secs(30);

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
            "on" => {
                remember(Path::new(CHOICE), true);
                unblock(Path::new(RADIO_SWITCHES));
                run(&["power", "on"]);
            }
            "off" => {
                remember(Path::new(CHOICE), false);
                run(&["power", "off"]);
            }
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
    ///
    /// The client is talked to as a person at a terminal would: started
    /// with nothing to do, told to pair once it says its agent is there,
    /// answered, and told to leave. Given `pair` as a command to run, it
    /// has no agent, and a keyboard, which has digits to be typed on it,
    /// can't be paired ("No agent available").
    fn pair(&self, address: &str) {
        let address = address.to_string();
        let this = self.clone();
        std::thread::spawn(move || {
            let started = Command::new(CLIENT)
                .args(["--agent", "KeyboardDisplay"])
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
            // Somebody who never types the digits, or a client that
            // never answers: it is ended when the time is up, or a moment
            // after it was told to leave.
            let (done, is_done) = std::sync::mpsc::channel::<()>();
            let ending = std::thread::spawn(move || {
                let grace = match is_done.recv_timeout(Duration::from_secs(PAIRING)) {
                    Ok(()) => Duration::from_secs(2),
                    Err(_) => Duration::ZERO,
                };
                let until = Instant::now() + grace;
                while matches!(child.try_wait(), Ok(None)) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let _ = child.kill();
                let _ = child.wait();
            });

            // What the client prints is read as it comes: its questions
            // end in no new line.
            let mut said = String::new();
            let mut buffer = [0u8; 512];
            let mut asked_to_pair = false;
            while let Ok(length) = output.read(&mut buffer) {
                if length == 0 {
                    break;
                }
                said.push_str(&String::from_utf8_lossy(&buffer[..length]));
                match heard(&said, asked_to_pair) {
                    Some(Heard::Ready) => {
                        let _ = input.write_all(format!("pair {address}\n").as_bytes());
                        asked_to_pair = true;
                        said.clear();
                    }
                    Some(Heard::Asked(Asked::Show(digits))) => {
                        if let Ok(mut stands) = this.0.lock() {
                            stands.passkey = Some((address.clone(), digits));
                        }
                        this.say();
                        said.clear();
                    }
                    // The same digits on both: nobody to compare them
                    // here but the person who asked to pair.
                    Some(Heard::Asked(Asked::Confirm)) => {
                        let _ = input.write_all(b"yes\n");
                        said.clear();
                    }
                    // An old device with a fixed code.
                    Some(Heard::Asked(Asked::Pin)) => {
                        let _ = input.write_all(b"0000\n");
                        said.clear();
                    }
                    Some(Heard::Over) => break,
                    None => {}
                }
            }
            let _ = input.write_all(b"quit\n");
            let _ = done.send(());
            let _ = ending.join();
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
        let running = self
            .0
            .lock()
            .is_ok_and(|mut stands| keep_running(&mut stands));
        if running {
            self.say();
        } else {
            // Nobody to ask yet: asking would wait for the daemon.
            files::say("bluetooth", &["radio off".to_string()]);
        }
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

/// The bus and the daemon, started if they aren't running. Whether the
/// daemon is, after that.
fn keep_running(stands: &mut Stands) -> bool {
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
        // The bus is up when its socket is there: a moment, waited for
        // here so the daemon starts now and not at the next look.
        let until = Instant::now() + BUS_STARTS;
        while !Path::new(BUS_SOCKET).exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if !alive(&mut stands.daemon) && Path::new(BUS_SOCKET).exists() {
        keep_pairings();
        let _ = fs::create_dir_all("/etc/bluetooth");
        let _ = fs::write(SETTINGS, settings(wanted_on(Path::new(CHOICE))));
        stands.daemon = Command::new(DAEMON)
            .arg("--nodetach")
            .stdin(Stdio::null())
            .spawn()
            .ok();
    }
    alive(&mut stands.daemon)
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

/// Keep whether the radio was turned on or off, in `choice`: a radio
/// turned off stays off when the machine is next started.
fn remember(choice: &Path, on: bool) {
    let _ = fs::write(choice, if on { "on\n" } else { "off\n" });
}

/// Whether the radio is to be on as the daemon starts: unless it was last
/// turned off. On, where nothing was ever chosen or kept.
fn wanted_on(choice: &Path) -> bool {
    fs::read_to_string(choice).map_or(true, |kept| kept.trim() != "off")
}

/// The daemon's settings: it turns each radio on as it finds it, or
/// leaves it as it is, which is off.
fn settings(on: bool) -> String {
    format!("[Policy]\nAutoEnable={on}\n")
}

/// Let the Bluetooth radios transmit, where the kernel was told to keep
/// them quiet (rfkill's soft block, under `switches`): a laptop's firmware
/// remembers the radio turned off, by its wireless key or by another
/// system, and BlueZ won't turn on a radio that is blocked. A block by a
/// switch on the machine itself is not the kernel's to lift.
fn unblock(switches: &Path) {
    let Ok(entries) = fs::read_dir(switches) else {
        return;
    };
    for entry in entries.flatten() {
        let switch = entry.path();
        let kind = fs::read_to_string(switch.join("type")).unwrap_or_default();
        if kind.trim() == "bluetooth" {
            let _ = fs::write(switch.join("soft"), "0");
        }
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
    // Connecting waits for the device to answer; the rest is the daemon's
    // own to say at once.
    let limit = match arguments.first() {
        Some(&"connect") => CONNECTING,
        _ => ANSWER,
    };
    output_of(Command::new(CLIENT).args(arguments), limit)
}

/// What `command` prints, given `limit` to end in: stopped then, and
/// `None`.
///
/// The limit is kept here and not by the client's own `--timeout`: with
/// that, the client stays for the whole time whatever it was asked, which
/// suits a search and made everything else, how the radio stands
/// included, take eight seconds to say.
fn output_of(command: &mut Command, limit: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read beside the waiting, so that a client with much to say is not
    // kept from ending by a pipe nobody empties.
    let mut printed = child.stdout.take()?;
    let reading = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = printed.read_to_end(&mut bytes);
        bytes
    });
    let until = Instant::now() + limit;
    let ended = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    // Stopped: what it printed is not waited for, which something it
    // started and left behind could keep open.
    if !ended {
        return None;
    }
    let bytes = reading.join().ok()?;
    Some(strip_colours(&String::from_utf8_lossy(&bytes)))
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

/// What the client has said while pairing, as far as it matters.
#[derive(Debug, PartialEq, Eq)]
enum Heard {
    /// Its agent is there: it can be told to pair.
    Ready,
    /// A question, or digits to show.
    Asked(Asked),
    /// Paired, or not: nothing more will come.
    Over,
}

/// What `said`, the client's words since it was last answered, comes to;
/// before it was told to pair (`asked_to_pair`), only whether it is ready.
fn heard(said: &str, asked_to_pair: bool) -> Option<Heard> {
    let plain = strip_colours(said);
    if !asked_to_pair {
        return plain.contains("Agent registered").then_some(Heard::Ready);
    }
    if let Some(question) = asked(said) {
        return Some(Heard::Asked(question));
    }
    let over = ["Pairing successful", "Failed to pair", "not available"];
    over.iter()
        .any(|end| plain.contains(end))
        .then_some(Heard::Over)
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
    fn a_command_is_given_its_time_and_no_more() {
        let mut quick = Command::new("/bin/sh");
        quick.args(["-c", "printf 'Powered: yes\\n'"]);
        let started = Instant::now();
        let said = output_of(&mut quick, Duration::from_secs(5));
        assert_eq!(said.as_deref(), Some("Powered: yes\n"));
        // What it says is there when it ends, not when its time is up.
        assert!(started.elapsed() < Duration::from_secs(2));

        let mut stuck = Command::new("/bin/sh");
        stuck.args(["-c", "echo waiting; sleep 30"]);
        let started = Instant::now();
        assert_eq!(output_of(&mut stuck, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_radio_turned_off_stays_off_when_the_daemon_next_starts() {
        let choice = std::env::temp_dir().join(format!("saule-bluetooth-{}", std::process::id()));
        let _ = fs::remove_file(&choice);
        // Never chosen: on, as BlueZ would have it.
        assert!(wanted_on(&choice));
        assert_eq!(settings(wanted_on(&choice)), "[Policy]\nAutoEnable=true\n");
        remember(&choice, false);
        assert!(!wanted_on(&choice));
        assert_eq!(settings(wanted_on(&choice)), "[Policy]\nAutoEnable=false\n");
        remember(&choice, true);
        assert!(wanted_on(&choice));
        let _ = fs::remove_file(&choice);
    }

    #[test]
    fn only_bluetooth_s_radios_are_let_transmit() {
        let switches = std::env::temp_dir().join(format!("saule-rfkill-{}", std::process::id()));
        for (name, kind) in [("rfkill0", "wlan"), ("rfkill1", "bluetooth")] {
            let switch = switches.join(name);
            fs::create_dir_all(&switch).unwrap();
            fs::write(switch.join("type"), format!("{kind}\n")).unwrap();
            fs::write(switch.join("soft"), "1\n").unwrap();
        }
        unblock(&switches);
        let soft = |name: &str| fs::read_to_string(switches.join(name).join("soft")).unwrap();
        assert_eq!(soft("rfkill0").trim(), "1");
        assert_eq!(soft("rfkill1").trim(), "0");
        let _ = fs::remove_dir_all(&switches);
    }

    #[test]
    fn pairing_is_asked_for_once_the_agent_is_there_and_ends_when_it_says() {
        // Nothing it says before its agent is there is an answer to wait
        // for, a question least of all.
        assert_eq!(heard("[bluetoothctl]> ", false), None);
        assert_eq!(heard("Confirm passkey 1 (yes/no): ", false), None);
        assert_eq!(
            heard(
                "\u{1}\u{1b}[0;94m\u{2}[bluetoothctl]> \u{1b}[0mAgent registered\n",
                false
            ),
            Some(Heard::Ready)
        );
        assert_eq!(
            heard("Attempting to pair with AA:BB:CC:DD:EE:FF\n", true),
            None
        );
        assert_eq!(
            heard("[agent] Confirm passkey 654321 (yes/no): ", true),
            Some(Heard::Asked(Asked::Confirm))
        );
        assert_eq!(heard("Pairing successful\n", true), Some(Heard::Over));
        assert_eq!(
            heard(
                "Failed to pair: org.bluez.Error.AuthenticationFailed\n",
                true
            ),
            Some(Heard::Over)
        );
        assert_eq!(
            heard("Device AA:BB:CC:DD:EE:FF not available\n", true),
            Some(Heard::Over)
        );
    }

    #[test]
    fn a_machine_s_number_is_thirty_two_digits() {
        let id = machine_id();
        assert_eq!(id.trim().len(), 32);
        assert!(id.trim().chars().all(|c| c.is_ascii_hexdigit()));
    }
}

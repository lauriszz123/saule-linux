//! Wi-Fi: the networks in range, and joining one.
//!
//! Joining a network — the scanning, the handshake with its password, the
//! keys — is `wpa_supplicant`'s work. This starts it on the machine's
//! radio and talks to it over its control socket: a datagram socket in
//! `/run/wpa_supplicant`, a line of text out and its answer back, the same
//! conversation `wpa_cli` has.
//!
//! The networks joined before are wpa_supplicant's own file, kept on the
//! data disk (`/data/system/wifi.conf`), so the machine rejoins the one it
//! knows when it starts.
//!
//! What it is asked (`ask`): `on`, `off`, `scan`, `join NAME[<tab>PASSWORD]`
//! and `forget NAME`. How it stands is part of `network` (see
//! `network.rs`).

use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::network::{self, Dhcp};

const SUPPLICANT: &str = "/sbin/wpa_supplicant";
const SOCKETS: &str = "/run/wpa_supplicant";

/// How long joining may take before it has failed: a wrong password is
/// refused over and over, never for good.
const JOIN_TIME: Duration = Duration::from_secs(25);

/// How often the air is looked at again while no network is joined.
const SCAN_EVERY: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------------
// What wpa_supplicant says
// ---------------------------------------------------------------------------

/// A network in range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub name: String,
    /// 0 to 100.
    pub signal: u8,
    /// It wants a password.
    pub secured: bool,
}

/// A strength in dBm as a share: −100 is nothing, −50 and above all of it.
fn signal_of(dbm: i32) -> u8 {
    (2 * (dbm + 100)).clamp(0, 100) as u8
}

/// A network's name as wpa_supplicant prints it, with what isn't plain
/// text written out (`\x20`, `\\`, `\"`), back as it is.
fn unescaped(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        match bytes[i + 1] {
            b'x' if i + 3 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 2..i + 4]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => out.push(byte),
                    Err(_) => out.extend_from_slice(&bytes[i..i + 4]),
                }
                i += 4;
            }
            b'n' => {
                out.push(b'\n');
                i += 2;
            }
            b't' => {
                out.push(b'\t');
                i += 2;
            }
            b'e' => {
                out.push(0x1B);
                i += 2;
            }
            other => {
                out.push(other);
                i += 2;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `SCAN_RESULTS`: a heading, then a line a transmitter — its address, its
/// frequency, its strength, what it asks for, its network's name. A
/// network heard from several is listed once, at its strongest; one that
/// hides its name is left out. Strongest first.
fn parse_scan(results: &str) -> Vec<Seen> {
    let mut seen: Vec<Seen> = Vec::new();
    for line in results.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        let [_, _, strength, flags, name] = fields.as_slice() else {
            continue;
        };
        let name = unescaped(name);
        if name.is_empty() || name.contains('\t') || name.contains('\n') {
            continue;
        }
        let network = Seen {
            signal: signal_of(strength.parse().unwrap_or(-100)),
            secured: ["WPA", "WEP", "SAE", "EAP"]
                .iter()
                .any(|kind| flags.contains(kind)),
            name,
        };
        match seen.iter_mut().find(|other| other.name == network.name) {
            Some(other) if other.signal >= network.signal => {}
            Some(other) => *other = network,
            None => seen.push(network),
        }
    }
    seen.sort_by(|a, b| b.signal.cmp(&a.signal).then(a.name.cmp(&b.name)));
    seen
}

/// `LIST_NETWORKS`: the networks known, each with wpa_supplicant's number
/// for it.
fn parse_known(list: &str) -> Vec<(u32, String)> {
    list.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let id = fields.next()?.parse().ok()?;
            Some((id, unescaped(fields.next()?)))
        })
        .collect()
}

/// One value out of `STATUS`'s `name=value` lines.
fn status_value<'a>(status: &'a str, name: &str) -> Option<&'a str> {
    status
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
}

fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Talking to it
// ---------------------------------------------------------------------------

/// A line to wpa_supplicant and its answer.
pub trait Control {
    fn request(&mut self, command: &str) -> io::Result<String>;
}

/// Its control socket for `interface`. A datagram socket answers to where
/// the question came from, so this end has a name too.
struct Socket {
    socket: UnixDatagram,
    ours: PathBuf,
}

impl Socket {
    fn open(interface: &str) -> io::Result<Socket> {
        let ours = Path::new(SOCKETS).join(format!("saule-system-{}", std::process::id()));
        let _ = std::fs::remove_file(&ours);
        let socket = UnixDatagram::bind(&ours)?;
        socket.connect(Path::new(SOCKETS).join(interface))?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        Ok(Socket { socket, ours })
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.ours);
    }
}

impl Control for Socket {
    fn request(&mut self, command: &str) -> io::Result<String> {
        self.socket.send(command.as_bytes())?;
        let mut answer = vec![0u8; 64 * 1024];
        loop {
            let length = self.socket.recv(&mut answer)?;
            // Lines starting `<` are news nobody asked for, not the answer.
            if !answer[..length].starts_with(b"<") {
                return Ok(String::from_utf8_lossy(&answer[..length]).into_owned());
            }
        }
    }
}

/// Join the network called `name`, with `password` if it wants one. A
/// network known already is joined as it is known unless a password is
/// given, which replaces the one kept. Wpa_supplicant's number for it.
fn join(control: &mut dyn Control, name: &str, password: Option<&str>) -> io::Result<u32> {
    let refused = || io::Error::other("wpa_supplicant refused the network");

    let known = parse_known(&control.request("LIST_NETWORKS")?);
    let (id, new) = match known.iter().find(|(_, known)| known == name) {
        Some((id, _)) => (*id, false),
        None => {
            let answer = control.request("ADD_NETWORK")?;
            (answer.trim().parse().map_err(|_| refused())?, true)
        }
    };

    let mut commands = Vec::new();
    if new {
        // The name as hex: whatever is in it, it needs no quoting.
        commands.push(format!("SET_NETWORK {id} ssid {}", hex(name)));
    }
    match password {
        Some(password) => commands.push(format!("SET_NETWORK {id} psk \"{password}\"")),
        None if new => commands.push(format!("SET_NETWORK {id} key_mgmt NONE")),
        None => {}
    }
    commands.push(format!("SELECT_NETWORK {id}"));

    for command in commands {
        if control.request(&command)?.trim() != "OK" {
            // What was only just added is not left half-made.
            if new {
                let _ = control.request(&format!("REMOVE_NETWORK {id}"));
            }
            return Err(refused());
        }
    }
    Ok(id)
}

// ---------------------------------------------------------------------------
// How it stands
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Joining,
    Joined,
    Failed,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Joining => "joining",
            State::Joined => "joined",
            State::Failed => "failed",
        }
    }
}

struct Radio {
    /// The radio's interface, once one is found.
    interface: Option<String>,
    supplicant: Option<Child>,
    on: bool,
    state: State,
    /// The network joined or being joined.
    name: String,
    signal: u8,
    seen: Vec<Seen>,
    known: Vec<String>,
    /// A join under way: wpa_supplicant's number for the network, whether
    /// it was new (and so forgotten again if the join fails), and when it
    /// began.
    joining: Option<(u32, bool, Instant)>,
    scanned: Option<Instant>,
    /// What was asked since the radio was last looked at.
    asked: Vec<String>,
}

#[derive(Clone)]
pub struct Wifi(Arc<Mutex<Radio>>);

pub fn start() -> Wifi {
    Wifi(Arc::new(Mutex::new(Radio {
        interface: None,
        supplicant: None,
        on: true,
        state: State::Idle,
        name: String::new(),
        signal: 0,
        seen: Vec::new(),
        known: Vec::new(),
        joining: None,
        scanned: None,
        asked: Vec::new(),
    })))
}

impl Wifi {
    /// `on`, `off`, `scan`, `join NAME[<tab>PASSWORD]`, `forget NAME`:
    /// noted, and done the next time the radio is looked at, which is
    /// within two seconds.
    pub fn ask(&self, request: &str) {
        if let Ok(mut radio) = self.0.lock() {
            radio.asked.push(request.to_string());
        }
    }

    /// Do what was asked, take in how the radio stands, and say it: the
    /// `wifi` and `seen` lines of `network`. Called every two seconds by
    /// `network.rs`, with the DHCP clients it keeps.
    pub fn look(&self, dhcp: &mut Dhcp) -> Vec<String> {
        let Ok(mut radio) = self.0.lock() else {
            return Vec::new();
        };
        radio.look(dhcp);
        radio.lines()
    }
}

impl Radio {
    fn look(&mut self, dhcp: &mut Dhcp) {
        if self.interface.is_none() {
            self.interface = network::wireless();
        }
        let Some(interface) = self.interface.clone() else {
            self.asked.clear();
            return;
        };

        // Switched on or off first: the rest needs the radio on.
        let asked = std::mem::take(&mut self.asked);
        for request in &asked {
            match request.as_str() {
                "on" => self.on = true,
                "off" => self.on = false,
                _ => {}
            }
        }
        if !self.on {
            self.stop(&interface, dhcp);
            return;
        }

        network::set_up(&interface, true);
        if !self.running() {
            self.supplicant = supplicant(&interface);
            return;
        }
        let Ok(mut control) = Socket::open(&interface) else {
            return;
        };
        for request in &asked {
            self.act(&mut control, request);
        }
        self.take_in(&mut control);

        if self.state == State::Joined {
            dhcp.want(&interface, network::WIFI_METRIC);
        } else {
            dhcp.drop(&interface);
        }
    }

    fn running(&mut self) -> bool {
        match &mut self.supplicant {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    fn stop(&mut self, interface: &str, dhcp: &mut Dhcp) {
        dhcp.drop(interface);
        if let Some(mut child) = self.supplicant.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        network::set_up(interface, false);
        self.state = State::Idle;
        self.name.clear();
        self.signal = 0;
        self.seen.clear();
        self.joining = None;
    }

    fn act(&mut self, control: &mut dyn Control, request: &str) {
        let (word, rest) = request.split_once(' ').unwrap_or((request, ""));
        match word {
            "scan" => {
                let _ = control.request("SCAN");
                self.scanned = Some(Instant::now());
            }
            "join" => {
                let (name, password) = match rest.split_once('\t') {
                    Some((name, password)) if !password.is_empty() => (name, Some(password)),
                    Some((name, _)) => (name, None),
                    None => (rest, None),
                };
                let new = !self.known.iter().any(|known| known == name);
                match join(control, name, password) {
                    Ok(id) => {
                        self.joining = Some((id, new, Instant::now()));
                        self.state = State::Joining;
                        self.name = name.to_string();
                    }
                    Err(_) => {
                        self.state = State::Failed;
                        self.name = name.to_string();
                    }
                }
            }
            "forget" => {
                let known = parse_known(&control.request("LIST_NETWORKS").unwrap_or_default());
                for (id, _) in known.iter().filter(|(_, name)| name == rest) {
                    let _ = control.request(&format!("REMOVE_NETWORK {id}"));
                }
                let _ = control.request("SAVE_CONFIG");
            }
            _ => {}
        }
    }

    /// How wpa_supplicant says things stand, and what follows from it.
    fn take_in(&mut self, control: &mut dyn Control) {
        let status = control.request("STATUS").unwrap_or_default();
        let complete = status_value(&status, "wpa_state") == Some("COMPLETED");
        let network = status_value(&status, "ssid")
            .map(unescaped)
            .unwrap_or_default();

        match (complete, self.joining) {
            // Joined what was asked for: it is kept, with the others on
            // again (choosing one switches the rest off).
            (true, Some((_, _, _))) if network == self.name => {
                let _ = control.request("ENABLE_NETWORK all");
                let _ = control.request("SAVE_CONFIG");
                self.joining = None;
                self.state = State::Joined;
            }
            (_, Some((id, new, began))) if began.elapsed() > JOIN_TIME => {
                // Not for want of trying: the password is wrong, or the
                // network is gone. One never joined is not kept.
                if new {
                    let _ = control.request(&format!("REMOVE_NETWORK {id}"));
                }
                let _ = control.request("ENABLE_NETWORK all");
                self.joining = None;
                self.state = State::Failed;
            }
            (_, Some(_)) => self.state = State::Joining,
            (true, None) => {
                self.state = State::Joined;
                self.name = network;
            }
            (false, None) if self.state != State::Failed => {
                self.state = State::Idle;
                self.name.clear();
            }
            (false, None) => {}
        }

        self.signal = 0;
        if self.state == State::Joined {
            let poll = control.request("SIGNAL_POLL").unwrap_or_default();
            let dbm = status_value(&poll, "RSSI").and_then(|v| v.parse().ok());
            self.signal = signal_of(dbm.unwrap_or(-100));
        }

        // Look at the air again now and then while nothing is joined;
        // joined, only when asked, since a scan stops traffic for a moment.
        let due = self.scanned.is_none_or(|at| at.elapsed() > SCAN_EVERY);
        if due && self.state != State::Joined {
            let _ = control.request("SCAN");
            self.scanned = Some(Instant::now());
        }

        self.seen = parse_scan(&control.request("SCAN_RESULTS").unwrap_or_default());
        self.known = parse_known(&control.request("LIST_NETWORKS").unwrap_or_default())
            .into_iter()
            .map(|(_, name)| name)
            .collect();
    }

    fn lines(&self) -> Vec<String> {
        let Some(interface) = &self.interface else {
            return vec!["wifi\tnone\tidle\t0\t-\t".to_string()];
        };
        let radio = if self.on { "on" } else { "off" };
        let address = match self.state {
            State::Joined => network::address_or_dash(interface),
            _ => "-".to_string(),
        };
        let mut lines = vec![format!(
            "wifi\t{radio}\t{}\t{}\t{address}\t{}",
            self.state.word(),
            self.signal,
            self.name
        )];
        for network in &self.seen {
            let known = self.known.contains(&network.name);
            lines.push(format!(
                "seen\t{}\t{}\t{}\t{}",
                network.signal,
                u8::from(network.secured),
                u8::from(known),
                network.name
            ));
        }
        lines
    }
}

/// Start wpa_supplicant on `interface`, with the file of known networks:
/// on the data disk where there is one, else one that lasts until the
/// machine is turned off.
fn supplicant(interface: &str) -> Option<Child> {
    if !Path::new(SUPPLICANT).exists() {
        return None;
    }
    let kept = if Path::new("/data/system").is_dir() {
        "/data/system/wifi.conf"
    } else {
        "/run/saule/wifi.conf"
    };
    if !Path::new(kept).exists() {
        let start = format!("ctrl_interface={SOCKETS}\nupdate_config=1\n");
        std::fs::write(kept, start).ok()?;
        // Passwords are in it.
        let _ = std::fs::set_permissions(
            kept,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
        );
    }
    Command::new(SUPPLICANT)
        .args(["-i", interface, "-D", "nl80211", "-c", kept])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wpa_supplicant that answers from a script, and remembers what it
    /// was asked.
    struct Scripted {
        answers: Vec<(&'static str, String)>,
        asked: Vec<String>,
    }

    impl Scripted {
        fn new(answers: &[(&'static str, &str)]) -> Scripted {
            Scripted {
                answers: answers.iter().map(|(q, a)| (*q, a.to_string())).collect(),
                asked: Vec::new(),
            }
        }
    }

    impl Control for Scripted {
        fn request(&mut self, command: &str) -> io::Result<String> {
            self.asked.push(command.to_string());
            let answer = self
                .answers
                .iter()
                .find(|(start, _)| command.starts_with(start))
                .map_or("OK\n".to_string(), |(_, answer)| answer.clone());
            Ok(answer)
        }
    }

    const SCAN: &str = "bssid / frequency / signal level / flags / ssid\n\
        02:00:00:00:01:00\t2412\t-48\t[WPA2-PSK-CCMP][ESS]\tHome\n\
        02:00:00:00:02:00\t5180\t-71\t[WPA2-PSK-CCMP][ESS]\tHome\n\
        02:00:00:00:03:00\t2437\t-80\t[ESS]\tCaf\\xc3\\xa9 Wi-Fi\n\
        02:00:00:00:04:00\t2462\t-90\t[WPA2-PSK-CCMP][ESS]\t\n\
        02:00:00:00:05:00\t2462\t-60\t[WPA3-SAE-CCMP][ESS]\tNeighbour\n";

    #[test]
    fn networks_are_listed_once_each_strongest_first() {
        let seen = parse_scan(SCAN);
        let names: Vec<&str> = seen.iter().map(|n| n.name.as_str()).collect();
        // The one that hides its name is left out; the one heard twice
        // counts at its stronger.
        assert_eq!(names, ["Home", "Neighbour", "Café Wi-Fi"]);
        assert_eq!(seen[0].signal, 100);
        assert_eq!(seen[1].signal, 80);
        assert_eq!(seen[2].signal, 40);
        assert!(seen[0].secured && seen[1].secured && !seen[2].secured);
    }

    #[test]
    fn a_strength_is_a_share_of_what_can_be_heard() {
        assert_eq!(signal_of(-100), 0);
        assert_eq!(signal_of(-75), 50);
        assert_eq!(signal_of(-50), 100);
        assert_eq!(signal_of(-30), 100);
        assert_eq!(signal_of(-120), 0);
    }

    #[test]
    fn a_name_comes_back_as_it_was_broadcast() {
        assert_eq!(unescaped("Caf\\xc3\\xa9"), "Café");
        assert_eq!(
            unescaped("back\\\\slash \\\"quoted\\\""),
            "back\\slash \"quoted\""
        );
        assert_eq!(unescaped("plain"), "plain");
        assert_eq!(unescaped("ends\\"), "ends\\");
    }

    #[test]
    fn known_networks_have_their_numbers() {
        let list = "network id / ssid / bssid / flags\n0\tHome\tany\t[CURRENT]\n3\tOffice\tany\t\n";
        assert_eq!(
            parse_known(list),
            [(0, "Home".to_string()), (3, "Office".to_string())]
        );
        assert!(parse_known("network id / ssid / bssid / flags\n").is_empty());
    }

    #[test]
    fn status_is_names_and_values() {
        let status =
            "bssid=02:00:00:00:01:00\nssid=Home\nwpa_state=COMPLETED\nip_address=10.0.0.7\n";
        assert_eq!(status_value(status, "wpa_state"), Some("COMPLETED"));
        assert_eq!(status_value(status, "ssid"), Some("Home"));
        assert_eq!(status_value(status, "ssi"), None);
    }

    #[test]
    fn a_new_network_is_added_with_its_name_and_password() {
        let mut supplicant = Scripted::new(&[
            ("LIST_NETWORKS", "network id / ssid / bssid / flags\n"),
            ("ADD_NETWORK", "2\n"),
        ]);
        assert_eq!(
            join(&mut supplicant, "Home", Some("secret pass")).unwrap(),
            2
        );
        assert_eq!(
            supplicant.asked,
            [
                "LIST_NETWORKS",
                "ADD_NETWORK",
                "SET_NETWORK 2 ssid 486f6d65",
                "SET_NETWORK 2 psk \"secret pass\"",
                "SELECT_NETWORK 2",
            ]
        );
    }

    #[test]
    fn an_open_network_is_added_as_one() {
        let mut supplicant = Scripted::new(&[
            ("LIST_NETWORKS", "network id / ssid / bssid / flags\n"),
            ("ADD_NETWORK", "0\n"),
        ]);
        join(&mut supplicant, "Café Wi-Fi", None).unwrap();
        assert!(supplicant
            .asked
            .contains(&"SET_NETWORK 0 key_mgmt NONE".to_string()));
        assert!(supplicant
            .asked
            .contains(&"SET_NETWORK 0 ssid 436166c3a92057692d4669".to_string()));
    }

    #[test]
    fn a_known_network_is_joined_as_it_is_known() {
        let mut supplicant = Scripted::new(&[(
            "LIST_NETWORKS",
            "network id / ssid / bssid / flags\n5\tHome\tany\t\n",
        )]);
        assert_eq!(join(&mut supplicant, "Home", None).unwrap(), 5);
        assert_eq!(supplicant.asked, ["LIST_NETWORKS", "SELECT_NETWORK 5"]);
    }

    #[test]
    fn a_network_wpa_supplicant_refuses_is_an_error() {
        let mut supplicant = Scripted::new(&[
            ("LIST_NETWORKS", "network id / ssid / bssid / flags\n"),
            ("ADD_NETWORK", "1\n"),
            // Too short to be a password.
            ("SET_NETWORK 1 psk", "FAIL\n"),
        ]);
        assert!(join(&mut supplicant, "Home", Some("short")).is_err());
    }

    fn radio() -> Radio {
        Radio {
            interface: Some("wlan0".into()),
            supplicant: None,
            on: true,
            state: State::Idle,
            name: String::new(),
            signal: 0,
            seen: Vec::new(),
            known: Vec::new(),
            joining: None,
            scanned: Some(Instant::now()),
            asked: Vec::new(),
        }
    }

    #[test]
    fn joining_ends_joined_when_wpa_supplicant_says_so() {
        let mut radio = radio();
        let mut supplicant = Scripted::new(&[
            ("LIST_NETWORKS", "network id / ssid / bssid / flags\n"),
            ("ADD_NETWORK", "0\n"),
            ("STATUS", "wpa_state=4WAY_HANDSHAKE\nssid=Home\n"),
            ("SCAN_RESULTS", SCAN),
        ]);
        radio.act(&mut supplicant, "join Home\thunter22");
        radio.take_in(&mut supplicant);
        assert_eq!(radio.state, State::Joining);
        assert_eq!(radio.name, "Home");

        let mut supplicant = Scripted::new(&[
            ("STATUS", "wpa_state=COMPLETED\nssid=Home\n"),
            ("SIGNAL_POLL", "RSSI=-60\nLINKSPEED=72\n"),
            (
                "LIST_NETWORKS",
                "network id / ssid / bssid / flags\n0\tHome\tany\t[CURRENT]\n",
            ),
            ("SCAN_RESULTS", SCAN),
        ]);
        radio.take_in(&mut supplicant);
        assert_eq!(radio.state, State::Joined);
        assert_eq!(radio.signal, 80);
        // Kept, and the other networks switched back on.
        assert!(supplicant.asked.contains(&"SAVE_CONFIG".to_string()));
        assert!(supplicant.asked.contains(&"ENABLE_NETWORK all".to_string()));

        let lines = radio.lines();
        assert!(lines[0].starts_with("wifi\ton\tjoined\t80\t"));
        assert!(lines[0].ends_with("\tHome"));
        assert_eq!(lines[1], "seen\t100\t1\t1\tHome");
        assert_eq!(lines[3], "seen\t40\t0\t0\tCafé Wi-Fi");
    }

    #[test]
    fn a_join_that_never_completes_fails_and_is_not_kept() {
        let mut radio = radio();
        radio.name = "Home".into();
        radio.state = State::Joining;
        radio.joining = Some((4, true, Instant::now() - JOIN_TIME - Duration::from_secs(1)));

        let mut supplicant = Scripted::new(&[("STATUS", "wpa_state=SCANNING\n")]);
        radio.take_in(&mut supplicant);
        assert_eq!(radio.state, State::Failed);
        assert!(supplicant.asked.contains(&"REMOVE_NETWORK 4".to_string()));
        // And it stays failed, with its name, until something else is asked.
        radio.take_in(&mut supplicant);
        assert_eq!(radio.state, State::Failed);
        assert_eq!(radio.name, "Home");
    }

    #[test]
    fn a_machine_with_no_radio_says_so() {
        let mut none = radio();
        none.interface = None;
        assert_eq!(none.lines(), ["wifi\tnone\tidle\t0\t-\t"]);
    }
}

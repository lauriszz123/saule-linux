//! The network: a cable's interface brought up and given an address, and
//! how every interface stands.
//!
//! An interface with a cable in it (its carrier is up) is asked an address
//! for by DHCP: BusyBox's `udhcpc`, which stays running to renew it, with
//! the script beside this program setting what it is given (`dhcp`,
//! installed as `/usr/share/saule/dhcp`). Wi-Fi's interface is `wifi.rs`'s
//! until it has joined a network; then it gets its address the same way.
//!
//! How it stands, in `network`, fields separated by tabs:
//!
//!   wired  NAME  1|0   ADDRESS|-      an interface for a cable, whether
//!                                     one is in, and its address
//!   wifi   none|off|on  STATE  SIGNAL  ADDRESS|-  NAME-OF-NETWORK
//!                                     STATE: idle joining joined failed
//!   seen   SIGNAL  1|0  1|0  NAME     a network in range: its strength
//!                                     (0..100), whether it wants a
//!                                     password, whether it is known

use std::collections::HashMap;
use std::fs;
use std::net::Ipv4Addr;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::files;
use crate::wifi::Wifi;

const BUSYBOX: &str = "/usr/bin/busybox";
const DHCP_SCRIPT: &str = "/usr/share/saule/dhcp";

/// Which route is taken when both lead out: the cable's.
const WIRED_METRIC: u32 = 100;
pub const WIFI_METRIC: u32 = 600;

/// DHCP clients running, by interface.
#[derive(Default)]
pub struct Dhcp {
    running: HashMap<String, Child>,
}

impl Dhcp {
    /// Have an address asked for on `interface`, and kept renewed.
    pub fn want(&mut self, interface: &str, metric: u32) {
        if let Some(child) = self.running.get_mut(interface) {
            // Still at it?
            if matches!(child.try_wait(), Ok(None)) {
                return;
            }
        }
        if !Path::new(BUSYBOX).exists() || !Path::new(DHCP_SCRIPT).exists() {
            return;
        }
        let child = Command::new(BUSYBOX)
            .args(["udhcpc", "-f", "-i", interface, "-s", DHCP_SCRIPT])
            .env("METRIC", metric.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(child) = child {
            self.running.insert(interface.to_string(), child);
        }
    }

    /// Stop asking on `interface`, and take its address away.
    pub fn drop(&mut self, interface: &str) {
        if let Some(mut child) = self.running.remove(interface) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = Command::new(BUSYBOX)
                .args(["ip", "addr", "flush", "dev", interface])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// An interface for a cable, as the kernel lists it.
struct Wired {
    name: String,
    cable: bool,
}

/// Every interface that takes a cable: Ethernet's kind, and not a radio.
fn wired() -> Vec<Wired> {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let read = |file: &str| fs::read_to_string(path.join(file)).unwrap_or_default();
        // 1 is Ethernet (ARPHRD_ETHER), which a radio says it is too.
        if read("type").trim() != "1" || is_wireless(&name) {
            continue;
        }
        found.push(Wired {
            cable: read("carrier").trim() == "1",
            name,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

pub fn is_wireless(interface: &str) -> bool {
    let path = Path::new("/sys/class/net").join(interface);
    path.join("wireless").exists() || path.join("phy80211").exists()
}

/// The first radio's interface, if the machine has one.
pub fn wireless() -> Option<String> {
    let mut names: Vec<String> = fs::read_dir("/sys/class/net")
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| is_wireless(name))
        .collect();
    names.sort();
    names.into_iter().next()
}

// The kernel's requests about an interface (<linux/sockios.h>), and the
// structure they are made with (`struct ifreq`: a name, then 24 bytes that
// are an address or flags).
const SIOCGIFFLAGS: libc::Ioctl = 0x8913;
const SIOCSIFFLAGS: libc::Ioctl = 0x8914;
const SIOCGIFADDR: libc::Ioctl = 0x8915;
const IFF_UP: u16 = 1;

fn request(interface: &str) -> [u8; 40] {
    let mut request = [0u8; 40];
    let name = interface.as_bytes();
    let length = name.len().min(15);
    request[..length].copy_from_slice(&name[..length]);
    request
}

fn with_socket<T>(act: impl FnOnce(libc::c_int) -> T) -> Option<T> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    Some(act(socket.as_raw_fd()))
}

/// Switch `interface` on or off. A cable is only noticed in one that is
/// on.
pub fn set_up(interface: &str, up: bool) {
    with_socket(|fd| {
        let mut request = request(interface);
        // SAFETY: both requests read and write within the 40 bytes given.
        unsafe {
            if libc::ioctl(fd, SIOCGIFFLAGS, request.as_mut_ptr()) != 0 {
                return;
            }
            let mut flags = u16::from_ne_bytes([request[16], request[17]]);
            if up == (flags & IFF_UP != 0) {
                return;
            }
            flags = if up { flags | IFF_UP } else { flags & !IFF_UP };
            request[16..18].copy_from_slice(&flags.to_ne_bytes());
            libc::ioctl(fd, SIOCSIFFLAGS, request.as_mut_ptr());
        }
    });
}

/// The address `interface` has, if it has one.
pub fn address(interface: &str) -> Option<Ipv4Addr> {
    with_socket(|fd| {
        let mut request = request(interface);
        // SAFETY: the request writes within the 40 bytes given.
        let ok = unsafe { libc::ioctl(fd, SIOCGIFADDR, request.as_mut_ptr()) == 0 };
        // A `sockaddr_in` after the name: family, port, then the address.
        ok.then(|| Ipv4Addr::new(request[20], request[21], request[22], request[23]))
    })
    .flatten()
}

pub fn address_or_dash(interface: &str) -> String {
    address(interface).map_or_else(|| "-".to_string(), |a| a.to_string())
}

/// Look at the interfaces every couple of seconds, for as long as the
/// machine runs: bring up what has a cable, and say how it all stands.
pub fn start(wifi: Wifi) {
    std::thread::spawn(move || {
        let mut dhcp = Dhcp::default();
        loop {
            let mut lines = Vec::new();
            for interface in wired() {
                set_up(&interface.name, true);
                if interface.cable {
                    dhcp.want(&interface.name, WIRED_METRIC);
                } else {
                    dhcp.drop(&interface.name);
                }
                lines.push(wired_line(
                    &interface.name,
                    interface.cable,
                    &address_or_dash(&interface.name),
                ));
            }
            lines.extend(wifi.look(&mut dhcp));
            files::say("network", &lines);
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

fn wired_line(name: &str, cable: bool, address: &str) -> String {
    format!("wired\t{name}\t{}\t{address}", u8::from(cable))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interface_s_name_starts_the_request() {
        let made = request("enp2s0");
        assert_eq!(&made[..7], b"enp2s0\0");
        // A name too long for the kernel is cut where the kernel would.
        let long = request("a-very-long-interface-name");
        assert_eq!(long[15], 0);
        assert_eq!(long[14], b't');
    }

    #[test]
    fn a_wired_interface_is_one_line() {
        assert_eq!(
            wired_line("eth0", true, "10.0.2.15"),
            "wired\teth0\t1\t10.0.2.15"
        );
        assert_eq!(wired_line("eth0", false, "-"), "wired\teth0\t0\t-");
    }
}

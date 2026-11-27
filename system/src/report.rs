//! A report of the machine, for finding out why something does not work
//! on it.
//!
//! One text file: which system this is, what the computer is made of
//! (every PCI and USB device with its ids and the driver that took it, or
//! none), what draws the screen, what the kernel said while starting, and
//! what the session and its apps printed (init keeps that in
//! `/run/saule/session.log`). No document, name, password or network
//! name is in it.
//!
//! Asked for with `report NOTES`: `NOTES` is what the asker knows and
//! this program does not (the window manager says what the screen is
//! drawn by), added as it is. The file is written to every drive that is
//! plugged in (`/run/media`), named for the moment; how that went is in
//! `report`:
//!
//!   written N NAME|none      how many drives the Nth report was written
//!                            to under NAME, or that there was no drive

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::files;

const BUSYBOX: &str = "/usr/bin/busybox";
const MEDIA: &str = "/run/media";

/// The most of any one file or command's output that goes in, in bytes:
/// its end, where the latest is.
const MOST: usize = 256 * 1024;

pub struct Report {
    made: u32,
}

pub fn start() -> Report {
    files::say("report", &["written 0 none".to_string()]);
    Report { made: 0 }
}

impl Report {
    pub fn ask(&mut self, notes: &str) {
        self.made += 1;
        let name = format!("saule-report-{}.txt", stamp());
        let text = gather(notes);
        let mut drives = 0;
        if let Ok(entries) = fs::read_dir(MEDIA) {
            for entry in entries.flatten() {
                if fs::write(entry.path().join(&name), &text).is_ok() {
                    drives += 1;
                }
            }
        }
        // On the drive before it is said to be: a drive is often pulled
        // out the moment the notice shows.
        let _ = Command::new(BUSYBOX).arg("sync").status();
        let said = if drives > 0 { name } else { "none".to_string() };
        files::say("report", &[format!("written {} {said}", self.made)]);
    }
}

/// The moment, for the file's name: "2026-11-25-184502".
fn stamp() -> String {
    let out = Command::new(BUSYBOX)
        .args(["date", "+%Y-%m-%d-%H%M%S"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    if out.is_empty() {
        "now".to_string()
    } else {
        out
    }
}

fn gather(notes: &str) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "Saule Linux report");
    section(&mut text, "System");
    line(&mut text, "version", &first_line("/etc/saule-release"));
    line(&mut text, "kernel", &first_line("/proc/version"));
    line(&mut text, "options", &first_line("/proc/cmdline"));
    line(&mut text, "up", &first_line("/proc/uptime"));
    for note in notes.split(';') {
        if let Some((name, value)) = note.trim().split_once('=') {
            line(&mut text, name.trim(), value.trim());
        }
    }

    section(&mut text, "Computer");
    for name in [
        "sys_vendor",
        "product_name",
        "product_version",
        "board_name",
        "bios_version",
        "bios_date",
    ] {
        line(
            &mut text,
            name,
            &first_line(&format!("/sys/class/dmi/id/{name}")),
        );
    }
    let cpu = read("/proc/cpuinfo");
    line(&mut text, "processor", &field(&cpu, "model name"));
    line(
        &mut text,
        "threads",
        &cpu.matches("processor\t").count().to_string(),
    );
    line(&mut text, "flags", &field(&cpu, "flags"));
    let memory = read("/proc/meminfo");
    for name in ["MemTotal", "MemFree", "MemAvailable", "Shmem", "SwapTotal"] {
        line(&mut text, name, &field(&memory, name));
    }

    section(
        &mut text,
        "PCI devices (address class vendor:device driver)",
    );
    text.push_str(&pci(Path::new("/sys/bus/pci/devices")));

    section(&mut text, "USB devices (vendor:product name driver)");
    text.push_str(&usb(Path::new("/sys/bus/usb/devices")));

    section(&mut text, "Screens and graphics");
    for (name, _) in sorted("/sys/class/drm") {
        let folder = format!("/sys/class/drm/{name}");
        let status = first_line(&format!("{folder}/status"));
        let modes = read(&format!("{folder}/modes"));
        let first_mode = modes.lines().next().unwrap_or("");
        let _ = writeln!(text, "{name} {status} {first_mode}");
    }
    for folder in [
        "/usr/lib/x86_64-linux-gnu/dri",
        "/usr/lib/dri",
        "/usr/lib/x86_64-linux-gnu",
    ] {
        let names: Vec<String> = sorted(folder)
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| {
                folder.ends_with("dri")
                    || name.contains("EGL")
                    || name.contains("gallium")
                    || name.contains("gbm")
            })
            .collect();
        line(&mut text, folder, &names.join(" "));
    }
    for (name, _) in sorted("/sys/class/backlight") {
        line(&mut text, "backlight", &name);
    }

    section(&mut text, "Network, sound, input, power");
    for (name, path) in sorted("/sys/class/net") {
        let driver = link_name(&path.join("device/driver"));
        let state = first_line(&format!("/sys/class/net/{name}/operstate"));
        let wireless = if path.join("wireless").exists() || path.join("phy80211").exists() {
            " wireless"
        } else {
            ""
        };
        let _ = writeln!(text, "net {name} {driver} {state}{wireless}");
    }
    for card in read("/proc/asound/cards").lines() {
        let _ = writeln!(text, "sound {}", card.trim());
    }
    for input in read("/proc/bus/input/devices").lines() {
        if let Some(name) = input.strip_prefix("N: Name=") {
            let _ = writeln!(text, "input {name}");
        }
    }
    for (name, path) in sorted("/sys/class/power_supply") {
        let kind = first_line(&path.join("type").to_string_lossy());
        let status = first_line(&path.join("status").to_string_lossy());
        let _ = writeln!(text, "power {name} {kind} {status}");
    }
    for name in ["power", "sound", "display", "clock", "update", "disks"] {
        for said in read(&format!("{}/{name}", files::FOLDER)).lines() {
            let _ = writeln!(text, "state {name}: {said}");
        }
    }

    section(&mut text, "Programs");
    text.push_str(&end_of(&run(&["ps"])));

    section(&mut text, "What init said");
    text.push_str(&end_of(&read("/run/saule/init.log")));

    section(&mut text, "What the session and its apps printed, before");
    text.push_str(&end_of(&read("/run/saule/session.log.old")));
    section(&mut text, "What the session and its apps printed");
    text.push_str(&end_of(&read("/run/saule/session.log")));

    section(&mut text, "What the kernel said");
    text.push_str(&end_of(&run(&["dmesg"])));
    text
}

/// Every PCI device under `root`, a line each, in the order of their
/// addresses.
fn pci(root: &Path) -> String {
    let mut text = String::new();
    for (address, path) in sorted(&root.to_string_lossy()) {
        let value = |file: &str| first_line(&path.join(file).to_string_lossy()).replace("0x", "");
        let driver = link_name(&path.join("driver"));
        let driver = if driver.is_empty() {
            "(no driver)".to_string()
        } else {
            driver
        };
        let _ = writeln!(
            text,
            "{address} {} {}:{} {driver}",
            value("class"),
            value("vendor"),
            value("device")
        );
    }
    text
}

/// Every USB device under `root` (not its interfaces, nor the hubs' own
/// ports), a line each.
fn usb(root: &Path) -> String {
    let mut text = String::new();
    for (name, path) in sorted(&root.to_string_lossy()) {
        let value = |file: &str| first_line(&path.join(file).to_string_lossy());
        let vendor = value("idVendor");
        if vendor.is_empty() {
            continue;
        }
        // What took its first interface, which is what a device with one
        // job has.
        let driver = sorted(&path.to_string_lossy())
            .into_iter()
            .filter(|(inner, _)| inner.starts_with(&format!("{name}:")))
            .map(|(_, inner)| link_name(&inner.join("driver")))
            .find(|driver| !driver.is_empty())
            .unwrap_or_else(|| "(no driver)".to_string());
        let _ = writeln!(
            text,
            "{vendor}:{} {} {} {driver}",
            value("idProduct"),
            value("manufacturer"),
            value("product")
        );
    }
    text
}

fn section(text: &mut String, title: &str) {
    let _ = writeln!(text, "\n== {title} ==");
}

fn line(text: &mut String, name: &str, value: &str) {
    let _ = writeln!(text, "{name}: {value}");
}

fn read(path: &str) -> String {
    fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn first_line(path: &str) -> String {
    read(path).lines().next().unwrap_or("").trim().to_string()
}

/// The value of the first `name : value` line of `text`, as
/// `/proc/cpuinfo` and `/proc/meminfo` write them.
fn field(text: &str, name: &str) -> String {
    text.lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == name).then(|| value.trim().to_string())
        })
        .unwrap_or_default()
}

/// The entries of the folder `path`, by name.
fn sorted(path: &str) -> Vec<(String, std::path::PathBuf)> {
    let mut entries: Vec<_> = fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        entry.path(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

/// The last part of what the link at `path` points to: a device's
/// `driver` link names its driver.
fn link_name(path: &Path) -> String {
    fs::read_link(path)
        .ok()
        .and_then(|to| {
            to.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

fn run(args: &[&str]) -> String {
    Command::new(BUSYBOX)
        .args(args)
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

/// The end of `text`, from the start of a line, when it is longer than
/// [`MOST`]; with a line saying so.
fn end_of(text: &str) -> String {
    if text.len() <= MOST {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        return out;
    }
    let mut from = text.len() - MOST;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let rest = &text[from..];
    let rest = rest.split_once('\n').map_or(rest, |(_, after)| after);
    format!("(the end of it)\n{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("saule-report-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn pci_devices_are_listed_with_their_ids_and_drivers() {
        let root = folder("pci");
        for (address, class, vendor, device) in [
            ("0000:01:00.0", "0x030200", "0x10de", "0x139b"),
            ("0000:00:02.0", "0x030000", "0x8086", "0x191b"),
        ] {
            let at = root.join(address);
            fs::create_dir_all(&at).unwrap();
            fs::write(at.join("class"), format!("{class}\n")).unwrap();
            fs::write(at.join("vendor"), format!("{vendor}\n")).unwrap();
            fs::write(at.join("device"), format!("{device}\n")).unwrap();
        }
        std::os::unix::fs::symlink("../../drivers/i915", root.join("0000:00:02.0/driver")).unwrap();

        assert_eq!(
            pci(&root),
            "0000:00:02.0 030000 8086:191b i915\n0000:01:00.0 030200 10de:139b (no driver)\n"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn usb_devices_are_listed_without_their_interfaces() {
        let root = folder("usb");
        let device = root.join("1-4");
        fs::create_dir_all(device.join("1-4:1.0")).unwrap();
        fs::write(device.join("idVendor"), "0a5c\n").unwrap();
        fs::write(device.join("idProduct"), "219c\n").unwrap();
        fs::write(device.join("product"), "Bluetooth Module\n").unwrap();
        std::os::unix::fs::symlink("../../drivers/btusb", device.join("1-4:1.0/driver")).unwrap();
        // An interface has a folder of its own beside the device's.
        fs::create_dir_all(root.join("1-4:1.0")).unwrap();

        assert_eq!(usb(&root), "0a5c:219c  Bluetooth Module btusb\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fields_are_read_as_proc_writes_them() {
        let text = "processor\t: 0\nmodel name\t: Intel(R) Atom(TM) CPU N450   @ 1.66GHz\nMemTotal:        1014404 kB\n";
        assert_eq!(
            field(text, "model name"),
            "Intel(R) Atom(TM) CPU N450   @ 1.66GHz"
        );
        assert_eq!(field(text, "MemTotal"), "1014404 kB");
        assert_eq!(field(text, "flags"), "");
    }

    #[test]
    fn a_long_text_keeps_its_end_from_a_whole_line() {
        let short = "one\ntwo";
        assert_eq!(end_of(short), "one\ntwo\n");
        let long = format!("{}\nlast line\n", "x".repeat(MOST + 10));
        assert_eq!(end_of(&long), "(the end of it)\nlast line\n");
    }
}

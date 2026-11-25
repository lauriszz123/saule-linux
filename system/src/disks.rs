//! Drives that are plugged in: a USB stick, a card, an external disk.
//!
//! Mounting takes root, so it is done here. Every couple of seconds the
//! kernel's list of disks is looked at; each partition of a disk that
//! came by USB (or says it can be taken out), and isn't one the system
//! itself is on, is mounted under `/run/media`, by its label where it
//! has one, for the session's account to read and write. A drive pulled
//! out is unmounted; one asked to be ejected is flushed and unmounted
//! first, and left alone until it has been pulled.
//!
//! How it stands, in `disks`, a line a mounted drive, tabs between:
//!
//!   drive  DEVICE  WHERE  LABEL  BYTES  FILESYSTEM
//!
//! The request: `eject DEVICE` (`sdb1`).

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::files;

const MEDIA: &str = "/run/media";

/// The file systems tried, in order, for a drive that doesn't say.
const KINDS: [&str; 4] = ["vfat", "exfat", "ntfs3", "ext4"];

#[derive(Clone, Debug, PartialEq, Eq)]
struct Mounted {
    at: PathBuf,
    label: String,
    bytes: u64,
    kind: String,
}

#[derive(Default)]
struct State {
    mounted: HashMap<String, Mounted>,
    /// Ejected, and still plugged in: not to be mounted again.
    ejected: HashSet<String>,
}

#[derive(Clone)]
pub struct Disks {
    state: Arc<Mutex<State>>,
}

pub fn start(owner: Option<(u32, u32)>) -> Disks {
    let disks = Disks {
        state: Arc::new(Mutex::new(State::default())),
    };
    let _ = fs::create_dir_all(MEDIA);
    files::say("disks", &[]);

    let watching = disks.clone();
    std::thread::spawn(move || loop {
        watching.look(owner);
        std::thread::sleep(Duration::from_secs(2));
    });
    disks
}

impl Disks {
    pub fn ask(&self, word: &str, rest: &str) {
        if word != "eject" {
            return;
        }
        let device = rest.trim();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(mounted) = state.mounted.remove(device) {
            // SAFETY: a plain system call.
            unsafe { libc::sync() };
            unmount(&mounted.at, false);
            state.ejected.insert(device.to_string());
        }
        say(&state);
    }

    /// Mount what has come, forget what has gone.
    fn look(&self, owner: Option<(u32, u32)>) {
        let present = removable_partitions();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let before = state.mounted.clone();

        let gone: Vec<String> = state
            .mounted
            .keys()
            .filter(|device| !present.contains(*device))
            .cloned()
            .collect();
        for device in gone {
            if let Some(mounted) = state.mounted.remove(&device) {
                unmount(&mounted.at, true);
            }
        }
        state.ejected.retain(|device| present.contains(device));

        for device in &present {
            if state.mounted.contains_key(device) || state.ejected.contains(device) {
                continue;
            }
            let told = describe(device);
            if told.label.starts_with("SAULE") {
                continue;
            }
            let taken: Vec<&Path> = state.mounted.values().map(|m| m.at.as_path()).collect();
            let at = Path::new(MEDIA).join(place_name(&told.label, device, &taken));
            if let Some(kind) = mount(device, &at, &told.kind, owner) {
                state.mounted.insert(
                    device.clone(),
                    Mounted {
                        at,
                        label: told.label,
                        bytes: size_of(device),
                        kind,
                    },
                );
            }
        }

        if state.mounted != before {
            say(&state);
        }
    }
}

fn say(state: &State) {
    let mut lines: Vec<String> = state
        .mounted
        .iter()
        .map(|(device, m)| {
            format!(
                "drive\t{device}\t{}\t{}\t{}\t{}",
                m.at.display(),
                m.label,
                m.bytes,
                m.kind
            )
        })
        .collect();
    lines.sort();
    files::say("disks", &lines);
}

/// What a partition says of itself.
#[derive(Debug, Default, PartialEq, Eq)]
struct Told {
    label: String,
    kind: String,
}

fn describe(device: &str) -> Told {
    use std::io::Read;
    let mut start = [0u8; 2048];
    let read =
        fs::File::open(format!("/dev/{device}")).and_then(|mut file| file.read_exact(&mut start));
    match read {
        Ok(()) => sniff(&start),
        Err(_) => Told::default(),
    }
}

/// What the start of a partition says it is, and is called: read from
/// the file system's own first sectors, since the image has no tool that
/// tells. FAT keeps a name in its boot sector, ext in its superblock;
/// exFAT and NTFS are known for what they are, and keep their names
/// further in than is looked.
fn sniff(start: &[u8]) -> Told {
    let text = |from: usize, to: usize| {
        String::from_utf8_lossy(&start[from..to])
            .trim_matches(|c: char| c == ' ' || c == '\0')
            .to_string()
    };
    if start.len() < 2048 {
        return Told::default();
    }
    if &start[3..11] == b"EXFAT   " {
        return Told {
            label: String::new(),
            kind: "exfat".into(),
        };
    }
    if &start[3..11] == b"NTFS    " {
        return Told {
            label: String::new(),
            kind: "ntfs3".into(),
        };
    }
    // FAT32 says so at 82, and names itself at 71; FAT16 and FAT12 at 54
    // and 43. "NO NAME" is what a formatter writes for none.
    let fat = if &start[82..87] == b"FAT32" {
        Some(text(71, 82))
    } else if &start[54..57] == b"FAT" {
        Some(text(43, 54))
    } else {
        None
    };
    if let Some(label) = fat {
        return Told {
            label: if label == "NO NAME" {
                String::new()
            } else {
                label
            },
            kind: "vfat".into(),
        };
    }
    // ext2, 3 and 4: the superblock is at 1024, its magic at 56 in it and
    // the name at 120.
    if start[1024 + 56] == 0x53 && start[1024 + 57] == 0xef {
        return Told {
            label: text(1024 + 120, 1024 + 136),
            kind: "ext4".into(),
        };
    }
    Told::default()
}

/// The folder a drive is mounted at: its label, with what can't be in a
/// name taken out; "USB Drive" for one without; a number after it when
/// another drive already has that name.
fn place_name(label: &str, device: &str, taken: &[&Path]) -> String {
    let cleaned: String = label
        .chars()
        .filter(|c| !c.is_control() && *c != '/')
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    let base = if cleaned.is_empty() {
        "USB Drive".to_string()
    } else {
        cleaned
    };
    let free = |name: &str| !taken.iter().any(|at| at.ends_with(name));
    if free(&base) {
        return base;
    }
    (2..100)
        .map(|n| format!("{base} {n}"))
        .find(|name| free(name))
        .unwrap_or_else(|| format!("{base} {device}"))
}

/// The options a file system of `kind` is mounted with. Those that have
/// no owners of their own (FAT and its like) are given the session's
/// account as the owner of everything; FAT is also told to write out
/// promptly, since sticks are pulled without asking.
fn options(kind: &str, owner: Option<(u32, u32)>) -> String {
    let owned = owner.map(|(uid, gid)| format!("uid={uid},gid={gid}"));
    match (kind, owned) {
        ("vfat", Some(owned)) => format!("{owned},umask=022,utf8,flush"),
        ("vfat", None) => "utf8,flush".to_string(),
        ("exfat", Some(owned)) => format!("{owned},umask=022"),
        ("ntfs3", Some(owned)) => format!("{owned},umask=022"),
        _ => String::new(),
    }
}

/// Mount `/dev/DEVICE` at `at`. The kind of file system it took.
fn mount(device: &str, at: &Path, said: &str, owner: Option<(u32, u32)>) -> Option<String> {
    fs::create_dir_all(at).ok()?;
    let source = CString::new(format!("/dev/{device}")).ok()?;
    let target = CString::new(at.as_os_str().as_encoded_bytes()).ok()?;
    let said = match said {
        "ntfs" => "ntfs3",
        "ext2" | "ext3" => "ext4",
        other => other,
    };
    let mut kinds: Vec<&str> = KINDS.to_vec();
    if let Some(known) = KINDS.iter().position(|kind| *kind == said) {
        kinds.swap(0, known);
    }

    for kind in kinds {
        let (Ok(kind_c), Ok(data)) = (CString::new(kind), CString::new(options(kind, owner)))
        else {
            continue;
        };
        let flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOATIME;
        // SAFETY: every pointer is to a NUL-terminated string that outlives
        // the call.
        let done = unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                kind_c.as_ptr(),
                flags,
                data.as_ptr().cast(),
            )
        };
        if done == 0 {
            // A file system with owners of its own (ext4) is the
            // session's to write in at its top, as a stick's is expected
            // to be.
            if options(kind, owner).is_empty() {
                if let Some((uid, gid)) = owner {
                    // SAFETY: as above.
                    unsafe { libc::chown(target.as_ptr(), uid, gid) };
                }
            }
            return Some(kind.to_string());
        }
    }
    let _ = fs::remove_dir(at);
    None
}

fn unmount(at: &Path, gone: bool) {
    let Ok(target) = CString::new(at.as_os_str().as_encoded_bytes()) else {
        return;
    };
    // A drive already pulled is let go of whatever still has it open.
    let flags = if gone { libc::MNT_DETACH } else { 0 };
    // SAFETY: a NUL-terminated path that outlives the calls.
    unsafe {
        if libc::umount2(target.as_ptr(), flags) != 0 && !gone {
            libc::umount2(target.as_ptr(), libc::MNT_DETACH);
        }
    }
    let _ = fs::remove_dir(at);
}

fn size_of(device: &str) -> u64 {
    let sectors = fs::read_to_string(format!("/sys/class/block/{device}/size"))
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0);
    sectors * 512
}

/// The partitions of every disk that can be taken out, by the kernel's
/// names (`sdb1`); a disk with no partitions is its own.
fn removable_partitions() -> Vec<String> {
    let system = system_disks(&fs::read_to_string("/proc/mounts").unwrap_or_default());
    let mut found = Vec::new();
    let Ok(disks) = fs::read_dir("/sys/block") else {
        return found;
    };
    for disk in disks.flatten() {
        let name = disk.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("sd") || name.starts_with("mmcblk")) || system.contains(&name) {
            continue;
        }
        let path = disk.path();
        let removable = fs::read_to_string(path.join("removable")).is_ok_and(|r| r.trim() == "1");
        let by_usb =
            fs::canonicalize(&path).is_ok_and(|real| real.to_string_lossy().contains("/usb"));
        let has_medium = fs::read_to_string(path.join("size")).is_ok_and(|s| s.trim() != "0");
        if !(removable || by_usb) || !has_medium {
            continue;
        }
        let mut parts: Vec<String> = fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|part| part.starts_with(&name) && part != &name)
            .collect();
        parts.sort();
        if parts.is_empty() {
            found.push(name);
        } else {
            found.extend(parts);
        }
    }
    found
}

/// The disks the system itself is on, from `/proc/mounts`: whatever is
/// mounted outside `/run/media` from a `/dev/sd…` or `/dev/mmcblk…`.
fn system_disks(mounts: &str) -> HashSet<String> {
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let device = fields.next()?.strip_prefix("/dev/")?;
            let at = fields.next()?;
            (!at.starts_with(MEDIA)).then(|| disk_of(device))
        })
        .collect()
}

/// The disk a partition is on: `sda2` is on `sda`, `mmcblk0p1` on
/// `mmcblk0`.
fn disk_of(partition: &str) -> String {
    // A card's own name ends in a number, and its partitions add `p` and
    // another.
    if partition.starts_with("mmcblk") {
        return match partition.rfind('p') {
            Some(at) if at > 6 => partition[..at].to_string(),
            _ => partition.to_string(),
        };
    }
    partition
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partition_says_what_it_is_and_is_called() {
        let mut fat32 = vec![0u8; 2048];
        fat32[82..90].copy_from_slice(b"FAT32   ");
        fat32[71..82].copy_from_slice(b"PHOTOS 2024");
        assert_eq!(
            sniff(&fat32),
            Told {
                label: "PHOTOS 2024".into(),
                kind: "vfat".into()
            }
        );

        let mut fat16 = vec![0u8; 2048];
        fat16[54..62].copy_from_slice(b"FAT16   ");
        fat16[43..54].copy_from_slice(b"NO NAME    ");
        assert_eq!(
            sniff(&fat16),
            Told {
                label: String::new(),
                kind: "vfat".into()
            }
        );

        let mut ext = vec![0u8; 2048];
        ext[1024 + 56] = 0x53;
        ext[1024 + 57] = 0xef;
        ext[1024 + 120..1024 + 126].copy_from_slice(b"backup");
        assert_eq!(
            sniff(&ext),
            Told {
                label: "backup".into(),
                kind: "ext4".into()
            }
        );

        let mut exfat = vec![0u8; 2048];
        exfat[3..11].copy_from_slice(b"EXFAT   ");
        assert_eq!(sniff(&exfat).kind, "exfat");
        assert_eq!(sniff(&vec![0u8; 2048]), Told::default());
        assert_eq!(sniff(&[0u8; 10]), Told::default());
    }

    #[test]
    fn a_drive_is_mounted_under_its_label_or_a_name_for_one_without() {
        let none: [&Path; 0] = [];
        assert_eq!(place_name("PHOTOS", "sdb1", &none), "PHOTOS");
        assert_eq!(place_name("", "sdb1", &none), "USB Drive");
        assert_eq!(place_name("../etc", "sdb1", &none), "etc");
        let taken = [
            Path::new("/run/media/USB Drive"),
            Path::new("/run/media/USB Drive 2"),
        ];
        assert_eq!(place_name(" ", "sdc1", &taken), "USB Drive 3");
    }

    #[test]
    fn file_systems_without_owners_are_given_the_session_s() {
        assert_eq!(
            options("vfat", Some((100, 100))),
            "uid=100,gid=100,umask=022,utf8,flush"
        );
        assert_eq!(
            options("exfat", Some((100, 100))),
            "uid=100,gid=100,umask=022"
        );
        assert_eq!(options("ext4", Some((100, 100))), "");
        assert_eq!(options("vfat", None), "utf8,flush");
    }

    #[test]
    fn the_system_s_own_disks_are_not_drives() {
        let mounts = "none / rootfs rw 0 0\n/dev/sda2 /data ext4 rw 0 0\n\
                      /dev/sdb1 /run/media/PHOTOS vfat rw 0 0\n/dev/mmcblk0p2 /data2 ext4 rw 0 0\n";
        let system = system_disks(mounts);
        assert!(system.contains("sda"));
        assert!(system.contains("mmcblk0"));
        assert!(!system.contains("sdb"));
        assert_eq!(disk_of("sdb1"), "sdb");
        assert_eq!(disk_of("sdc"), "sdc");
        assert_eq!(disk_of("mmcblk1p3"), "mmcblk1");
        assert_eq!(disk_of("mmcblk1"), "mmcblk1");
    }
}

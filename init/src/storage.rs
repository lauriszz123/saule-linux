//! The data disk: where what the session saves outlives a reboot.
//!
//! Saule Linux runs from memory (the initramfs), so everything it writes is
//! gone when the machine goes off. This finds a place on a disk instead and
//! mounts it at `/data`:
//!
//! 1. an ext2/3/4 file system labelled `SAULE`, on any disk or partition —
//!    one made before, here or by hand;
//! 2. else, on an internal disk that is *completely empty* (no partition
//!    table, nothing written in its first megabyte), one partition across
//!    the disk with such a file system (ext4, journaled), made now.
//!
//! It is checked, and mended if a power cut left it damaged, before it is
//! mounted.
//!
//! A disk with anything on it is never touched: the netbook's disk may still
//! hold Windows, and losing that to a first boot would be unforgivable. With
//! no data disk the system runs as before and keeps nothing.
//!
//! The session gets `SAULE_DATA=/data`, and its home in `/data/home/<user>`:
//! what it keeps there (its settings in `~/.config/saule`) is kept. Its
//! caches are not: they stay in memory (`XDG_CACHE_HOME`, see `session.rs`).

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::log;

/// Where the data disk is mounted.
pub const MOUNT: &str = "/data";

/// The label that marks Saule's file system.
const LABEL: &str = "SAULE";

/// The tools that make and check the file system: e2fsprogs' own, which
/// the image has when the build machine does (every Linux has). They make
/// ext4 with a journal, so a power cut costs the last moment's writes and
/// not the file system, and `e2fsck` mends what is left of one anyway.
const MKE2FS: &str = "/sbin/mke2fs";
const E2FSCK: &str = "/sbin/e2fsck";

/// BusyBox's `mke2fs`, in an image without those but with the command-line
/// tools (`TOOLS=1`): plain ext2, no journal, and nothing to check it with.
const MKE2FS_BUSYBOX: &str = "/usr/bin/mke2fs";

/// How the data disk is mounted. Synchronously: a write is on the disk
/// when it returns, not seconds later. The machine may lose power at any
/// moment (a netbook's battery; a QEMU window closed), and what is kept
/// here is small and rarely written — settings, documents — so being slow
/// to write costs little, and a setting changed a moment before the power
/// went being lost costs more.
const MOUNT_FLAGS: libc::c_ulong =
    libc::MS_NOATIME | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_SYNCHRONOUS | libc::MS_DIRSYNC;

/// Where the one partition starts, in 512-byte sectors: at the first
/// megabyte, as every partitioning tool has put it for years.
const PARTITION_START: u32 = 2048;

/// The `ioctl` that has the kernel read a disk's partition table again
/// (`BLKRRPART` in `<linux/fs.h>`, which the libc crate doesn't name).
const BLKRRPART: libc::Ioctl = 0x125F;

/// Where an ext file system's superblock is on its device, and inside it
/// the magic number that says it is one and the volume's name.
const SUPERBLOCK_OFFSET: u64 = 1024;
const SUPERBLOCK_SIZE: usize = 1024;
const EXT_MAGIC_AT: usize = 0x38;
const EXT_MAGIC: u16 = 0xEF53;
const EXT_LABEL_AT: std::ops::Range<usize> = 0x78..0x88;

/// How long to wait for disks. The kernel finishes probing built-in SATA
/// and NVMe before it starts init, so those are there already; USB drives
/// are found seconds later, by the hub's own thread. A machine with no
/// disk must not be kept from starting for long, so: this long for the
/// first disk (or USB drive) to show up at all,
const FIRST_DISK: Duration = Duration::from_millis(1500);
/// this long more, once one has, for its partitions and any other disk,
const AFTER_A_DISK: Duration = Duration::from_millis(500);
/// and this long while a USB drive is plugged in that has no disk yet:
/// those take seconds.
const USB_DRIVE: Duration = Duration::from_secs(4);

static MOUNTED: OnceLock<bool> = OnceLock::new();

/// Whether `/data` is a disk.
pub fn mounted() -> bool {
    MOUNTED.get().copied().unwrap_or(false)
}

/// Find, or make, the data disk and mount it at [`MOUNT`]. Never fails:
/// anything wrong is logged and the system runs without one.
pub fn mount_data() {
    let found = find_or_make();
    let ok = match found {
        Some(device) => match check(&device).and_then(|()| mount(&device)) {
            Ok(()) => {
                log::info(&format!("data disk: {} at {MOUNT}", device.display()));
                true
            }
            Err(error) => {
                log::warn(&format!("could not mount {}: {error}", device.display()));
                false
            }
        },
        None => {
            log::info("no data disk: nothing is kept across a reboot");
            false
        }
    };
    let _ = MOUNTED.set(ok);
}

/// Check the file system on `device` before it is mounted, and mend it:
/// after a power cut it may not be whole. Clean, `e2fsck` looks at one flag
/// and is done; an image without `e2fsck` mounts it as it is.
fn check(device: &Path) -> std::io::Result<()> {
    if !Path::new(E2FSCK).exists() {
        return Ok(());
    }
    // -p mends what is safe to mend unasked; what it leaves, -y mends.
    match e2fsck(device, "-p")? {
        code if code & FSCK_UNCORRECTED == 0 => {
            if code & FSCK_CORRECTED != 0 {
                log::info(&format!("{}: file system mended", device.display()));
            }
            return Ok(());
        }
        _ => log::warn(&format!(
            "{}: the file system is damaged, repairing it",
            device.display()
        )),
    }
    let code = e2fsck(device, "-y")?;
    if code & FSCK_UNCORRECTED != 0 {
        return Err(std::io::Error::other(format!(
            "e2fsck could not repair it (exit code {code})"
        )));
    }
    Ok(())
}

/// What `e2fsck` says by its exit code: bits, of which these two matter.
const FSCK_CORRECTED: i32 = 1;
const FSCK_UNCORRECTED: i32 = 4;

/// Run `e2fsck` with `mode` on `device`, quietly; its exit code.
fn e2fsck(device: &Path, mode: &str) -> std::io::Result<i32> {
    let status = Command::new(E2FSCK)
        .arg(mode)
        .arg(device)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    status
        .code()
        .ok_or_else(|| std::io::Error::other("e2fsck was killed"))
}

/// The home of `user` on the data disk, made and handed to them.
pub fn home_for(user: &str, uid: libc::uid_t, gid: libc::gid_t) -> Option<PathBuf> {
    if !mounted() {
        return None;
    }
    let home = Path::new(MOUNT).join("home").join(user);
    if let Err(error) = fs::create_dir_all(&home) {
        log::warn(&format!("could not make {}: {error}", home.display()));
        return None;
    }
    let _ = crate::account::chown(&home, uid, gid);
    let _ = crate::account::chmod(&home, 0o700);
    // `/data/home` itself: readable, so a home can be found in it.
    let _ = crate::account::chmod(&Path::new(MOUNT).join("home"), 0o755);
    Some(home)
}

/// Write everything out and let go of the disk, before power goes.
pub fn unmount() {
    if !mounted() {
        return;
    }
    // SAFETY: plain system calls with a NUL-terminated path.
    unsafe {
        libc::sync();
        if let Ok(target) = CString::new(MOUNT) {
            if libc::umount2(target.as_ptr(), 0) != 0 {
                // Still in use (a process the session left behind):
                // read-only at least, so nothing is half-written.
                libc::mount(
                    std::ptr::null(),
                    target.as_ptr(),
                    std::ptr::null(),
                    libc::MS_REMOUNT | libc::MS_RDONLY,
                    std::ptr::null(),
                );
            }
        }
    }
}

fn find_or_make() -> Option<PathBuf> {
    let started = Instant::now();
    let mut first_disk_at = None;
    loop {
        if let Some(device) = labelled() {
            return Some(device);
        }
        let disks = disks();
        let waited = started.elapsed();
        if first_disk_at.is_none() && !disks.is_empty() {
            first_disk_at = Some(waited);
        }
        if !worth_waiting(waited, first_disk_at, usb_drive_pending(&disks)) {
            return disks
                .into_iter()
                .find(|disk| !disk.usb && !disk.removable && is_blank(&disk.device))
                .and_then(|disk| make_on(&disk));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether a disk, or a partition with Saule's file system, may still turn
/// up `waited` after looking began: see the times above. `first_disk_at`
/// is when the first disk was seen.
fn worth_waiting(waited: Duration, first_disk_at: Option<Duration>, usb_pending: bool) -> bool {
    if usb_pending {
        return waited < USB_DRIVE;
    }
    match first_disk_at {
        Some(at) => waited < at + AFTER_A_DISK,
        None => waited < FIRST_DISK,
    }
}

/// Whether a USB drive is plugged in that the kernel has not made a disk
/// of yet: more USB storage interfaces (class 08h) than USB disks.
fn usb_drive_pending(disks: &[Disk]) -> bool {
    let Ok(entries) = fs::read_dir("/sys/bus/usb/devices") else {
        return false;
    };
    let drives = entries
        .flatten()
        .filter(|entry| {
            fs::read_to_string(entry.path().join("bInterfaceClass"))
                .is_ok_and(|class| class.trim() == "08")
        })
        .count();
    drives > disks.iter().filter(|disk| disk.usb).count()
}

/// A whole disk, as `/sys/block` lists it.
struct Disk {
    name: String,
    device: PathBuf,
    sectors: u64,
    removable: bool,
    /// On a USB port: a stick or an enclosure, never formatted unasked.
    usb: bool,
}

fn disks() -> Vec<Disk> {
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return Vec::new();
    };
    let mut disks = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("sd") || name.starts_with("vd") || name.starts_with("nvme")) {
            continue;
        }
        let read = |file: &str| fs::read_to_string(entry.path().join(file)).unwrap_or_default();
        let sectors = read("size").trim().parse().unwrap_or(0);
        if sectors == 0 {
            continue;
        }
        // `/sys/block/sda` links to the disk's place on its bus.
        let usb = fs::canonicalize(entry.path()).is_ok_and(|path| on_usb(&path));
        disks.push(Disk {
            device: Path::new("/dev").join(&name),
            removable: read("removable").trim() == "1",
            usb,
            sectors,
            name,
        });
    }
    disks.sort_by(|a, b| a.name.cmp(&b.name));
    disks
}

/// Whether the device at sysfs `path` hangs off a USB controller, whose
/// root hubs are `usb1`, `usb2`, …:
/// `/sys/devices/pci0000:00/0000:00:1d.7/usb1/1-1/1-1:1.0/host2/…/block/sdb`.
fn on_usb(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str()
            .to_str()
            .and_then(|name| name.strip_prefix("usb"))
            .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Every disk and partition with an ext file system labelled [`LABEL`].
fn labelled() -> Option<PathBuf> {
    let entries = fs::read_dir("/sys/class/block").ok()?;
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with("loop") && !n.starts_with("ram"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|n| Path::new("/dev").join(n))
        .find(|device| ext_label(device).as_deref() == Some(LABEL))
}

/// The label of the ext file system on `device`, if it has one.
fn ext_label(device: &Path) -> Option<String> {
    let mut file = File::open(device).ok()?;
    let mut superblock = [0u8; SUPERBLOCK_SIZE];
    file.seek(SeekFrom::Start(SUPERBLOCK_OFFSET)).ok()?;
    file.read_exact(&mut superblock).ok()?;
    label_in(&superblock)
}

/// The volume name in an ext superblock; `None` when the block isn't one.
fn label_in(superblock: &[u8; SUPERBLOCK_SIZE]) -> Option<String> {
    let magic = u16::from_le_bytes([superblock[EXT_MAGIC_AT], superblock[EXT_MAGIC_AT + 1]]);
    if magic != EXT_MAGIC {
        return None;
    }
    let name = &superblock[EXT_LABEL_AT];
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    Some(String::from_utf8_lossy(&name[..end]).into_owned())
}

/// Nothing at all in the first megabyte: no partition table, no file
/// system, no boot code. Only such a disk is ever written to.
fn is_blank(device: &Path) -> bool {
    let Ok(mut file) = File::open(device) else {
        return false;
    };
    let mut head = vec![0u8; 1 << 20];
    file.read_exact(&mut head).is_ok() && head.iter().all(|&b| b == 0)
}

/// One partition across `disk`, with Saule's file system on it.
fn make_on(disk: &Disk) -> Option<PathBuf> {
    log::info(&format!(
        "{} is empty: making it Saule's data disk ({} MB)",
        disk.device.display(),
        disk.sectors / 2048
    ));
    if let Err(error) = write_partition_table(disk) {
        log::warn(&format!(
            "could not partition {}: {error}",
            disk.device.display()
        ));
        return None;
    }
    let partition = Path::new("/dev").join(format!("{}1", disk.name));
    // The kernel reads the new table and makes the partition's device.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !partition.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if !partition.exists() {
        log::warn(&format!("{} did not appear", partition.display()));
        return None;
    }
    format(&partition).then_some(partition)
}

/// Make Saule's file system on `partition`; whether it was made.
fn format(partition: &Path) -> bool {
    // One inode per megabyte: a home of documents and settings needs far
    // fewer than the default, and their tables cost a 250 GB disk gigabytes.
    let mut command = if Path::new(MKE2FS).exists() {
        let mut command = Command::new(MKE2FS);
        // The tables and the journal are zeroed by the kernel once the
        // file system is in use, not now: a 250 GB disk is ready at once.
        command.args([
            "-t",
            "ext4",
            "-F",
            "-E",
            "lazy_itable_init=1,lazy_journal_init=1",
        ]);
        command
    } else if Path::new(MKE2FS_BUSYBOX).exists() {
        Command::new(MKE2FS_BUSYBOX)
    } else {
        log::warn("no mke2fs in this image: the disk stays unformatted");
        return false;
    };
    let status = command
        .args(["-q", "-L", LABEL, "-m", "1", "-i", "1048576"])
        .arg(partition)
        .stdin(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            log::warn(&format!("mke2fs failed ({status})"));
            false
        }
        Err(error) => {
            log::warn(&format!("could not run mke2fs: {error}"));
            false
        }
    }
}

/// Write the partition table to `disk` and have the kernel read it.
fn write_partition_table(disk: &Disk) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_SYNC)
        .open(&disk.device)?;
    file.write_all(&partition_table(disk.sectors))?;
    file.sync_all()?;

    // SAFETY: an ioctl with no argument, on a block device opened here.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), BLKRRPART) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// The first sector of a disk of `sectors` sectors: an MBR with one Linux
/// partition from the first megabyte to the end. An MBR counts sectors in
/// 32 bits, so the partition stops at 2 TiB on a larger disk.
fn partition_table(sectors: u64) -> [u8; 512] {
    let end = sectors.min(u64::from(u32::MAX)) as u32;
    let count = end.saturating_sub(PARTITION_START);
    // Past what cylinders, heads and sectors can say: every tool then
    // goes by the sector numbers after them.
    const CHS_LIMIT: [u8; 3] = [0xFE, 0xFF, 0xFF];

    let mut mbr = [0u8; 512];
    // A disk signature, for tools that want one.
    mbr[440..444].copy_from_slice(b"SAUL");

    let entry = &mut mbr[446..462];
    entry[0] = 0x00; // not bootable
    entry[1..4].copy_from_slice(&CHS_LIMIT);
    entry[4] = 0x83; // Linux
    entry[5..8].copy_from_slice(&CHS_LIMIT);
    entry[8..12].copy_from_slice(&PARTITION_START.to_le_bytes());
    entry[12..16].copy_from_slice(&count.to_le_bytes());

    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    mbr
}

fn mount(device: &Path) -> std::io::Result<()> {
    fs::create_dir_all(MOUNT)?;
    let source = CString::new(device.as_os_str().as_encoded_bytes())?;
    let target = CString::new(MOUNT)?;
    let fstype = CString::new("ext4")?;
    // SAFETY: NUL-terminated strings that outlive the call.
    let result = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype.as_ptr(),
            MOUNT_FLAGS,
            std::ptr::null(),
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn superblock(magic: u16, label: &[u8]) -> [u8; SUPERBLOCK_SIZE] {
        let mut block = [0u8; SUPERBLOCK_SIZE];
        block[EXT_MAGIC_AT..EXT_MAGIC_AT + 2].copy_from_slice(&magic.to_le_bytes());
        block[EXT_LABEL_AT.start..EXT_LABEL_AT.start + label.len()].copy_from_slice(label);
        block
    }

    #[test]
    fn label_of_an_ext_file_system() {
        assert_eq!(
            label_in(&superblock(EXT_MAGIC, b"SAULE")).as_deref(),
            Some("SAULE")
        );
    }

    #[test]
    fn a_label_may_fill_all_sixteen_bytes() {
        let label = b"0123456789abcdef";
        assert_eq!(
            label_in(&superblock(EXT_MAGIC, label)).as_deref(),
            Some("0123456789abcdef")
        );
    }

    #[test]
    fn an_ext_file_system_without_a_label_has_an_empty_one() {
        assert_eq!(label_in(&superblock(EXT_MAGIC, b"")).as_deref(), Some(""));
    }

    #[test]
    fn anything_else_has_no_label() {
        // A label where ext keeps it, but not ext's magic number: NTFS,
        // FAT, or an empty disk must never be taken for Saule's.
        assert_eq!(label_in(&superblock(0, b"SAULE")), None);
        assert_eq!(label_in(&superblock(0xAA55, b"SAULE")), None);
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn a_machine_without_disks_is_not_kept_waiting_long() {
        assert!(worth_waiting(MS(0), None, false));
        assert!(worth_waiting(MS(1400), None, false));
        assert!(!worth_waiting(MS(1500), None, false));
    }

    #[test]
    fn a_disk_gets_a_moment_more_for_its_partitions() {
        assert!(worth_waiting(MS(300), Some(MS(0)), false));
        assert!(!worth_waiting(MS(500), Some(MS(0)), false));
        // One that showed up late still gets its moment.
        assert!(worth_waiting(MS(1700), Some(MS(1400)), false));
        assert!(!worth_waiting(MS(1900), Some(MS(1400)), false));
    }

    #[test]
    fn a_usb_drive_on_its_way_is_waited_for() {
        assert!(worth_waiting(MS(3900), None, true));
        assert!(worth_waiting(MS(3900), Some(MS(0)), true));
        assert!(!worth_waiting(MS(4000), None, true));
    }

    #[test]
    fn a_disk_on_a_usb_port_is_told_from_one_inside() {
        let stick = "/sys/devices/pci0000:00/0000:00:1d.7/usb1/1-1/1-1:1.0/host2/target2:0:0/2:0:0:0/block/sdb";
        let sata = "/sys/devices/pci0000:00/0000:00:1f.2/ata1/host0/target0:0:0/0:0:0:0/block/sda";
        assert!(on_usb(Path::new(stick)));
        assert!(!on_usb(Path::new(sata)));
        // A name that merely starts the same is not a USB root hub.
        assert!(!on_usb(Path::new("/sys/devices/platform/usbphy/block/sda")));
    }

    /// The netbook's 250 GB disk.
    const NETBOOK_SECTORS: u64 = 488_397_168;

    fn entry_u32(mbr: &[u8; 512], at: usize) -> u32 {
        u32::from_le_bytes(mbr[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn the_partition_fills_the_disk_from_the_first_megabyte() {
        let mbr = partition_table(NETBOOK_SECTORS);
        assert_eq!(mbr[446], 0x00, "not bootable");
        assert_eq!(mbr[450], 0x83, "a Linux partition");
        assert_eq!(entry_u32(&mbr, 454), 2048);
        assert_eq!(u64::from(entry_u32(&mbr, 458)), NETBOOK_SECTORS - 2048);
        assert_eq!(&mbr[510..], &[0x55, 0xAA]);
    }

    #[test]
    fn there_is_only_one_partition() {
        let mbr = partition_table(NETBOOK_SECTORS);
        assert!(mbr[462..510].iter().all(|&b| b == 0));
    }

    #[test]
    fn a_disk_past_two_tebibytes_gets_what_an_mbr_can_hold() {
        let mbr = partition_table(8_000_000_000);
        assert_eq!(entry_u32(&mbr, 458), u32::MAX - 2048);
    }

    #[test]
    fn a_disk_smaller_than_the_first_megabyte_gets_an_empty_partition() {
        let mbr = partition_table(1000);
        assert_eq!(entry_u32(&mbr, 458), 0);
    }

    #[test]
    fn a_partition_table_is_not_a_blank_disk() {
        // What `is_blank` looks for: once written, the disk is never
        // formatted again.
        assert!(partition_table(NETBOOK_SECTORS).iter().any(|&b| b != 0));
    }
}

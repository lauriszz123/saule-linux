//! The disks of this machine, and what is on each.
//!
//! What the installer shows before anything is erased: a disk's name and
//! size, and whether it is empty, has Saule Linux on it already, or has
//! something else — Windows, above all, which the netbook shipped with.
//! Nothing here writes.
//!
//! A disk is told by two kinds of mark. The file systems on it, by how
//! each begins; and the partition table's own word for each partition,
//! which is still there when the system on it no longer starts and the
//! file system's beginning is gone. A GPT keeps a copy of itself at the
//! end of the disk, which is read when the one at the start is damaged.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::layout::{self, Partition, DATA_LABEL, MEDIUM_LABEL, SECTOR, SYSTEM_LABEL};

/// What a disk holds, as far as it matters to someone about to erase it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contents {
    /// Nothing at all in its first megabyte.
    Blank,
    /// Saule Linux, installed: its system and its data.
    System,
    /// Saule's data alone, as a machine booted from elsewhere made it.
    Data,
    Windows,
    Linux,
    /// Both, side by side: a machine that starts either.
    WindowsAndLinux,
    /// Something, of a kind not recognised, or a disk that can't be read.
    Other,
    /// The stick this system was started from.
    Medium,
}

impl Contents {
    pub fn word(self) -> &'static str {
        match self {
            Contents::Blank => "blank",
            Contents::System => "saule",
            Contents::Data => "saule-data",
            Contents::Windows => "windows",
            Contents::Linux => "linux",
            Contents::WindowsAndLinux => "windows-linux",
            Contents::Other => "other",
            Contents::Medium => "medium",
        }
    }
}

/// A file system, told by the marks it leaves at its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileSystem {
    /// ext2, 3 or 4, with its label.
    Ext(String),
    Ntfs,
    Fat,
    /// What only a Linux system makes: XFS, Btrfs, swap, LVM, LUKS.
    Linux,
}

/// What one partition says of the disk: the file system found in it, or
/// failing that whose kind of partition the table says it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    Holds(FileSystem),
    /// A kind of partition Windows makes for itself.
    ForWindows,
    /// A kind of partition Linux makes for itself.
    ForLinux,
}

/// A partition as its table lists it: where it starts, in bytes, and whose
/// the table says it is, when it says.
struct Listed {
    start: u64,
    kind: Option<Mark>,
}

pub struct Disk {
    pub name: String,
    pub device: PathBuf,
    pub model: String,
    pub sectors: u64,
    /// On a USB port: a stick or an enclosure.
    pub usb: bool,
    pub contents: Contents,
}

impl Disk {
    pub fn bytes(&self) -> u64 {
        self.sectors * SECTOR
    }

    /// One line for the installer's window: tab-separated, the model last
    /// since it is the one part that has spaces in it.
    pub fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}",
            self.name,
            self.bytes(),
            if self.usb { "usb" } else { "internal" },
            self.contents.word(),
            self.model
        )
    }
}

/// Every disk, in the kernel's order.
pub fn all() -> Vec<Disk> {
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return Vec::new();
    };

    let mut disks = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_disk(&name) {
            continue;
        }
        let read = |file: &str| fs::read_to_string(entry.path().join(file)).unwrap_or_default();
        let sectors = read("size").trim().parse().unwrap_or(0);
        if sectors == 0 {
            continue;
        }

        let device = Path::new("/dev").join(&name);
        let usb = fs::canonicalize(entry.path()).is_ok_and(|path| on_usb(&path));
        let contents = File::open(&device)
            .ok()
            .and_then(|mut file| contents_of(&mut file))
            .unwrap_or(Contents::Other);

        disks.push(Disk {
            model: model(&read("device/vendor"), &read("device/model")),
            device,
            sectors,
            usb,
            contents,
            name,
        });
    }

    disks.sort_by(|a, b| a.name.cmp(&b.name));
    disks
}

/// Whether the block device called `name` is a disk a system could be
/// installed on: SATA and USB (`sda`), virtio (`vda`), NVMe (`nvme0n1`)
/// and eMMC (`mmcblk0`, but not its `mmcblk0boot0` and `mmcblk0rpmb`,
/// which are parts of the chip a system has no business in).
fn is_disk(name: &str) -> bool {
    if let Some(number) = name.strip_prefix("mmcblk") {
        return !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
    }
    name.starts_with("sd") || name.starts_with("vd") || name.starts_with("nvme")
}

/// The disk called `name`, if this machine has it.
pub fn find(name: &str) -> Option<Disk> {
    all().into_iter().find(|disk| disk.name == name)
}

/// A disk's name as its maker gave it. SATA disks say it all in the model
/// ("Hitachi HTS545025B9A300") and "ATA" as the vendor; USB sticks split
/// it in two.
fn model(vendor: &str, model: &str) -> String {
    let vendor = vendor.trim();
    let model = model.trim();
    match vendor {
        "" | "ATA" => model.to_string(),
        _ if model.is_empty() => vendor.to_string(),
        _ => format!("{vendor} {model}"),
    }
}

/// Whether the device at sysfs `path` hangs off a USB controller, whose
/// root hubs are `usb1`, `usb2`, ….
fn on_usb(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str()
            .to_str()
            .and_then(|name| name.strip_prefix("usb"))
            .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// What is on the disk `file` is opened on; `None` when it can't be read.
pub fn contents_of(file: &mut File) -> Option<Contents> {
    let mut head = vec![0u8; 1 << 20];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut head).ok()?;

    let listed = partitions(file, &head);
    if listed.is_empty() && head.iter().all(|&b| b == 0) {
        return Some(Contents::Blank);
    }

    let mut marks = Vec::new();
    for partition in &listed {
        match file_system_at(file, partition.start) {
            Some(system) => marks.push(Mark::Holds(system)),
            None => marks.extend(partition.kind.clone()),
        }
    }
    // A file system straight on the disk, with no partitions.
    if listed.is_empty() {
        marks.extend(file_system_at(file, 0).map(Mark::Holds));
    }

    Some(classify(&marks))
}

/// What a disk with these marks is, to someone choosing one.
pub fn classify(marks: &[Mark]) -> Contents {
    let labelled = |label: &str| {
        marks
            .iter()
            .any(|m| matches!(m, Mark::Holds(FileSystem::Ext(l)) if l == label))
    };
    let windows = marks
        .iter()
        .any(|m| matches!(m, Mark::Holds(FileSystem::Ntfs) | Mark::ForWindows));
    let linux = marks.iter().any(|m| {
        matches!(
            m,
            Mark::Holds(FileSystem::Ext(_) | FileSystem::Linux) | Mark::ForLinux
        )
    });

    if labelled(MEDIUM_LABEL) {
        Contents::Medium
    } else if labelled(SYSTEM_LABEL) && labelled(DATA_LABEL) {
        Contents::System
    } else if labelled(DATA_LABEL) {
        Contents::Data
    } else if windows && linux {
        Contents::WindowsAndLinux
    } else if windows {
        Contents::Windows
    } else if linux {
        Contents::Linux
    } else {
        Contents::Other
    }
}

/// The disk's partitions: from the MBR, or from the GPT when the MBR is
/// only there to protect one, or has been lost and the GPT's copy at the
/// end of the disk has not.
fn partitions(file: &mut File, head: &[u8]) -> Vec<Listed> {
    let mut sector = [0u8; 512];
    sector.copy_from_slice(&head[..512]);
    let from_mbr = layout::partitions_in(&sector);

    let protective = from_mbr.iter().any(|(kind, _)| *kind == 0xEE);
    if protective || from_mbr.is_empty() {
        let from_gpt = gpt_at_start(file, head).or_else(|| gpt_at_end(file));
        if let Some(listed) = from_gpt {
            return listed;
        }
        if protective {
            return Vec::new();
        }
    }

    from_mbr
        .iter()
        .map(|(kind, partition)| Listed {
            start: partition.offset(),
            kind: mbr_kind(*kind),
        })
        .collect()
}

/// Whose an MBR's partition type says a partition is.
fn mbr_kind(kind: u8) -> Option<Mark> {
    match kind {
        // NTFS or exFAT, Windows' recovery, its dynamic disks.
        0x07 | 0x27 | 0x42 => Some(Mark::ForWindows),
        // Swap, a Linux file system, LVM, software RAID.
        0x82 | 0x83 | 0x8E | 0xFD => Some(Mark::ForLinux),
        _ => None,
    }
}

/// Whose a GPT's partition type says a partition is: the type is a GUID,
/// kept with its first three parts byte-swapped.
fn gpt_kind(kind: &[u8]) -> Option<Mark> {
    const WINDOWS: [[u8; 16]; 3] = [
        // Basic data: EBD0A0A2-B9E5-4433-87C0-68B6B72699C7
        [
            0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26,
            0x99, 0xC7,
        ],
        // Microsoft reserved: E3C9E316-0B5C-4DB8-817D-F92DF00215AE
        [
            0x16, 0xE3, 0xC9, 0xE3, 0x5C, 0x0B, 0xB8, 0x4D, 0x81, 0x7D, 0xF9, 0x2D, 0xF0, 0x02,
            0x15, 0xAE,
        ],
        // Windows recovery: DE94BBA4-06D1-4D40-A16A-BFD50179D6AC
        [
            0xA4, 0xBB, 0x94, 0xDE, 0xD1, 0x06, 0x40, 0x4D, 0xA1, 0x6A, 0xBF, 0xD5, 0x01, 0x79,
            0xD6, 0xAC,
        ],
    ];
    const LINUX: [[u8; 16]; 5] = [
        // Linux file system: 0FC63DAF-8483-4772-8E79-3D69D8477DE4
        [
            0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47,
            0x7D, 0xE4,
        ],
        // Linux swap: 0657FD6D-A4AB-43C4-84E5-0933C84B4F4F
        [
            0x6D, 0xFD, 0x57, 0x06, 0xAB, 0xA4, 0xC4, 0x43, 0x84, 0xE5, 0x09, 0x33, 0xC8, 0x4B,
            0x4F, 0x4F,
        ],
        // Linux LVM: E6D6D379-F507-44C2-A23C-238F2A3DF928
        [
            0x79, 0xD3, 0xD6, 0xE6, 0x07, 0xF5, 0xC2, 0x44, 0xA2, 0x3C, 0x23, 0x8F, 0x2A, 0x3D,
            0xF9, 0x28,
        ],
        // Linux root, x86-64: 4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709
        [
            0xE3, 0xBC, 0x68, 0x4F, 0xCD, 0xE8, 0xB1, 0x4D, 0x96, 0xE7, 0xFB, 0xCA, 0xF9, 0x84,
            0xB7, 0x09,
        ],
        // LUKS: CA7D7CCB-63ED-4C53-861C-1742536059CC
        [
            0xCB, 0x7C, 0x7D, 0xCA, 0xED, 0x63, 0x53, 0x4C, 0x86, 0x1C, 0x17, 0x42, 0x53, 0x60,
            0x59, 0xCC,
        ],
    ];
    if WINDOWS.iter().any(|guid| guid == kind) {
        Some(Mark::ForWindows)
    } else if LINUX.iter().any(|guid| guid == kind) {
        Some(Mark::ForLinux)
    } else {
        None
    }
}

/// The GPT in the disk's second sector, if it is whole enough to read.
fn gpt_at_start(file: &mut File, head: &[u8]) -> Option<Vec<Listed>> {
    let mut header = [0u8; 512];
    header.copy_from_slice(&head[512..1024]);
    gpt_entries(file, &header)
}

/// The copy of the GPT in the disk's last sector.
fn gpt_at_end(file: &mut File) -> Option<Vec<Listed>> {
    let end = file.seek(SeekFrom::End(0)).ok()?;
    if end < 2 * SECTOR {
        return None;
    }
    let mut header = [0u8; 512];
    file.seek(SeekFrom::Start(end - SECTOR)).ok()?;
    file.read_exact(&mut header).ok()?;
    gpt_entries(file, &header)
}

/// The partitions a GPT header leads to: it says where its entries are;
/// an entry whose type is all zeros is unused.
fn gpt_entries(file: &mut File, header: &[u8; 512]) -> Option<Vec<Listed>> {
    if &header[..8] != b"EFI PART" {
        return None;
    }
    let le64 = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
    let le32 = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
    let entries_at = le64(72).checked_mul(SECTOR)?;
    let count = le32(80).min(128) as usize;
    let size = le32(84) as usize;
    if !(128..=512).contains(&size) {
        return None;
    }

    let mut entries = vec![0u8; count * size];
    file.seek(SeekFrom::Start(entries_at)).ok()?;
    file.read_exact(&mut entries).ok()?;
    Some(
        entries
            .chunks(size)
            .filter(|entry| entry[..16].iter().any(|&b| b != 0))
            .map(|entry| Listed {
                start: u64::from_le_bytes(entry[32..40].try_into().unwrap()).saturating_mul(SECTOR),
                kind: gpt_kind(&entry[..16]),
            })
            .collect(),
    )
}

/// How much of a file system's start is looked at: Btrfs keeps its mark
/// 64 KiB in.
const START: usize = 0x10048;

/// The file system that starts `offset` bytes into `file`, if it is one of
/// those told apart here.
pub fn file_system_at(file: &mut File, offset: u64) -> Option<FileSystem> {
    let mut start = vec![0u8; START];
    file.seek(SeekFrom::Start(offset)).ok()?;
    // A partition shorter than this (none worth the name is) reads short.
    let mut filled = 0;
    while filled < START {
        match file.read(&mut start[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(count) => filled += count,
        }
    }
    file_system_in(&start)
}

/// The same, from the first [`START`] bytes of it.
pub fn file_system_in(start: &[u8]) -> Option<FileSystem> {
    let at = |offset: usize, mark: &[u8]| start.get(offset..offset + mark.len()) == Some(mark);

    // ext: a superblock a kilobyte in, with its magic number and its name.
    if at(1024 + 0x38, &[0x53, 0xEF]) {
        let name = &start[1024 + 0x78..1024 + 0x88];
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        return Some(FileSystem::Ext(
            String::from_utf8_lossy(&name[..end]).into_owned(),
        ));
    }
    if at(3, b"NTFS    ") {
        return Some(FileSystem::Ntfs);
    }
    if at(3, b"EXFAT   ") || at(82, b"FAT32") || at(54, b"FAT") {
        return Some(FileSystem::Fat);
    }
    let linux = at(0, b"XFSB")
        || at(0, b"LUKS\xBA\xBE")
        || at(512, b"LABELONE")
        || at(4086, b"SWAPSPACE2")
        || at(4086, b"SWAP-SPACE")
        || at(0x10040, b"_BHRfS_M");
    linux.then_some(FileSystem::Linux)
}

/// The partition of `disk` whose ext file system is labelled `label`: its
/// number and where it is.
pub fn labelled_partition(disk: &Disk, label: &str) -> Option<(u32, Partition)> {
    let mut file = File::open(&disk.device).ok()?;
    let mut sector = [0u8; 512];
    file.read_exact(&mut sector).ok()?;

    layout::partitions_in(&sector)
        .into_iter()
        .enumerate()
        .find(|(_, (_, partition))| {
            file_system_at(&mut file, partition.offset())
                == Some(FileSystem::Ext(label.to_string()))
        })
        .map(|(index, (_, partition))| (index as u32 + 1, partition))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn ext(label: &str) -> Vec<u8> {
        let mut start = vec![0u8; 2048];
        start[1024 + 0x38] = 0x53;
        start[1024 + 0x39] = 0xEF;
        start[1024 + 0x78..1024 + 0x78 + label.len()].copy_from_slice(label.as_bytes());
        start
    }

    fn ntfs() -> Vec<u8> {
        let mut start = vec![0u8; 2048];
        start[3..11].copy_from_slice(b"NTFS    ");
        start
    }

    fn marked(at: usize, mark: &[u8]) -> Vec<u8> {
        let mut start = vec![0u8; START];
        start[at..at + mark.len()].copy_from_slice(mark);
        start
    }

    #[test]
    fn file_systems_are_told_by_their_marks() {
        assert_eq!(
            file_system_in(&ext("SAULE")),
            Some(FileSystem::Ext("SAULE".into()))
        );
        assert_eq!(
            file_system_in(&ext("")),
            Some(FileSystem::Ext(String::new()))
        );
        assert_eq!(file_system_in(&ntfs()), Some(FileSystem::Ntfs));
        assert_eq!(file_system_in(&marked(82, b"FAT32")), Some(FileSystem::Fat));
        assert_eq!(file_system_in(&[0u8; 2048]), None);
    }

    #[test]
    fn what_only_linux_makes_is_linux_s() {
        for (at, mark) in [
            (0, &b"XFSB"[..]),
            (0, &b"LUKS\xBA\xBE"[..]),
            (512, &b"LABELONE"[..]),
            (4086, &b"SWAPSPACE2"[..]),
            (0x10040, &b"_BHRfS_M"[..]),
        ] {
            assert_eq!(file_system_in(&marked(at, mark)), Some(FileSystem::Linux));
        }
        // The same marks cut short are none.
        assert_eq!(file_system_in(&marked(4086, b"SWAPSPACE2")[..4090]), None);
    }

    #[test]
    fn a_disk_is_what_matters_most_on_it() {
        let ext = |label: &str| Mark::Holds(FileSystem::Ext(label.to_string()));
        let ntfs = Mark::Holds(FileSystem::Ntfs);
        let fat = Mark::Holds(FileSystem::Fat);

        assert_eq!(
            classify(&[ext("SAULE-BOOT"), ext("SAULE")]),
            Contents::System
        );
        assert_eq!(classify(&[ext("SAULE")]), Contents::Data);
        assert_eq!(classify(&[fat.clone(), ntfs.clone()]), Contents::Windows);
        assert_eq!(classify(&[ext("root"), fat.clone()]), Contents::Linux);
        assert_eq!(classify(&[Mark::Holds(FileSystem::Linux)]), Contents::Linux);
        assert_eq!(
            classify(&[ntfs.clone(), ext("root")]),
            Contents::WindowsAndLinux
        );
        assert_eq!(classify(&[fat]), Contents::Other);
        assert_eq!(classify(&[]), Contents::Other);
        // The stick itself, whatever else is on it.
        assert_eq!(classify(&[ext("SAULE-INSTALL"), ntfs]), Contents::Medium);
    }

    #[test]
    fn a_partition_s_kind_speaks_when_its_file_system_can_t() {
        assert_eq!(classify(&[Mark::ForWindows]), Contents::Windows);
        assert_eq!(classify(&[Mark::ForLinux]), Contents::Linux);
        assert_eq!(
            classify(&[Mark::ForWindows, Mark::ForLinux]),
            Contents::WindowsAndLinux
        );
        assert_eq!(mbr_kind(0x07), Some(Mark::ForWindows));
        assert_eq!(mbr_kind(0x83), Some(Mark::ForLinux));
        assert_eq!(mbr_kind(0x0C), None);
    }

    /// A disk image in a file, 64 MB, empty.
    fn image(name: &str) -> File {
        let path =
            std::env::temp_dir().join(format!("saule-install-{name}-{}", std::process::id()));
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        file.set_len(64 << 20).unwrap();
        file
    }

    /// One with an MBR: `partitions` as (type, start in sectors, the start
    /// of what is in it).
    fn mbr_image(name: &str, partitions: &[(u8, u32, Vec<u8>)]) -> File {
        let mut file = image(name);
        let mut sector = [0u8; 512];
        for (i, (kind, start, _)) in partitions.iter().enumerate() {
            let entry = &mut sector[446 + i * 16..462 + i * 16];
            entry[4] = *kind;
            entry[8..12].copy_from_slice(&start.to_le_bytes());
            entry[12..16].copy_from_slice(&2048u32.to_le_bytes());
        }
        sector[510] = 0x55;
        sector[511] = 0xAA;
        file.write_all(&sector).unwrap();
        for (_, start, bytes) in partitions {
            file.seek(SeekFrom::Start(u64::from(*start) * 512)).unwrap();
            file.write_all(bytes).unwrap();
        }
        file
    }

    /// The Linux file system type, as a GPT keeps it.
    const LINUX_GUID: [u8; 16] = [
        0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D,
        0xE4,
    ];
    /// Microsoft's basic data type.
    const WINDOWS_GUID: [u8; 16] = [
        0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99,
        0xC7,
    ];

    /// One with a GPT of one partition of type `kind` at sector 2048,
    /// written at the start of the disk, at its end, or both.
    fn gpt_image(name: &str, kind: [u8; 16], at_start: bool, at_end: bool) -> File {
        let mut file = image(name);
        let sectors = (64u64 << 20) / 512;

        let mut protective = [0u8; 512];
        protective[446 + 4] = 0xEE;
        protective[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
        protective[446 + 12..446 + 16].copy_from_slice(&(sectors as u32 - 1).to_le_bytes());
        protective[510] = 0x55;
        protective[511] = 0xAA;
        file.write_all(&protective).unwrap();

        let mut entry = [0u8; 128];
        entry[..16].copy_from_slice(&kind);
        entry[32..40].copy_from_slice(&2048u64.to_le_bytes());
        let header = |entries_at: u64| {
            let mut header = [0u8; 512];
            header[..8].copy_from_slice(b"EFI PART");
            header[72..80].copy_from_slice(&entries_at.to_le_bytes());
            header[80..84].copy_from_slice(&1u32.to_le_bytes());
            header[84..88].copy_from_slice(&128u32.to_le_bytes());
            header
        };
        if at_start {
            file.seek(SeekFrom::Start(512)).unwrap();
            file.write_all(&header(2)).unwrap();
            file.write_all(&entry).unwrap();
        }
        if at_end {
            file.seek(SeekFrom::Start((sectors - 2) * 512)).unwrap();
            file.write_all(&entry).unwrap();
            file.seek(SeekFrom::Start((sectors - 1) * 512)).unwrap();
            file.write_all(&header(sectors - 2)).unwrap();
        }
        file
    }

    #[test]
    fn an_empty_disk_is_blank() {
        assert_eq!(contents_of(&mut image("blank")), Some(Contents::Blank));
    }

    #[test]
    fn a_disk_with_windows_says_so() {
        let mut disk = mbr_image(
            "windows",
            &[(0x0C, 2048, vec![0u8; 512]), (0x07, 4096, ntfs())],
        );
        assert_eq!(contents_of(&mut disk), Some(Contents::Windows));
    }

    #[test]
    fn an_installed_disk_and_a_data_disk_are_told_apart() {
        let mut installed = mbr_image(
            "installed",
            &[(0x83, 2048, ext("SAULE-BOOT")), (0x83, 8192, ext("SAULE"))],
        );
        assert_eq!(contents_of(&mut installed), Some(Contents::System));

        let mut data = mbr_image("data", &[(0x83, 2048, ext("SAULE"))]);
        assert_eq!(contents_of(&mut data), Some(Contents::Data));
    }

    #[test]
    fn partitions_with_nothing_known_in_them_are_something() {
        let mut disk = mbr_image("other", &[(0x0C, 2048, vec![0u8; 512])]);
        assert_eq!(contents_of(&mut disk), Some(Contents::Other));
    }

    #[test]
    fn a_windows_whose_file_systems_are_gone_is_still_windows() {
        // The partitions are listed as Windows'; what was in them is noise.
        let mut disk = mbr_image(
            "broken",
            &[
                (0x07, 2048, vec![0xA5; 4096]),
                (0x27, 8192, vec![0x5A; 4096]),
            ],
        );
        assert_eq!(contents_of(&mut disk), Some(Contents::Windows));
    }

    #[test]
    fn windows_and_linux_side_by_side_are_both_said() {
        let mut disk = mbr_image("both", &[(0x07, 2048, ntfs()), (0x83, 8192, ext("root"))]);
        assert_eq!(contents_of(&mut disk), Some(Contents::WindowsAndLinux));
    }

    #[test]
    fn a_gpt_is_read_from_the_start_of_the_disk() {
        let mut disk = gpt_image("gpt", WINDOWS_GUID, true, true);
        assert_eq!(contents_of(&mut disk), Some(Contents::Windows));
    }

    #[test]
    fn a_gpt_lost_at_the_start_is_read_from_its_copy_at_the_end() {
        let mut disk = gpt_image("gpt-end", LINUX_GUID, false, true);
        assert_eq!(contents_of(&mut disk), Some(Contents::Linux));
    }

    #[test]
    fn a_protective_mbr_with_no_gpt_anywhere_is_something() {
        let mut disk = gpt_image("gpt-none", LINUX_GUID, false, false);
        assert_eq!(contents_of(&mut disk), Some(Contents::Other));
    }

    #[test]
    fn a_disk_of_noise_is_something_not_nothing() {
        let mut disk = image("noise");
        disk.write_all(&[0x7Fu8; 8192]).unwrap();
        assert_eq!(contents_of(&mut disk), Some(Contents::Other));
    }

    #[test]
    fn disks_are_told_from_the_other_block_devices() {
        for name in ["sda", "vdb", "nvme0n1", "mmcblk0"] {
            assert!(is_disk(name), "{name}");
        }
        for name in [
            "loop0",
            "ram0",
            "sr0",
            "mmcblk0boot0",
            "mmcblk0rpmb",
            "mmcblk",
        ] {
            assert!(!is_disk(name), "{name}");
        }
    }

    #[test]
    fn a_disk_s_name_is_its_maker_s() {
        assert_eq!(model("ATA     ", "Hitachi HTS54502\n"), "Hitachi HTS54502");
        assert_eq!(model("SanDisk", "Cruzer Blade"), "SanDisk Cruzer Blade");
        assert_eq!(model("QEMU", ""), "QEMU");
        assert_eq!(model("", ""), "");
    }
}

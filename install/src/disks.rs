//! The disks of this machine, and what is on each.
//!
//! What the installer shows before anything is erased: a disk's name and
//! size, and whether it is empty, has Saule Linux on it already, or has
//! something else — Windows, above all, which the netbook shipped with.
//! Nothing here writes.

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
        if !(name.starts_with("sd") || name.starts_with("vd") || name.starts_with("nvme")) {
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
    if head.iter().all(|&b| b == 0) {
        return Some(Contents::Blank);
    }

    let mut systems = Vec::new();
    for start in partition_starts(file, &head) {
        if let Some(system) = file_system_at(file, start) {
            systems.push(system);
        }
    }
    // A file system straight on the disk, with no partitions.
    if systems.is_empty() {
        systems.extend(file_system_at(file, 0));
    }

    Some(classify(&systems))
}

/// What a disk with these file systems is, to someone choosing one.
pub fn classify(systems: &[FileSystem]) -> Contents {
    let labelled = |label: &str| {
        systems
            .iter()
            .any(|s| matches!(s, FileSystem::Ext(l) if l == label))
    };

    if labelled(MEDIUM_LABEL) {
        Contents::Medium
    } else if labelled(SYSTEM_LABEL) && labelled(DATA_LABEL) {
        Contents::System
    } else if labelled(DATA_LABEL) {
        Contents::Data
    } else if systems.contains(&FileSystem::Ntfs) {
        Contents::Windows
    } else if systems.iter().any(|s| matches!(s, FileSystem::Ext(_))) {
        Contents::Linux
    } else {
        Contents::Other
    }
}

/// Where each partition starts, in bytes: from the MBR, or from the GPT
/// when the MBR is only there to protect one.
fn partition_starts(file: &mut File, head: &[u8]) -> Vec<u64> {
    let mut sector = [0u8; 512];
    sector.copy_from_slice(&head[..512]);
    let partitions = layout::partitions_in(&sector);

    if partitions.iter().any(|(kind, _)| *kind == 0xEE) {
        return gpt_starts(file, head);
    }
    partitions.iter().map(|(_, p)| p.offset()).collect()
}

/// A GPT's partitions: the header in the disk's second sector says where
/// the entries are; an entry whose type is all zeros is unused.
fn gpt_starts(file: &mut File, head: &[u8]) -> Vec<u64> {
    let header = &head[512..1024];
    if &header[..8] != b"EFI PART" {
        return Vec::new();
    }
    let le64 = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
    let le32 = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
    let entries_at = le64(72) * SECTOR;
    let count = le32(80).min(128) as usize;
    let size = le32(84) as usize;
    if !(128..=512).contains(&size) {
        return Vec::new();
    }

    let mut entries = vec![0u8; count * size];
    if file.seek(SeekFrom::Start(entries_at)).is_err() || file.read_exact(&mut entries).is_err() {
        return Vec::new();
    }
    entries
        .chunks(size)
        .filter(|entry| entry[..16].iter().any(|&b| b != 0))
        .map(|entry| u64::from_le_bytes(entry[32..40].try_into().unwrap()) * SECTOR)
        .collect()
}

/// The file system that starts `offset` bytes into `file`, if it is one of
/// those told apart here.
pub fn file_system_at(file: &mut File, offset: u64) -> Option<FileSystem> {
    let mut start = [0u8; 2048];
    file.seek(SeekFrom::Start(offset)).ok()?;
    file.read_exact(&mut start).ok()?;
    file_system_in(&start)
}

/// The same, from the first two kilobytes of it.
pub fn file_system_in(start: &[u8; 2048]) -> Option<FileSystem> {
    // ext: a superblock a kilobyte in, with its magic number and its name.
    if start[1024 + 0x38..1024 + 0x3A] == [0x53, 0xEF] {
        let name = &start[1024 + 0x78..1024 + 0x88];
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        return Some(FileSystem::Ext(
            String::from_utf8_lossy(&name[..end]).into_owned(),
        ));
    }
    if &start[3..11] == b"NTFS    " {
        return Some(FileSystem::Ntfs);
    }
    if &start[3..11] == b"EXFAT   " || &start[82..87] == b"FAT32" || &start[54..57] == b"FAT" {
        return Some(FileSystem::Fat);
    }
    None
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

    fn ext(label: &str) -> [u8; 2048] {
        let mut start = [0u8; 2048];
        start[1024 + 0x38] = 0x53;
        start[1024 + 0x39] = 0xEF;
        start[1024 + 0x78..1024 + 0x78 + label.len()].copy_from_slice(label.as_bytes());
        start
    }

    fn ntfs() -> [u8; 2048] {
        let mut start = [0u8; 2048];
        start[3..11].copy_from_slice(b"NTFS    ");
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

        let mut fat = [0u8; 2048];
        fat[82..87].copy_from_slice(b"FAT32");
        assert_eq!(file_system_in(&fat), Some(FileSystem::Fat));
        assert_eq!(file_system_in(&[0u8; 2048]), None);
    }

    #[test]
    fn a_disk_is_what_matters_most_on_it() {
        let ext = |label: &str| FileSystem::Ext(label.to_string());
        assert_eq!(
            classify(&[ext("SAULE-BOOT"), ext("SAULE")]),
            Contents::System
        );
        assert_eq!(classify(&[ext("SAULE")]), Contents::Data);
        assert_eq!(
            classify(&[FileSystem::Fat, FileSystem::Ntfs]),
            Contents::Windows
        );
        assert_eq!(classify(&[ext("root"), FileSystem::Fat]), Contents::Linux);
        assert_eq!(classify(&[FileSystem::Fat]), Contents::Other);
        assert_eq!(classify(&[]), Contents::Other);
        // The stick itself, whatever else is on it.
        assert_eq!(
            classify(&[ext("SAULE-INSTALL"), FileSystem::Ntfs]),
            Contents::Medium
        );
    }

    /// A disk image in a file: `partitions` as (start in sectors, first
    /// two kilobytes of the file system).
    fn image(name: &str, partitions: &[(u32, [u8; 2048])]) -> File {
        let path =
            std::env::temp_dir().join(format!("saule-install-{name}-{}", std::process::id()));
        let mut file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        file.set_len(64 << 20).unwrap();

        if !partitions.is_empty() {
            let mut sector = [0u8; 512];
            for (i, (start, _)) in partitions.iter().enumerate() {
                let entry = &mut sector[446 + i * 16..462 + i * 16];
                entry[4] = 0x07;
                entry[8..12].copy_from_slice(&start.to_le_bytes());
                entry[12..16].copy_from_slice(&2048u32.to_le_bytes());
            }
            sector[510] = 0x55;
            sector[511] = 0xAA;
            file.write_all(&sector).unwrap();
        }
        for (start, bytes) in partitions {
            file.seek(SeekFrom::Start(u64::from(*start) * 512)).unwrap();
            file.write_all(bytes).unwrap();
        }
        file
    }

    #[test]
    fn an_empty_disk_is_blank() {
        assert_eq!(contents_of(&mut image("blank", &[])), Some(Contents::Blank));
    }

    #[test]
    fn a_disk_with_windows_says_so() {
        let mut disk = image("windows", &[(2048, [0u8; 2048]), (4096, ntfs())]);
        assert_eq!(contents_of(&mut disk), Some(Contents::Windows));
    }

    #[test]
    fn an_installed_disk_and_a_data_disk_are_told_apart() {
        let mut installed = image(
            "installed",
            &[(2048, ext("SAULE-BOOT")), (8192, ext("SAULE"))],
        );
        assert_eq!(contents_of(&mut installed), Some(Contents::System));

        let mut data = image("data", &[(2048, ext("SAULE"))]);
        assert_eq!(contents_of(&mut data), Some(Contents::Data));
    }

    #[test]
    fn partitions_with_nothing_known_in_them_are_something() {
        let mut disk = image("other", &[(2048, [0u8; 2048])]);
        assert_eq!(contents_of(&mut disk), Some(Contents::Other));
    }

    #[test]
    fn a_disk_s_name_is_its_maker_s() {
        assert_eq!(model("ATA     ", "Hitachi HTS54502\n"), "Hitachi HTS54502");
        assert_eq!(model("SanDisk", "Cruzer Blade"), "SanDisk Cruzer Blade");
        assert_eq!(model("QEMU", ""), "QEMU");
        assert_eq!(model("", ""), "");
    }
}

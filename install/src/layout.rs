//! Where things are on a disk Saule Linux boots from.
//!
//! The netbook has a BIOS and no EFI, so the disk has an MBR: the first
//! sector holds the boot code and a table of four partitions.
//!
//! ```text
//!   sector 0          MBR: GRUB's first stage, and the partition table
//!   sectors 1–2047    GRUB itself (`core.img`), in the gap every
//!                     partitioning tool leaves before the first megabyte
//!   partition 1       the system: the kernel, the initramfs it runs from,
//!                     a driver pack, and GRUB's configuration (ext2)
//!   partition 2       what the session keeps: `/data` (ext4, journaled)
//! ```
//!
//! The system has a partition of its own so that it is never mounted while
//! the machine runs: nothing a program does, and no power cut, can leave a
//! machine that does not boot. A USB stick has the first three and no
//! data partition.

/// Bytes in a sector, as an MBR and the BIOS count them.
pub const SECTOR: u64 = 512;

/// Where the first partition starts, in sectors: at the first megabyte.
pub const FIRST_PARTITION: u32 = 2048;

/// The system partition on a hard disk: room for the kernel and initramfs
/// several times over, so that a later, larger system still fits.
pub const SYSTEM_SECTORS: u32 = 512 * 2048;

/// What an ext file system holding the system is called on a hard disk,
pub const SYSTEM_LABEL: &str = "SAULE-BOOT";
/// on the stick it is installed from,
pub const MEDIUM_LABEL: &str = "SAULE-INSTALL";
/// and the one that is `/data` (init looks for it by this name).
pub const DATA_LABEL: &str = "SAULE";

/// A partition: where it starts and how long it is, in sectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partition {
    pub start: u32,
    pub sectors: u32,
}

impl Partition {
    pub fn offset(&self) -> u64 {
        u64::from(self.start) * SECTOR
    }

    pub fn bytes(&self) -> u64 {
        u64::from(self.sectors) * SECTOR
    }
}

/// The partitions of a disk of `sectors` sectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub system: Partition,
    /// None on a stick: it carries the system and keeps nothing.
    pub data: Option<Partition>,
}

/// The smallest disk the system and a little data fit on, in sectors:
/// the system partition and as much again.
pub const SMALLEST_DISK: u64 = FIRST_PARTITION as u64 + 2 * SYSTEM_SECTORS as u64;

impl Layout {
    /// A hard disk: the system, then data to the end. An MBR counts
    /// sectors in 32 bits, so the data stops at 2 TiB on a larger disk.
    pub fn disk(sectors: u64) -> Option<Layout> {
        if sectors < SMALLEST_DISK {
            return None;
        }
        let end = sectors.min(u64::from(u32::MAX)) as u32;
        let data_start = FIRST_PARTITION + SYSTEM_SECTORS;
        Some(Layout {
            system: Partition {
                start: FIRST_PARTITION,
                sectors: SYSTEM_SECTORS,
            },
            data: Some(Partition {
                start: data_start,
                sectors: end - data_start,
            }),
        })
    }

    /// A stick, or its image: one partition from the first megabyte to
    /// the end.
    pub fn stick(sectors: u64) -> Option<Layout> {
        let end = sectors.min(u64::from(u32::MAX)) as u32;
        if end <= FIRST_PARTITION {
            return None;
        }
        Some(Layout {
            system: Partition {
                start: FIRST_PARTITION,
                sectors: end - FIRST_PARTITION,
            },
            data: None,
        })
    }

    /// The partition table, as bytes 440 to 511 of the first sector: a
    /// disk signature, the four entries, and the mark that ends an MBR.
    /// The boot code before it is GRUB's to write (`grub.rs`).
    pub fn table(&self) -> [u8; 72] {
        let mut table = [0u8; 72];
        // A disk signature, for tools that want one.
        table[0..4].copy_from_slice(b"SAUL");

        // The system partition is the active one: some BIOSes of the
        // netbook's years refuse to boot a disk that has none.
        table[6..22].copy_from_slice(&entry(self.system, true));
        if let Some(data) = self.data {
            table[22..38].copy_from_slice(&entry(data, false));
        }

        table[70] = 0x55;
        table[71] = 0xAA;
        table
    }
}

/// Where the table starts in the first sector.
pub const TABLE_AT: usize = 440;

fn entry(partition: Partition, active: bool) -> [u8; 16] {
    // Past what cylinders, heads and sectors can say: every tool then
    // goes by the sector numbers after them.
    const CHS_LIMIT: [u8; 3] = [0xFE, 0xFF, 0xFF];

    let mut entry = [0u8; 16];
    entry[0] = if active { 0x80 } else { 0x00 };
    entry[1..4].copy_from_slice(&CHS_LIMIT);
    entry[4] = 0x83; // Linux
    entry[5..8].copy_from_slice(&CHS_LIMIT);
    entry[8..12].copy_from_slice(&partition.start.to_le_bytes());
    entry[12..16].copy_from_slice(&partition.sectors.to_le_bytes());
    entry
}

/// The partitions an MBR's table lists, in its order; empty entries left
/// out. `sector` is the disk's first.
pub fn partitions_in(sector: &[u8; 512]) -> Vec<(u8, Partition)> {
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return Vec::new();
    }
    (0..4)
        .filter_map(|i| {
            let entry = &sector[446 + i * 16..462 + i * 16];
            let kind = entry[4];
            let start = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
            let sectors = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]);
            (kind != 0 && sectors != 0).then_some((kind, Partition { start, sectors }))
        })
        .collect()
}

/// The device of partition `number` of disk `name`: `sda1`, but
/// `nvme0n1p1`, as the kernel names them.
pub fn partition_name(disk: &str, number: u32) -> String {
    if disk.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{disk}p{number}")
    } else {
        format!("{disk}{number}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The netbook's disk: 488,397,168 sectors, "250 GB".
    const NETBOOK: u64 = 488_397_168;

    #[test]
    fn a_hard_disk_has_the_system_then_the_data_to_its_end() {
        let layout = Layout::disk(NETBOOK).unwrap();
        assert_eq!(layout.system.start, 2048);
        assert_eq!(layout.system.bytes(), 512 << 20);
        let data = layout.data.unwrap();
        assert_eq!(data.start, layout.system.start + layout.system.sectors);
        assert_eq!(u64::from(data.start) + u64::from(data.sectors), NETBOOK);
    }

    #[test]
    fn a_disk_too_small_for_both_has_no_layout() {
        assert!(Layout::disk(SMALLEST_DISK - 1).is_none());
        assert!(Layout::disk(SMALLEST_DISK).is_some());
    }

    #[test]
    fn a_disk_past_two_tebibytes_stops_where_the_table_can_count() {
        let layout = Layout::disk(8 << 32).unwrap();
        let data = layout.data.unwrap();
        assert_eq!(
            u64::from(data.start) + u64::from(data.sectors),
            u64::from(u32::MAX)
        );
    }

    #[test]
    fn a_stick_is_one_partition() {
        let layout = Layout::stick(256 * 2048).unwrap();
        assert_eq!(layout.system.start, 2048);
        assert_eq!(layout.system.sectors, 255 * 2048);
        assert!(layout.data.is_none());
        assert!(Layout::stick(2048).is_none());
    }

    #[test]
    fn the_table_reads_back_as_it_was_written() {
        let layout = Layout::disk(NETBOOK).unwrap();
        let mut sector = [0u8; 512];
        sector[TABLE_AT..].copy_from_slice(&layout.table());

        let found = partitions_in(&sector);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], (0x83, layout.system));
        assert_eq!(found[1], (0x83, layout.data.unwrap()));
        // The system partition is the active one, and only it.
        assert_eq!(sector[446], 0x80);
        assert_eq!(sector[462], 0x00);
    }

    #[test]
    fn a_sector_without_the_mark_has_no_partitions() {
        let mut sector = [0u8; 512];
        sector[446 + 4] = 0x83;
        sector[446 + 12] = 1;
        assert!(partitions_in(&sector).is_empty());
    }

    #[test]
    fn partitions_are_named_as_the_kernel_names_them() {
        assert_eq!(partition_name("sda", 1), "sda1");
        assert_eq!(partition_name("vdb", 2), "vdb2");
        assert_eq!(partition_name("nvme0n1", 1), "nvme0n1p1");
    }
}

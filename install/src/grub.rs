//! Putting GRUB on a disk, and telling it what to boot.
//!
//! GRUB for a BIOS is two pieces, both built on the build machine
//! (`scripts/mkgrub.sh`) and carried on the stick:
//!
//! - `boot.img`, 512 bytes: the code the BIOS loads from the disk's first
//!   sector. All it knows is which sector the second piece starts at.
//! - `core.img`, a few dozen kilobytes: GRUB itself, with the modules that
//!   read an MBR and an ext file system and start Linux. It goes in the
//!   gap between the first sector and the first partition, where it needs
//!   no file system to be found. Its own first sector knows where the rest
//!   of it is.
//!
//! This does what `grub-bios-setup` does for that plain case: write the two
//! where the BIOS and `boot.img` look, and fill in the three numbers that
//! say so. `grub-bios-setup` itself wants a mounted system, a device map
//! and root; this needs a file it can write, so the same code puts GRUB on
//! the netbook's disk and on a stick's image on the build machine.
//!
//! A PC with UEFI and no BIOS ignores all of that and starts
//! `EFI/BOOT/BOOTX64.EFI` from a FAT partition marked as its own. That file
//! is GRUB again, built for UEFI; `mkgrub.sh` makes the whole file system
//! with it inside, and [`write_efi`] copies it into the partition.
//!
//! Either GRUB reads `/grub/grub.cfg` on the first partition of the disk it
//! was started from, whichever disk that is ([`config`]).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

use crate::layout::{FIRST_PARTITION, SECTOR, TABLE_AT};

/// Where in `boot.img` the sector `core.img` starts at is kept (8 bytes),
const BOOT_CORE_SECTOR: usize = 0x5C;
/// and a two-byte jump that skips a check of the drive number the BIOS
/// handed over. `grub-bios-setup` replaces it with two no-ops on anything
/// but a floppy, which turns the check on: some BIOSes pass a wrong number
/// for a hard disk, and the check puts it right.
const BOOT_DRIVE_CHECK: usize = 0x66;
const NOP: u8 = 0x90;

/// The sector `core.img` is written from: the one after the MBR.
const CORE_SECTOR: u64 = 1;

/// At the end of `core.img`'s first sector: where the rest of it is (a
/// sector number, 8 bytes) and how long (in sectors, 2 bytes).
const CORE_REST_SECTOR: usize = 0x1F4;
const CORE_REST_LENGTH: usize = 0x1FC;

/// GRUB's two pieces, as built.
pub struct Grub {
    pub boot: Vec<u8>,
    pub core: Vec<u8>,
}

impl Grub {
    /// Read them from `dir` (`boot.img`, `core.img`).
    pub fn read(dir: &std::path::Path) -> io::Result<Grub> {
        let grub = Grub {
            boot: std::fs::read(dir.join("boot.img"))?,
            core: std::fs::read(dir.join("core.img"))?,
        };
        grub.check()?;
        Ok(grub)
    }

    fn check(&self) -> io::Result<()> {
        if self.boot.len() != SECTOR as usize {
            return Err(io::Error::other("boot.img is not one sector long"));
        }
        if self.core.len() <= SECTOR as usize {
            return Err(io::Error::other("core.img is too short to be GRUB"));
        }
        let room = (u64::from(FIRST_PARTITION) - CORE_SECTOR) * SECTOR;
        if self.core.len() as u64 > room {
            return Err(io::Error::other(
                "core.img does not fit before the first partition",
            ));
        }
        Ok(())
    }

    /// `core.img` in whole sectors, with its first sector saying where
    /// the others are once it is written from [`CORE_SECTOR`].
    fn core_sectors(&self) -> Vec<u8> {
        let sector = SECTOR as usize;
        let mut core = self.core.clone();
        core.resize(core.len().div_ceil(sector) * sector, 0);

        let rest = (core.len() / sector - 1) as u16;
        core[CORE_REST_SECTOR..CORE_REST_SECTOR + 8]
            .copy_from_slice(&(CORE_SECTOR + 1).to_le_bytes());
        core[CORE_REST_LENGTH..CORE_REST_LENGTH + 2].copy_from_slice(&rest.to_le_bytes());
        core
    }

    /// The disk's first sector: `boot.img` pointing at `core.img`, with
    /// the partition `table` (see `Layout::table`) in its place at the end.
    fn first_sector(&self, table: &[u8; 72]) -> [u8; 512] {
        let mut sector = [0u8; 512];
        sector.copy_from_slice(&self.boot);
        sector[BOOT_CORE_SECTOR..BOOT_CORE_SECTOR + 8].copy_from_slice(&CORE_SECTOR.to_le_bytes());
        sector[BOOT_DRIVE_CHECK] = NOP;
        sector[BOOT_DRIVE_CHECK + 1] = NOP;
        sector[TABLE_AT..].copy_from_slice(table);
        sector
    }

    /// Write GRUB and the partition `table` to `disk`: a block device, or
    /// an image file. `core.img` first: a disk written halfway boots what
    /// it booted before.
    pub fn write(&self, disk: &mut File, table: &[u8; 72]) -> io::Result<()> {
        disk.seek(SeekFrom::Start(CORE_SECTOR * SECTOR))?;
        disk.write_all(&self.core_sectors())?;
        disk.seek(SeekFrom::Start(0))?;
        disk.write_all(&self.first_sector(table))?;
        disk.sync_all()
    }

    /// Write GRUB to `disk` and leave the partition table it has.
    pub fn write_keeping_table(&self, disk: &mut File) -> io::Result<()> {
        let mut sector = [0u8; 512];
        disk.seek(SeekFrom::Start(0))?;
        disk.read_exact(&mut sector)?;
        let mut table = [0u8; 72];
        table.copy_from_slice(&sector[TABLE_AT..]);
        self.write(disk, &table)
    }
}

/// Copy the EFI system partition's file system from `source` (the image
/// `mkgrub.sh` made, or the stick's own partition) to `partition` of
/// `disk`. `source` may be the longer; the partition's length is copied.
pub fn write_efi(
    disk: &mut File,
    partition: crate::layout::Partition,
    source: &mut impl Read,
) -> io::Result<()> {
    disk.seek(SeekFrom::Start(partition.offset()))?;
    let copied = io::copy(&mut source.take(partition.bytes()), disk)?;
    if copied < 1 << 20 {
        return Err(io::Error::other("GRUB for UEFI is missing or cut short"));
    }
    disk.sync_all()
}

/// What GRUB is told to boot: the kernel with `options`, and the
/// initramfs followed by `overlays` — more archives the kernel unpacks
/// over it, which is how a machine gets the one graphics driver pack it
/// needs. Paths are on the partition GRUB was started from.
///
/// The system starts by itself after a second in which nothing is shown.
/// Esc pressed (or Shift held) in that second brings up a menu with a
/// second way to start: with the processor drawing everything
/// (`SHINE_GPU=0`), for a graphics chip whose driver misbehaves. And a
/// third: with the desktop drawn by the graphics chip and the programs
/// in it by the processor (`SHINE_APP_GPU=0`), for a chip whose driver
/// runs the one and not the others. Shine finds that out for itself on
/// the old Intel chips, after one program has failed to open; this is
/// for a chip it does not know to doubt.
/// `gfxpayload` has GRUB leave the screen in a graphics mode, which is all
/// a kernel with no driver for the machine's graphics chip has to draw on.
pub fn config(options: &str, overlays: &[String]) -> String {
    let mut initrd = String::from("/initramfs.cpio.gz");
    for overlay in overlays {
        initrd.push(' ');
        initrd.push_str(overlay);
    }
    let options = options.trim();
    let space = if options.is_empty() { "" } else { " " };
    let entry = |title: &str, more: &str| {
        format!(
            "menuentry \"{title}\" {{\n    linux /vmlinuz{space}{options}{more}\n    initrd {initrd}\n}}\n"
        )
    };
    format!(
        "set timeout_style=hidden\nset timeout=1\nset default=0\nset gfxpayload=auto\n{}{}{}",
        entry("Saule Linux", ""),
        entry(SAFE_TITLE, SAFE_OPTIONS),
        entry(TRIAL_TITLE, TRIAL_OPTIONS),
    )
}

/// The menu's second entry, and what it adds to the kernel's options.
const SAFE_TITLE: &str = "Saule Linux, drawn by the processor (safe graphics)";
const SAFE_OPTIONS: &str = " saule.env.SHINE_GPU=0";

/// The third, and what it adds.
const TRIAL_TITLE: &str = "Saule Linux, programs drawn by the processor";
const TRIAL_OPTIONS: &str = " saule.env.SHINE_APP_GPU=0";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Layout;

    fn grub(core_bytes: usize) -> Grub {
        Grub {
            boot: vec![0xEB; 512],
            core: vec![0x11; core_bytes],
        }
    }

    #[test]
    fn the_first_sector_points_at_core_and_keeps_the_table() {
        let layout = Layout::disk(488_397_168).unwrap();
        let sector = grub(30_000).first_sector(&layout.table());

        assert_eq!(&sector[0x5C..0x64], &1u64.to_le_bytes());
        assert_eq!(&sector[0x66..0x68], &[0x90, 0x90]);
        // boot.img's code elsewhere is untouched,
        assert_eq!(sector[0], 0xEB);
        assert_eq!(sector[0x68], 0xEB);
        // and the table is the layout's.
        assert_eq!(crate::layout::partitions_in(&sector).len(), 3);
        assert_eq!(&sector[510..], &[0x55, 0xAA]);
    }

    #[test]
    fn core_says_where_the_rest_of_it_is() {
        // 30,000 bytes: 59 sectors, the last one padded.
        let core = grub(30_000).core_sectors();
        assert_eq!(core.len(), 59 * 512);
        assert_eq!(&core[0x1F4..0x1FC], &2u64.to_le_bytes());
        assert_eq!(&core[0x1FC..0x1FE], &58u16.to_le_bytes());
        assert_eq!(core[30_000], 0);
    }

    #[test]
    fn a_core_too_long_for_the_gap_is_refused() {
        assert!(grub(40_000).check().is_ok());
        assert!(grub(2047 * 512).check().is_ok());
        assert!(grub(2047 * 512 + 1).check().is_err());
        assert!(grub(512).check().is_err());
    }

    #[test]
    fn written_to_a_file_it_is_where_the_bios_looks() {
        let path = std::env::temp_dir().join(format!("saule-install-grub-{}", std::process::id()));
        let mut file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.set_len(4 << 20).unwrap();

        let layout = Layout::stick(80 * 2048).unwrap();
        grub(1000).write(&mut file, &layout.table()).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(bytes[0], 0xEB);
        assert_eq!(bytes[512], 0x11);
        assert_eq!(bytes[512 + 999], 0x11);
        assert_eq!(bytes[512 + 1000], 0);
        assert_eq!(&bytes[510..512], &[0x55, 0xAA]);
    }

    #[test]
    fn keeping_the_table_keeps_it() {
        let path = std::env::temp_dir().join(format!("saule-install-keep-{}", std::process::id()));
        let mut file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.set_len(4 << 20).unwrap();
        let layout = Layout::disk(488_397_168).unwrap();
        let mut first = [0u8; 512];
        first[TABLE_AT..].copy_from_slice(&layout.table());
        file.write_all(&first).unwrap();

        grub(1000).write_keeping_table(&mut file).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut sector = [0u8; 512];
        sector.copy_from_slice(&bytes[..512]);
        assert_eq!(crate::layout::partitions_in(&sector).len(), 3);
        assert_eq!(sector[0], 0xEB);
    }

    #[test]
    fn grub_for_uefi_is_copied_into_its_partition() {
        let path = std::env::temp_dir().join(format!("saule-install-efi-{}", std::process::id()));
        let mut file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.set_len(80 << 20).unwrap();
        let layout = Layout::stick(80 * 2048).unwrap();

        // Longer than the partition: only the partition's length is taken.
        let source = vec![0xEFu8; 35 << 20];
        write_efi(&mut file, layout.efi, &mut source.as_slice()).unwrap();
        // Too short to be GRUB: refused.
        assert!(write_efi(&mut file, layout.efi, &mut &source[..1000]).is_err());

        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let start = layout.efi.offset() as usize;
        assert_eq!(bytes[start - 1], 0);
        assert_eq!(bytes[start + 2000], 0xEF);
        assert_eq!(bytes.len(), 80 << 20);
        assert_eq!(bytes[bytes.len() - 1], 0xEF);
    }

    #[test]
    fn the_configuration_starts_the_system_and_offers_safe_graphics() {
        assert_eq!(
            config("quiet", &[]),
            "set timeout_style=hidden\nset timeout=1\nset default=0\nset gfxpayload=auto\n\
             menuentry \"Saule Linux\" {\n    linux /vmlinuz quiet\n    initrd /initramfs.cpio.gz\n}\n\
             menuentry \"Saule Linux, drawn by the processor (safe graphics)\" {\n    \
             linux /vmlinuz quiet saule.env.SHINE_GPU=0\n    initrd /initramfs.cpio.gz\n}\n\
             menuentry \"Saule Linux, programs drawn by the processor\" {\n    \
             linux /vmlinuz quiet saule.env.SHINE_APP_GPU=0\n    initrd /initramfs.cpio.gz\n}\n"
        );
        let with_pack = config("", &["/drivers/gen3.cpio.gz".into()]);
        assert!(with_pack
            .contains("    linux /vmlinuz\n    initrd /initramfs.cpio.gz /drivers/gen3.cpio.gz\n"));
        assert!(with_pack.contains("    linux /vmlinuz saule.env.SHINE_GPU=0\n"));
    }
}

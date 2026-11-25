//! A USB stick's image, made on the build machine.
//!
//! `saule-install image OUT ROOT` turns a folder — the kernel, the
//! initramfs, the driver packs, GRUB and its configuration, laid out as
//! they are on the stick — into a file that is the stick, byte for byte:
//! written to one with `dd` (or Rufus, in its DD mode), the netbook's BIOS
//! boots it. Nothing here needs root: the file system is made already
//! filled, straight into its place in the file.

use std::fs::{self, File};
use std::io;
use std::path::Path;

use crate::grub::Grub;
use crate::layout::{Layout, MEDIUM_LABEL, SECTOR};
use crate::sys::{self, Format};

/// Make `out` from the folder `root`. `megabytes` is the image's size;
/// without it, what the files need and a little room.
pub fn make(out: &Path, root: &Path, megabytes: Option<u64>) -> io::Result<u64> {
    let grub = Grub::read(&root.join("grub"))?;
    for needed in ["vmlinuz", "initramfs.cpio.gz", "grub/grub.cfg"] {
        if !root.join(needed).is_file() {
            return Err(io::Error::other(format!(
                "{} has no {needed}",
                root.display()
            )));
        }
    }

    let megabytes = megabytes.unwrap_or_else(|| megabytes_for(size_of(root)));
    let bytes = megabytes << 20;
    let layout = Layout::stick(bytes / SECTOR)
        .ok_or_else(|| io::Error::other("an image needs more than a megabyte"))?;

    let mut file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(out)?;
    file.set_len(bytes)?;

    sys::format(
        out,
        Some((layout.system.offset(), layout.system.bytes())),
        MEDIUM_LABEL,
        Format::System { from: Some(root) },
    )?;
    grub.write(&mut file, &layout.table())?;
    Ok(bytes)
}

/// The megabytes an image needs for files of `bytes` in all: the first
/// megabyte before the partition, the file system's own tables (a tenth
/// more and four megabytes covers them), rounded up to eight.
fn megabytes_for(bytes: u64) -> u64 {
    let megabytes = bytes.div_ceil(1 << 20);
    let needed = 1 + megabytes + megabytes / 10 + 4;
    needed.div_ceil(8) * 8
}

/// The size of every file under `path`.
fn size_of(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => size_of(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_has_room_for_its_files_and_the_tables() {
        assert_eq!(megabytes_for(0), 8);
        assert_eq!(megabytes_for(40 << 20), 56);
        assert_eq!(megabytes_for((100 << 20) + 1), 120);
    }
}

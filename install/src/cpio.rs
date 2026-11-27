//! A cpio archive ("newc", the kind an initramfs is), written and read.
//!
//! An update's initramfs begins with one, not compressed: the part of the
//! system that an installed machine keeps on its disk and not in memory
//! (the apps; see `update.rs`). The kernel unpacks an initramfs made of
//! several archives one after another, so a machine that starts the file
//! whole has the apps in memory, as systems had before; the updater here
//! reads the archive off the front, puts its files on the disk, and
//! keeps the rest as the initramfs.
//!
//! ```text
//!   per entry   "070701", thirteen numbers of eight hexadecimal digits
//!               (the file's mode is the second, its length the seventh,
//!               its name's length with the NUL after it the twelfth),
//!               the name and a NUL, padded to four bytes; the file's
//!               bytes, padded to four
//!   at the end  an entry named TRAILER!!!, and NULs to a whole 512
//! ```

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

const MAGIC: &[u8; 6] = b"070701";
const HEADER: usize = 110;
const TRAILER: &str = "TRAILER!!!";
const DIRECTORY: u32 = 0o040000;
const REGULAR: u32 = 0o100000;
const KIND: u32 = 0o170000;

/// The longest name read, and the most entries: far more than the apps
/// are, and little enough that a damaged archive is not followed far.
const LONGEST_NAME: usize = 1024;
const MOST_ENTRIES: usize = 4096;

fn damaged<T>() -> io::Result<T> {
    Err(io::Error::other("The update file is damaged."))
}

/// Whether `start`, the first bytes of a file, begin such an archive.
pub fn begins(start: &[u8]) -> bool {
    start.starts_with(MAGIC)
}

/// The archive of everything under `folder`: its folders, then their
/// files, named from `folder` down, all root's. Programs keep that they
/// are programs.
pub fn pack(folder: &Path) -> io::Result<Vec<u8>> {
    let mut paths = Vec::new();
    collect(folder, Path::new(""), &mut paths)?;
    paths.sort();

    let mut out = Vec::new();
    let mut number = 1u32;
    for relative in &paths {
        let path = folder.join(relative);
        let name = relative.to_string_lossy();
        if path.is_dir() {
            entry(&mut out, number, &name, DIRECTORY | 0o755, 2, &[]);
        } else {
            let runs = fs::metadata(&path)?.permissions().mode() & 0o111 != 0;
            let mode = REGULAR | if runs { 0o755 } else { 0o644 };
            entry(&mut out, number, &name, mode, 1, &fs::read(&path)?);
        }
        number += 1;
    }
    entry(&mut out, 0, TRAILER, 0, 1, &[]);
    out.resize(out.len().next_multiple_of(512), 0);
    Ok(out)
}

/// Every folder and file under `folder`, as paths from it. A folder
/// sorts before what is in it, so it is made before they are.
fn collect(folder: &Path, under: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for found in fs::read_dir(folder.join(under))? {
        let found = found?;
        let relative = under.join(found.file_name());
        let kind = found.file_type()?;
        if kind.is_dir() {
            out.push(relative.clone());
            collect(folder, &relative, out)?;
        } else if kind.is_file() {
            out.push(relative);
        }
    }
    Ok(())
}

fn entry(out: &mut Vec<u8>, number: u32, name: &str, mode: u32, links: u32, data: &[u8]) {
    let fields = [
        number,
        mode,
        0,
        0,
        links,
        0,
        data.len() as u32,
        0,
        0,
        0,
        0,
        name.len() as u32 + 1,
        0,
    ];
    out.extend_from_slice(MAGIC);
    for field in fields {
        out.extend_from_slice(format!("{field:08X}").as_bytes());
    }
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.resize(out.len().next_multiple_of(4), 0);
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

/// Read the archive `from` begins with into `folder`, and return how many
/// bytes of `from` it was, the NULs after it included: what follows is
/// the next archive. No more than `limit` bytes are read.
///
/// Only folders and files are taken, under names that stay inside
/// `folder`, and each file is on the disk when this returns.
pub fn unpack(from: &mut impl Read, limit: u64, folder: &Path) -> io::Result<u64> {
    let mut from = Counted {
        inner: from,
        read: 0,
        limit,
    };
    fs::create_dir_all(folder)?;

    for _ in 0..MOST_ENTRIES {
        let mut header = [0u8; HEADER];
        from.read_exact(&mut header)?;
        if !begins(&header) {
            return damaged();
        }
        let field = |at: usize| -> io::Result<u32> {
            let digits = &header[6 + at * 8..14 + at * 8];
            std::str::from_utf8(digits)
                .ok()
                .and_then(|digits| u32::from_str_radix(digits, 16).ok())
                .map_or_else(damaged, Ok)
        };
        let (mode, length, name_length) = (field(1)?, field(6)? as u64, field(11)? as usize);
        if name_length == 0 || name_length > LONGEST_NAME {
            return damaged();
        }
        let mut name = vec![0u8; name_length];
        from.read_exact(&mut name)?;
        from.skip_to_four()?;
        name.pop();
        let Ok(name) = String::from_utf8(name) else {
            return damaged();
        };

        if name == TRAILER {
            // The NULs that fill the archive out, up to whatever is next.
            return from.skip_nuls();
        }
        let place = folder.join(inside(&name)?);
        match mode & KIND {
            DIRECTORY => {
                fs::create_dir_all(&place)?;
                fs::set_permissions(&place, fs::Permissions::from_mode(0o755))?;
            }
            REGULAR => {
                let mut out = File::create(&place)?;
                let copied = io::copy(&mut (&mut from).take(length), &mut out)?;
                if copied != length {
                    return damaged();
                }
                let runs = mode & 0o111 != 0;
                out.set_permissions(fs::Permissions::from_mode(if runs { 0o755 } else { 0o644 }))?;
                out.flush()?;
                out.sync_all()?;
                from.skip_to_four()?;
            }
            _ => return damaged(),
        }
    }
    damaged()
}

/// `name` as a path that stays inside the folder it is unpacked into.
fn inside(name: &str) -> io::Result<PathBuf> {
    let path = Path::new(name);
    let plain = path
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    if name.is_empty() || !plain {
        return damaged();
    }
    Ok(path.to_path_buf())
}

/// A reader that knows how far it has read, and reads no further than it
/// may.
struct Counted<'a, R: Read> {
    inner: &'a mut R,
    read: u64,
    limit: u64,
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let left = self.limit - self.read;
        let want = (buffer.len() as u64).min(left) as usize;
        let count = self.inner.read(&mut buffer[..want])?;
        self.read += count as u64;
        Ok(count)
    }
}

impl<R: Read> Counted<'_, R> {
    fn skip_to_four(&mut self) -> io::Result<()> {
        let pad = (4 - self.read % 4) % 4;
        let mut bytes = [0u8; 3];
        self.read_exact(&mut bytes[..pad as usize])
    }

    /// Read on over NULs; how many bytes were read before the first that
    /// isn't one, or the limit.
    fn skip_nuls(&mut self) -> io::Result<u64> {
        let mut byte = [0u8; 1];
        loop {
            if self.read(&mut byte)? == 0 {
                return Ok(self.read);
            }
            if byte[0] != 0 {
                return Ok(self.read - 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("saule-cpio-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn tree(root: &Path) {
        fs::create_dir_all(root.join("bin/apps")).unwrap();
        fs::write(root.join("bin/apps/editor"), b"#!/bin/saule run\nbytecode").unwrap();
        fs::write(root.join("bin/apps/files"), vec![9u8; 70_001]).unwrap();
        fs::write(root.join("bin/saule-pdf"), b"a program").unwrap();
        fs::write(root.join("bin/notes"), b"not a program").unwrap();
        for program in ["bin/apps/editor", "bin/apps/files", "bin/saule-pdf"] {
            fs::set_permissions(root.join(program), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::set_permissions(root.join("bin/notes"), fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn what_is_packed_is_unpacked_the_same_and_the_rest_is_found() {
        let work = folder("round");
        tree(&work.join("from"));
        let archive = pack(&work.join("from")).unwrap();
        // As the kernel wants the next archive: on a whole four bytes.
        assert!(begins(&archive));
        assert_eq!(archive.len() % 512, 0);

        // With an initramfs after it, as in an update.
        let mut whole = archive.clone();
        whole.extend_from_slice(b"\x1f\x8b the rest");
        let into = work.join("into");
        let read = unpack(&mut whole.as_slice(), whole.len() as u64, &into).unwrap();
        assert_eq!(read, archive.len() as u64);
        assert_eq!(&whole[read as usize..], b"\x1f\x8b the rest");

        assert_eq!(
            fs::read(into.join("bin/apps/editor")).unwrap(),
            b"#!/bin/saule run\nbytecode"
        );
        assert_eq!(fs::read(into.join("bin/apps/files")).unwrap().len(), 70_001);
        let mode = |file: &str| fs::metadata(into.join(file)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("bin/apps/editor"), 0o755);
        assert_eq!(mode("bin/saule-pdf"), 0o755);
        assert_eq!(mode("bin/notes"), 0o644);
        assert_eq!(mode("bin/apps"), 0o755);

        // Alone, it is read to its end.
        let again = work.join("again");
        assert_eq!(
            unpack(&mut archive.as_slice(), archive.len() as u64, &again).unwrap(),
            archive.len() as u64
        );
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    fn the_kernel_s_own_tool_reads_what_is_packed() {
        // `cpio` is on every build machine; without it there is nothing
        // to ask.
        let work = folder("kernel");
        tree(&work.join("from"));
        let archive = work.join("apps.cpio");
        fs::write(&archive, pack(&work.join("from")).unwrap()).unwrap();
        let listed = std::process::Command::new("cpio")
            .arg("-t")
            .stdin(File::open(&archive).unwrap())
            .stderr(std::process::Stdio::null())
            .output();
        if let Ok(listed) = listed {
            assert!(listed.status.success());
            assert_eq!(
                String::from_utf8_lossy(&listed.stdout),
                "bin\nbin/apps\nbin/apps/editor\nbin/apps/files\nbin/notes\nbin/saule-pdf\n"
            );
        }
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    fn an_archive_that_would_write_elsewhere_or_is_cut_short_is_refused() {
        let work = folder("refuse");
        for name in ["../out", "/etc/passwd", "bin/../../out", ""] {
            let mut archive = Vec::new();
            entry(&mut archive, 1, name, REGULAR | 0o644, 1, b"x");
            entry(&mut archive, 0, TRAILER, 0, 1, &[]);
            let result = unpack(
                &mut archive.as_slice(),
                archive.len() as u64,
                &work.join("a"),
            );
            assert!(result.is_err(), "{name:?} was taken");
        }
        assert!(!work.join("out").exists());

        // A link, a device: nothing but folders and files.
        let mut archive = Vec::new();
        entry(&mut archive, 1, "bin/sh", 0o120777, 1, b"/bin/busybox");
        entry(&mut archive, 0, TRAILER, 0, 1, &[]);
        assert!(unpack(
            &mut archive.as_slice(),
            archive.len() as u64,
            &work.join("b")
        )
        .is_err());

        // Cut short, and read no further than allowed.
        tree(&work.join("from"));
        let archive = pack(&work.join("from")).unwrap();
        assert!(unpack(&mut &archive[..300], 300, &work.join("c")).is_err());
        assert!(unpack(&mut archive.as_slice(), 300, &work.join("d")).is_err());
        assert!(unpack(&mut &b"not an archive at all, but long enough to hold one header of a hundred and ten bytes, and then some more"[..], 200, &work.join("e")).is_err());
        fs::remove_dir_all(work).unwrap();
    }
}

//! What is asked of the kernel and of `mke2fs`.

use std::ffi::CString;
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The `ioctl` that has the kernel read a disk's partition table again
/// (`BLKRRPART` in `<linux/fs.h>`, which the libc crate doesn't name).
const BLKRRPART: libc::Ioctl = 0x125F;

/// Have the kernel read the partition table of the disk `file` is open on.
pub fn reread_partitions(file: &File) -> io::Result<()> {
    // SAFETY: an ioctl with no argument, on a file opened by the caller.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), BLKRRPART) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Wait for `path` to exist: the kernel makes a partition's device a
/// moment after it has read the table.
pub fn wait_for(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "The disk's new partition {} did not appear.",
                path.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// A file system mounted for as long as this lives.
pub struct Mounted {
    at: PathBuf,
}

impl Mounted {
    /// Mount the ext file system on `device` at `at`, made if it isn't
    /// there.
    pub fn new(device: &Path, at: &Path, read_only: bool) -> io::Result<Mounted> {
        fs::create_dir_all(at)?;
        let source = CString::new(device.as_os_str().as_encoded_bytes())?;
        let target = CString::new(at.as_os_str().as_encoded_bytes())?;
        // The kernel's ext4 driver reads ext2 as well.
        let fstype = CString::new("ext4")?;
        let mut flags = libc::MS_NOATIME | libc::MS_NOSUID | libc::MS_NODEV;
        if read_only {
            flags |= libc::MS_RDONLY;
        }

        // SAFETY: NUL-terminated strings that outlive the call.
        let result = unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                fstype.as_ptr(),
                flags,
                std::ptr::null(),
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Mounted {
            at: at.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.at
    }

    /// Write everything out and let go of it, saying so if that fails:
    /// what was copied isn't on the disk until this returns.
    pub fn unmount(self) -> io::Result<()> {
        let result = self.release();
        std::mem::forget(self);
        result
    }

    fn release(&self) -> io::Result<()> {
        let target = CString::new(self.at.as_os_str().as_encoded_bytes())?;
        // SAFETY: plain system calls with a NUL-terminated path.
        let result = unsafe {
            libc::sync();
            libc::umount2(target.as_ptr(), 0)
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Whether anything on the disk called `name` is mounted: the disk itself
/// or one of its partitions.
pub fn in_use(name: &str) -> bool {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    mounts_use(&mounts, name)
}

fn mounts_use(mounts: &str, name: &str) -> bool {
    let whole = format!("/dev/{name}");
    mounts
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .any(|device| match device.strip_prefix(whole.as_str()) {
            Some(rest) => {
                let rest = rest.strip_prefix('p').unwrap_or(rest);
                rest.bytes().all(|b| b.is_ascii_digit())
            }
            None => false,
        })
}

/// `mke2fs`: where the image keeps it, or the build machine's.
fn mke2fs() -> Command {
    for path in ["/sbin/mke2fs", "/usr/sbin/mke2fs"] {
        if Path::new(path).exists() {
            return Command::new(path);
        }
    }
    Command::new("mke2fs")
}

/// What a file system is made for.
pub enum Format<'a> {
    /// The system: ext2, which GRUB reads whatever the tools' version, and
    /// which is written once and only read after. When `from` is given
    /// the file system is made already holding that folder's files, which
    /// needs no mounting, and so no root: how the build machine fills a
    /// stick's image.
    System { from: Option<&'a Path> },
    /// `/data`: ext4 with a journal, as init makes it.
    Data,
}

/// Make a file system labelled `label` on `target`: a partition's device,
/// or with `within` a disk or image and the place in it (byte offset and
/// length).
pub fn format(
    target: &Path,
    within: Option<(u64, u64)>,
    label: &str,
    format: Format,
) -> io::Result<()> {
    let mut command = mke2fs();
    command.args(["-q", "-F", "-L", label]);

    let mut extended = Vec::new();
    match format {
        Format::System { from } => {
            command.args(["-t", "ext2", "-m", "0"]);
            if let Some(from) = from {
                command.arg("-d").arg(from);
            }
        }
        Format::Data => {
            // One inode per megabyte: a home of documents and settings
            // needs far fewer than the default, and their tables cost a
            // 250 GB disk gigabytes. The tables and the journal are
            // zeroed by the kernel once the file system is in use, not
            // now: the disk is ready at once.
            command.args(["-t", "ext4", "-m", "1", "-i", "1048576"]);
            extended.push("lazy_itable_init=1,lazy_journal_init=1".to_string());
        }
    }
    if let Some((offset, _)) = within {
        extended.push(format!("offset={offset}"));
    }
    if !extended.is_empty() {
        command.arg("-E").arg(extended.join(","));
    }

    command.arg(target);
    if let Some((_, bytes)) = within {
        command.arg(format!("{}k", bytes / 1024));
    }

    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.lines().last().unwrap_or("").trim();
        return Err(io::Error::other(format!(
            "The disk could not be formatted ({said})."
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTS: &str = "\
proc /proc proc rw 0 0
/dev/sda2 /data ext4 rw 0 0
/dev/nvme0n1p1 /mnt ext4 rw 0 0
";

    #[test]
    fn a_disk_is_in_use_when_any_of_it_is_mounted() {
        assert!(mounts_use(MOUNTS, "sda"));
        assert!(mounts_use(MOUNTS, "nvme0n1"));
        assert!(!mounts_use(MOUNTS, "sdb"));
        // `sd` is not `sda`, and `nvme0n` not `nvme0n1`.
        assert!(!mounts_use(MOUNTS, "sd"));
        assert!(!mounts_use("/dev/sdab1 /x ext4 rw 0 0\n", "sda"));
    }
}

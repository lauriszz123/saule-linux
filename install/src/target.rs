//! Installing onto this machine's disk.
//!
//! `saule-install install DISK` erases the disk and makes it boot Saule
//! Linux, from what is on the stick the machine was started from. In
//! order, so that a disk is only bootable once all of it is there:
//!
//! 1. a new partition table, and nothing else in the first megabyte;
//! 2. the system's file system, and into it the kernel, the initramfs,
//!    the one graphics driver pack this machine's chip needs, and GRUB's
//!    configuration;
//! 3. the data's file system, and in it what the person told the installer
//!    (`answers.rs`: who the machine is for, the password, the keyboard's
//!    layout), and a note when the disk had Windows on it: its clock keeps
//!    local time;
//! 4. GRUB, which is what makes the machine start any of it: for UEFI in
//!    the partition at the end of the disk, copied from the stick's own,
//!    and for a BIOS in the disk's first sectors, last of all.
//!
//! With `--keep-data`, on a disk that has Saule Linux already: the same
//! without 1, and with no new file system in 3. The system is written
//! again and what was kept stays, but for the answers given this time.
//!
//! What it is doing goes to standard output a line at a time, for the
//! installer's window to show:
//!
//! ```text
//!   step prepare | system | data | boot
//!   copied 42            how much of the system is on the disk, in percent
//!   driver gen3          the pack chosen (or `driver none`)
//!   done
//! ```
//!
//! An error is one line on standard error and exit code 1. The window shows
//! the line as it is, so each is a sentence said to a person.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::answers::Answers;
use crate::disks::{self, Contents, Disk};
use crate::drivers;
use crate::grub::{self, Grub};
use crate::layout::{self, Layout, DATA_LABEL, MEDIUM_LABEL, SYSTEM_LABEL, TABLE_AT};
use crate::sys::{self, Format, Mounted};

/// Where the stick and the new system are mounted meanwhile.
const MEDIUM_AT: &str = "/run/saule/medium";
const TARGET_AT: &str = "/run/saule/target";

/// The kernel options an installed system starts with when the stick
/// doesn't say (its `cmdline` file).
const DEFAULT_OPTIONS: &str = "quiet";

pub struct Request<'a> {
    pub disk: &'a str,
    pub keep_data: bool,
    /// A folder laid out as the stick is, instead of the stick itself.
    pub from: Option<&'a Path>,
    /// What the person told the installer.
    pub answers: Answers,
}

fn say(line: &str) {
    println!("{line}");
    let _ = io::stdout().flush();
}

fn refuse<T>(why: impl Into<String>) -> io::Result<T> {
    Err(io::Error::other(why.into()))
}

pub fn install(request: &Request) -> io::Result<()> {
    say("step prepare");

    let disk = match disks::find(request.disk) {
        Some(disk) => disk,
        None => return refuse("That disk is no longer there."),
    };
    if disk.contents == Contents::Medium {
        return refuse("That is the USB stick this computer was started from.");
    }
    if sys::in_use(&disk.name) {
        return refuse("The disk is in use.");
    }
    let Some(layout) = Layout::disk(disk.sectors) else {
        return refuse(format!(
            "The disk is too small: Saule Linux needs {} MB.",
            layout::SMALLEST_DISK / 2048
        ));
    };

    // The stick, or the folder standing in for it.
    let mounted;
    let medium: &Path = match request.from {
        Some(folder) => folder,
        None => {
            mounted = mount_medium()?;
            mounted.path()
        }
    };
    let grub = Grub::read(&medium.join("grub"))?;
    let system = System::on(medium)?;
    say(&format!(
        "driver {}",
        system.pack.as_deref().unwrap_or("none")
    ));

    let mut whole = File::options().read(true).write(true).open(&disk.device)?;

    let (system_device, data_device) = if request.keep_data {
        (kept_system(&disk)?, None)
    } else {
        write_table(&mut whole, &layout)?;
        let system_device = partition_device(&disk, 1);
        let data_device = partition_device(&disk, 2);
        sys::wait_for(&system_device)?;
        sys::wait_for(&data_device)?;
        (system_device, Some(data_device))
    };

    say("step system");
    sys::format(
        &system_device,
        None,
        SYSTEM_LABEL,
        Format::System { from: None },
    )?;
    let target = Mounted::new(&system_device, Path::new(TARGET_AT), false)?;
    system.copy_to(target.path())?;
    target.unmount()?;

    say("step data");
    match data_device {
        Some(data_device) => {
            sys::format(&data_device, None, DATA_LABEL, Format::Data)?;
            let local_clock =
                matches!(disk.contents, Contents::Windows | Contents::WindowsAndLinux);
            settle(&data_device, local_clock, &request.answers)?;
        }
        // The data kept: only what was answered this time is written.
        None if !request.answers.is_empty() => {
            let Some((number, _)) = disks::labelled_partition(&disk, DATA_LABEL) else {
                return refuse("The disk has no data partition to keep the answers on.");
            };
            settle(&partition_device(&disk, number), false, &request.answers)?;
        }
        None => {}
    }

    say("step boot");
    copy_efi(&mut whole, &disk, &layout, request)?;
    if request.keep_data {
        grub.write_keeping_table(&mut whole)?;
    } else {
        grub.write(&mut whole, &layout.table())?;
    }
    // SAFETY: a plain system call.
    unsafe { libc::sync() };

    say("done");
    Ok(())
}

/// Mount the stick's file system: the partition labelled as one.
fn mount_medium() -> io::Result<Mounted> {
    for disk in disks::all() {
        if disk.contents != Contents::Medium {
            continue;
        }
        if let Some((number, _)) = disks::labelled_partition(&disk, MEDIUM_LABEL) {
            return Mounted::new(&partition_device(&disk, number), Path::new(MEDIUM_AT), true);
        }
    }
    refuse("The USB stick Saule Linux is installed from was not found. Is it still plugged in?")
}

/// How long the stick may take to show up after the kernel has started:
/// the BIOS read the system off it, but Linux finds USB drives seconds
/// into its own start.
const STICK_APPEARS: Duration = Duration::from_secs(8);

/// Unpack the graphics driver pack this machine's chip needs over the
/// running system, from the stick it started from: what GRUB does for an
/// installed system, done late for the stick, which can't know the chip
/// when it is made. Init runs this before it starts the installer, so the
/// installer and the system tried from the stick draw on the GPU too.
///
/// Prints the pack's name, or `none`. Needs `zcat` and `cpio` (the
/// image's BusyBox tools); without them, or without a pack for the chip,
/// the screen is drawn on the CPU, as it always can be.
pub fn live_drivers() -> io::Result<()> {
    let deadline = Instant::now() + STICK_APPEARS;
    let medium = loop {
        match mount_medium() {
            Ok(medium) => break medium,
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    };

    let list = fs::read_to_string(medium.path().join("drivers.list")).unwrap_or_default();
    let pack = drivers::pack_for(&list, &drivers::chips())
        .filter(|pack| medium.path().join(overlay(pack)).is_file());
    let Some(pack) = pack else {
        say("none");
        return Ok(());
    };

    let archive = medium.path().join(overlay(&pack));
    let unpack = format!(
        "{BUSYBOX} zcat '{}' | {BUSYBOX} cpio -idmu",
        archive.display()
    );
    let status = Command::new(BUSYBOX)
        .args(["sh", "-c", &unpack])
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return refuse(format!("The {pack} driver pack could not be unpacked."));
    }

    say(&pack);
    Ok(())
}

/// The image's command-line tools.
const BUSYBOX: &str = "/usr/bin/busybox";

/// GRUB for UEFI, from the stick's partition for it into the disk's.
///
/// Keeping the data keeps the disk's partitions, so a disk installed
/// before there was such a partition has none to fill, and goes on
/// starting from a BIOS only. Installing from a folder rather than a stick
/// takes `efi.img` beside the folder, when there is one.
fn copy_efi(whole: &mut File, disk: &Disk, layout: &Layout, request: &Request) -> io::Result<()> {
    let into = if request.keep_data {
        match efi_partition(&disk.device) {
            Some(partition) => partition,
            None => return Ok(()),
        }
    } else {
        layout.efi
    };

    if let Some(folder) = request.from {
        let image = folder.with_file_name("efi.img");
        if !image.is_file() {
            return Ok(());
        }
        return grub::write_efi(whole, into, &mut File::open(image)?);
    }

    for stick in disks::all() {
        if stick.contents != Contents::Medium {
            continue;
        }
        if let Some(from) = efi_partition(&stick.device) {
            let mut source = File::open(&stick.device)?;
            source.seek(SeekFrom::Start(from.offset()))?;
            return grub::write_efi(whole, into, &mut source.take(from.bytes()));
        }
    }
    refuse("The USB stick has no GRUB for UEFI on it.")
}

/// The EFI system partition of the disk at `device`, as its MBR lists it.
fn efi_partition(device: &Path) -> Option<layout::Partition> {
    let mut sector = [0u8; 512];
    File::open(device).ok()?.read_exact(&mut sector).ok()?;
    layout::partitions_in(&sector)
        .into_iter()
        .find(|(kind, _)| *kind == layout::EFI_SYSTEM)
        .map(|(_, partition)| partition)
}

fn partition_device(disk: &Disk, number: u32) -> PathBuf {
    Path::new("/dev").join(layout::partition_name(&disk.name, number))
}

/// The system partition of a disk that has Saule Linux on it, to be
/// written again in place.
fn kept_system(disk: &Disk) -> io::Result<PathBuf> {
    if disk.contents != Contents::System {
        return refuse("The disk has no Saule Linux on it to keep the files of.");
    }
    match disks::labelled_partition(disk, SYSTEM_LABEL) {
        Some((number, _)) => Ok(partition_device(disk, number)),
        None => refuse("The disk has no system partition to install into."),
    }
}

/// What a new system finds on its data disk before anything has run:
/// the answers given to the installer, and whether the PC's clock keeps
/// local time.
///
/// Windows sets a PC's own clock to local time, where Linux expects UTC.
/// A machine that had Windows on it is taken to have its clock that way
/// still: said in a file on the new data disk, which init reads once the
/// person has chosen a time zone (`clock` in ../init/src/system.rs), so
/// the time comes out right without anyone having to set it.
fn settle(data_device: &Path, local_clock: bool, answers: &Answers) -> io::Result<()> {
    let data = Mounted::new(data_device, Path::new(TARGET_AT), false)?;
    if local_clock {
        let folder = data.path().join("system");
        fs::create_dir_all(&folder)?;
        fs::write(folder.join("clock"), "local\n")?;
    }
    answers.keep(data.path())?;
    data.unmount()
}

/// A new partition table on `whole`, and zeros over the rest of the first
/// megabyte: the boot code of whatever was there, and the start of a GPT
/// if it had one, which the kernel would otherwise still find. Then the
/// kernel reads the table.
fn write_table(whole: &mut File, layout: &Layout) -> io::Result<()> {
    let mut start = vec![0u8; 1 << 20];
    start[TABLE_AT..512].copy_from_slice(&layout.table());
    whole.seek(SeekFrom::Start(0))?;
    whole.write_all(&start)?;
    whole.sync_all()?;
    sys::reread_partitions(whole)
}

/// What the stick has for the system partition, and what this machine
/// takes of it.
struct System {
    medium: PathBuf,
    /// Files copied as they are, by their path on both.
    files: Vec<String>,
    /// The graphics driver pack for this machine's chip, if the stick has
    /// it.
    pack: Option<String>,
    /// The kernel's options.
    options: String,
}

impl System {
    fn on(medium: &Path) -> io::Result<System> {
        let mut files = Vec::new();
        for needed in [
            "vmlinuz",
            "initramfs.cpio.gz",
            "grub/boot.img",
            "grub/core.img",
        ] {
            if !medium.join(needed).is_file() {
                return refuse(format!("The USB stick is missing {needed}."));
            }
            files.push(needed.to_string());
        }
        // Kept beside the system: what a later repair would go by.
        for optional in ["cmdline", "drivers.list"] {
            if medium.join(optional).is_file() {
                files.push(optional.to_string());
            }
        }

        let list = fs::read_to_string(medium.join("drivers.list")).unwrap_or_default();
        let pack = drivers::pack_for(&list, &drivers::chips())
            .filter(|pack| medium.join(overlay(pack)).is_file());
        if let Some(pack) = &pack {
            files.push(overlay(pack));
        }

        let options = fs::read_to_string(medium.join("cmdline"))
            .map(|text| text.trim().to_string())
            .unwrap_or_else(|_| DEFAULT_OPTIONS.to_string());

        Ok(System {
            medium: medium.to_path_buf(),
            files,
            pack,
            options,
        })
    }

    /// Copy it all into the file system mounted at `target`, saying how
    /// far along it is, and write GRUB's configuration for it.
    fn copy_to(&self, target: &Path) -> io::Result<()> {
        let total: u64 = self
            .files
            .iter()
            .map(|file| fs::metadata(self.medium.join(file)).map_or(0, |meta| meta.len()))
            .sum();
        let mut copied = 0u64;
        let mut last_said = 0u64;
        let mut buffer = vec![0u8; 1 << 20];

        for file in &self.files {
            let to = target.join(file);
            if let Some(folder) = to.parent() {
                fs::create_dir_all(folder)?;
            }
            let mut source = File::open(self.medium.join(file))?;
            let mut out = File::create(&to)?;
            loop {
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                out.write_all(&buffer[..count])?;
                copied += count as u64;
                // Every four megabytes: often enough for a bar to move.
                if copied - last_said >= 4 << 20 {
                    last_said = copied;
                    say(&format!("copied {}", copied * 100 / total.max(1)));
                }
            }
            out.sync_all()?;
        }

        let overlays: Vec<String> = self
            .pack
            .iter()
            .map(|pack| format!("/{}", overlay(pack)))
            .collect();
        fs::write(
            target.join("grub/grub.cfg"),
            grub::config(&self.options, &overlays),
        )?;
        say("copied 100");
        Ok(())
    }
}

/// Where a driver pack is, on the stick and on the disk.
fn overlay(pack: &str) -> String {
    format!("drivers/{pack}.cpio.gz")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("saule-install-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("grub")).unwrap();
        fs::create_dir_all(path.join("drivers")).unwrap();
        path
    }

    fn stick(name: &str) -> PathBuf {
        let medium = folder(name);
        fs::write(medium.join("vmlinuz"), vec![1u8; 5000]).unwrap();
        fs::write(medium.join("initramfs.cpio.gz"), vec![2u8; 9000]).unwrap();
        fs::write(medium.join("grub/boot.img"), vec![3u8; 512]).unwrap();
        fs::write(medium.join("grub/core.img"), vec![4u8; 30_000]).unwrap();
        fs::write(medium.join("grub/grub.cfg"), "the stick's own\n").unwrap();
        fs::write(medium.join("cmdline"), "quiet console=tty1\n").unwrap();
        medium
    }

    #[test]
    fn the_system_is_copied_with_a_configuration_of_its_own() {
        let medium = stick("copy-from");
        let target = folder("copy-to");

        let system = System::on(&medium).unwrap();
        assert_eq!(system.options, "quiet console=tty1");
        system.copy_to(&target).unwrap();

        assert_eq!(fs::read(target.join("vmlinuz")).unwrap().len(), 5000);
        assert_eq!(
            fs::read(target.join("initramfs.cpio.gz")).unwrap().len(),
            9000
        );
        assert_eq!(
            fs::read(target.join("grub/core.img")).unwrap().len(),
            30_000
        );
        // Not the stick's, which starts the installer.
        assert_eq!(
            fs::read_to_string(target.join("grub/grub.cfg")).unwrap(),
            "set timeout=0\nset gfxpayload=auto\nlinux /vmlinuz quiet console=tty1\ninitrd /initramfs.cpio.gz\nboot\n"
        );

        fs::remove_dir_all(medium).unwrap();
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn a_stick_missing_the_kernel_is_refused() {
        let medium = stick("no-kernel");
        fs::remove_file(medium.join("vmlinuz")).unwrap();
        assert!(System::on(&medium).is_err());
        fs::remove_dir_all(medium).unwrap();
    }

    #[test]
    fn without_a_cmdline_file_the_system_starts_quietly() {
        let medium = stick("no-cmdline");
        fs::remove_file(medium.join("cmdline")).unwrap();
        assert_eq!(System::on(&medium).unwrap().options, "quiet");
        fs::remove_dir_all(medium).unwrap();
    }
}

//! The part of the system that stays on the disk: the apps.
//!
//! The system runs from memory, and what is in the initramfs is in memory
//! for as long as the machine is on, opened or not. The apps, and what
//! only they run, are not needed to start the machine, so a stick and an
//! installed system keep them beside the initramfs as files,
//!
//! ```text
//!   disk/<version>/bin/apps/<name>     an app
//!   disk/<version>/bin/<name>          a program only apps run
//! ```
//!
//! in the file system the machine started from: the stick's, or the
//! system's partition on its disk. Here that file system is mounted, read
//! only, and what is in that `bin` is linked into `/bin` under its own
//! name, so that `/bin/apps` is where it always was. An app then takes
//! memory while it runs and none before.
//!
//! `<version>` is the system's own (`/etc/saule-release`): the apps are
//! bytecode for exactly the Saule in this initramfs, and an update writes
//! the folder for its version before it replaces the initramfs
//! (`../install/src/update.rs`), so the two never disagree, whenever the
//! power goes.
//!
//! An image that carries its apps in the initramfs (one started straight
//! from a kernel and an initramfs, with no disk under it; one an older
//! updater installed) has them in `/bin/apps` already, and nothing is
//! mounted.

use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{log, storage};

/// Where the file system the machine started from is mounted.
pub const MOUNT: &str = "/run/saule/disk";

/// The home of the Saule every program in the image runs on: its native
/// packages, once for all of them. Each program's `SAULE_HOME`.
pub const SAULE_HOME: &str = "/usr/lib/saule";

const BIN: &str = "/bin";
const APPS: &str = "/bin/apps";
const RELEASE: &str = "/etc/saule-release";

/// The labels of the system's partition on a disk and of a stick's file
/// system, as `../install/src/layout.rs` makes them.
const SYSTEM_LABEL: &str = "SAULE-BOOT";
const MEDIUM_LABEL: &str = "SAULE-INSTALL";

/// How long to wait for the file system. A disk the machine started from
/// is there already; a stick is found seconds into the kernel's start.
const DISK_APPEARS: Duration = Duration::from_secs(3);
const STICK_APPEARS: Duration = Duration::from_secs(8);

/// Find the apps kept on the disk and put them where apps are looked for.
/// `stick` when the machine was started from one. Never fails: without
/// them the system runs, and has no apps to open.
pub fn attach(stick: bool) {
    if has_entries(Path::new(APPS)) {
        return;
    }
    let version = fs::read_to_string(RELEASE).unwrap_or_default();
    let version = version.trim();
    if version.is_empty() {
        log::warn("no apps: the system says no version to find them by");
        return;
    }

    let (label, patience) = if stick {
        (MEDIUM_LABEL, STICK_APPEARS)
    } else {
        (SYSTEM_LABEL, DISK_APPEARS)
    };
    let deadline = Instant::now() + patience;
    loop {
        for device in storage::labelled_as(label) {
            if let Some(tree) = mounted_with(&device, version) {
                let linked = link_into(&tree.join("bin"), Path::new(BIN));
                log::info(&format!(
                    "apps: {} on {} ({linked} linked into {BIN})",
                    tree.strip_prefix(MOUNT).unwrap_or(&tree).display(),
                    device.display()
                ));
                return;
            }
        }
        if Instant::now() >= deadline {
            log::warn(&format!(
                "no apps: no file system labelled {label} has disk/{version}"
            ));
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Let go of the file system, before power goes. It was only read, so
/// nothing is lost if it can't be.
pub fn detach() {
    if let Ok(target) = CString::new(MOUNT) {
        // SAFETY: a plain system call with a NUL-terminated path.
        unsafe {
            libc::umount2(target.as_ptr(), libc::MNT_DETACH);
        }
    }
}

/// Mount `device` at [`MOUNT`] and return its folder for `version`; let
/// go of it again when it has none.
fn mounted_with(device: &Path, version: &str) -> Option<PathBuf> {
    if let Err(error) = mount(device) {
        log::warn(&format!("could not mount {}: {error}", device.display()));
        return None;
    }
    let tree = Path::new(MOUNT).join("disk").join(version);
    if tree.join("bin").is_dir() {
        return Some(tree);
    }
    detach();
    None
}

/// Read only, and nothing on it a device or a way to become root. Not
/// `noexec`: its programs are what it is for.
fn mount(device: &Path) -> std::io::Result<()> {
    fs::create_dir_all(MOUNT)?;
    let source = CString::new(device.as_os_str().as_encoded_bytes())?;
    let target = CString::new(MOUNT)?;
    // The kernel's ext4 driver reads ext2 as well.
    let fstype = CString::new("ext4")?;
    // SAFETY: NUL-terminated strings that outlive the call.
    let result = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype.as_ptr(),
            libc::MS_RDONLY | libc::MS_NOATIME | libc::MS_NOSUID | libc::MS_NODEV,
            std::ptr::null(),
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Link everything in `from` into `into` under its own name; how many
/// were. What is in `into` already stays: an empty folder aside, which is
/// what an image without apps has for `/bin/apps`.
fn link_into(from: &Path, into: &Path) -> usize {
    let Ok(entries) = fs::read_dir(from) else {
        return 0;
    };
    let mut linked = 0;
    for entry in entries.flatten() {
        let place = into.join(entry.file_name());
        if place.is_dir() && !place.is_symlink() && !has_entries(&place) {
            let _ = fs::remove_dir(&place);
        }
        if fs::symlink_metadata(&place).is_ok() {
            continue;
        }
        match std::os::unix::fs::symlink(entry.path(), &place) {
            Ok(()) => linked += 1,
            Err(error) => log::warn(&format!("could not link {}: {error}", place.display())),
        }
    }
    linked
}

fn has_entries(folder: &Path) -> bool {
    fs::read_dir(folder).is_ok_and(|mut entries| entries.next().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("saule-disk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn what_is_on_the_disk_is_linked_where_it_is_looked_for() {
        let work = folder("link");
        let (from, into) = (work.join("disk/bin"), work.join("bin"));
        fs::create_dir_all(from.join("apps")).unwrap();
        fs::write(from.join("apps/editor"), b"an app").unwrap();
        fs::write(from.join("saule-pdf"), b"a tool").unwrap();
        fs::write(from.join("session"), b"the disk's").unwrap();
        // The image's own: an empty folder for apps, and its session.
        fs::create_dir_all(into.join("apps")).unwrap();
        fs::write(into.join("session"), b"the image's").unwrap();

        assert_eq!(link_into(&from, &into), 2);
        assert_eq!(fs::read(into.join("apps/editor")).unwrap(), b"an app");
        assert_eq!(fs::read(into.join("saule-pdf")).unwrap(), b"a tool");
        assert!(into.join("apps").is_symlink());
        // What the image has is never replaced by what a disk says.
        assert_eq!(fs::read(into.join("session")).unwrap(), b"the image's");
        // A second time changes nothing.
        assert_eq!(link_into(&from, &into), 0);
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    fn a_folder_of_apps_the_image_has_is_not_replaced() {
        let work = folder("keep");
        let (from, into) = (work.join("disk/bin"), work.join("bin"));
        fs::create_dir_all(from.join("apps")).unwrap();
        fs::write(from.join("apps/editor"), b"the disk's").unwrap();
        fs::create_dir_all(into.join("apps")).unwrap();
        fs::write(into.join("apps/editor"), b"the image's").unwrap();

        assert_eq!(link_into(&from, &into), 0);
        assert_eq!(fs::read(into.join("apps/editor")).unwrap(), b"the image's");
        assert!(has_entries(&into.join("apps")));
        assert!(!has_entries(&work.join("nowhere")));
        fs::remove_dir_all(work).unwrap();
    }
}

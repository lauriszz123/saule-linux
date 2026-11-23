//! The kernel's own file systems, mounted before anything else runs.

use std::ffi::CString;

struct Mount {
    source: &'static str,
    target: &'static str,
    fstype: &'static str,
    flags: libc::c_ulong,
    options: &'static str,
}

const NOSUID_NODEV_NOEXEC: libc::c_ulong = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;

/// Mounted in this order. `/dev/pts` comes after `/dev`, which it lives in.
const MOUNTS: &[Mount] = &[
    Mount {
        source: "proc",
        target: "/proc",
        fstype: "proc",
        flags: NOSUID_NODEV_NOEXEC,
        options: "",
    },
    Mount {
        source: "sysfs",
        target: "/sys",
        fstype: "sysfs",
        flags: NOSUID_NODEV_NOEXEC,
        options: "",
    },
    Mount {
        source: "devtmpfs",
        target: "/dev",
        fstype: "devtmpfs",
        flags: libc::MS_NOSUID,
        options: "mode=0755",
    },
    // Pseudo-terminals: a terminal app's shell runs behind one
    // (`Os.spawnPty`). `ptmxmode` opens `/dev/pts/ptmx` to everyone, as the
    // `/dev/ptmx` devtmpfs makes already is.
    Mount {
        source: "devpts",
        target: "/dev/pts",
        fstype: "devpts",
        flags: libc::MS_NOSUID | libc::MS_NOEXEC,
        options: "mode=0620,ptmxmode=0666",
    },
    Mount {
        source: "tmpfs",
        target: "/run",
        fstype: "tmpfs",
        flags: libc::MS_NOSUID | libc::MS_NODEV,
        options: "mode=0755",
    },
    Mount {
        source: "tmpfs",
        target: "/tmp",
        fstype: "tmpfs",
        flags: libc::MS_NOSUID | libc::MS_NODEV,
        options: "mode=1777",
    },
];

pub fn mount_system() {
    for mount in MOUNTS {
        if let Err(error) = mount_one(mount) {
            crate::log::warn(&format!("could not mount {}: {error}", mount.target));
        }
    }
}

fn mount_one(mount: &Mount) -> std::io::Result<()> {
    std::fs::create_dir_all(mount.target)?;

    let source = CString::new(mount.source)?;
    let target = CString::new(mount.target)?;
    let fstype = CString::new(mount.fstype)?;
    let options = CString::new(mount.options)?;

    // SAFETY: every pointer is a NUL-terminated string that lives across the call.
    let result = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype.as_ptr(),
            mount.flags,
            options.as_ptr().cast(),
        )
    };

    if result == 0 {
        return Ok(());
    }

    let error = std::io::Error::last_os_error();

    // Already there — the kernel mounts devtmpfs itself on some setups.
    if error.raw_os_error() == Some(libc::EBUSY) {
        return Ok(());
    }

    Err(error)
}

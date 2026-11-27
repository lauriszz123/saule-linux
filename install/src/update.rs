//! Updating an installed system from a file.
//!
//! An update is one file: the kernel, the initramfs and every graphics
//! driver pack, as the stick carries them, with a signature at its end.
//!
//! ```text
//!   SAULE-UPDATE 1
//!   version 2026.11.25
//!   file vmlinuz 8617472
//!   file initramfs.cpio.gz 44150298
//!   file drivers/intel.cpio.gz 31006213
//!   end
//!   …the files' bytes, one after another, in that order…
//!   …64 bytes: the signature of everything before it…
//! ```
//!
//! Whoever can make the machine take an update can replace its kernel, so
//! an update is only taken when it is signed (Ed25519, over the SHA-512
//! of the file) with the key whose public half is built into this program
//! (`config/update.pub`). The private half is the builder's, and is not
//! in the repository.
//!
//! Applying one writes the new files beside the old in the system's
//! partition, flushes them, and only then renames them over the old: a
//! power cut leaves the old system or the new one, and a kernel and an
//! initramfs of two versions for no longer than the two renames take.
//! Of the driver packs, the one this machine has is replaced. The kernel
//! options the machine starts with are left as they are.
//!
//! **The apps** are kept on the disk and not in memory
//! (`../init/src/disk.rs`): `disk/<version>/` in the system's partition,
//! beside the initramfs of that version. In an update they are the first
//! part of the file called `initramfs.cpio.gz`, as an archive of their own
//! that is not compressed (`cpio.rs`), with the initramfs after it:
//!
//! ```text
//!   bin/apps/<name>…, bin/<name>…        the archive, to a whole 512 bytes
//!   the initramfs, compressed            the rest
//! ```
//!
//! Applying one unpacks the archive into `disk/<version>/` and keeps the
//! rest as the initramfs. The folder is whole on the disk before the
//! initramfs is replaced, and the one for the version running is left
//! until a later update, so each initramfs finds its own apps whenever
//! the power goes. The file is laid out so because an installed system
//! is updated by the updater it has: one from before the apps were kept
//! on the disk knows the four names above and no others, writes that
//! file whole, and the kernel unpacks both parts into memory, where that
//! system's apps always were. It keeps them on the disk from its next
//! update on.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha512};

use crate::cpio;
use crate::disks;
use crate::layout::{self, SYSTEM_LABEL};
use crate::sys::Mounted;

const MAGIC: &str = "SAULE-UPDATE 1";
const SIGNATURE: u64 = 64;
const TARGET_AT: &str = "/run/saule/update";
/// The file the apps travel in front of, and where they are kept.
const INITRAMFS: &str = "initramfs.cpio.gz";
const DISK: &str = "disk";
/// What the running system says its version is.
const RELEASE: &str = "/etc/saule-release";

/// The public half of the key updates are signed with, in hexadecimal.
const PUBLIC: &str = include_str!("../../config/update.pub");

/// What an update says of itself before its files.
#[derive(Debug, PartialEq, Eq)]
pub struct Header {
    pub version: String,
    /// Each file's name and length, in the order their bytes follow.
    pub files: Vec<(String, u64)>,
    /// Where the first file's bytes start.
    body_at: u64,
}

fn refuse<T>(why: impl Into<String>) -> io::Result<T> {
    Err(io::Error::other(why.into()))
}

/// Whether `name` is one of the files an update may carry: nothing that
/// could be written anywhere but where the system's files go.
fn allowed(name: &str) -> bool {
    if matches!(name, "vmlinuz" | "initramfs.cpio.gz" | "drivers.list") {
        return true;
    }
    name.strip_prefix("drivers/")
        .and_then(|pack| pack.strip_suffix(".cpio.gz"))
        .is_some_and(|pack| {
            !pack.is_empty()
                && pack
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

fn read_header(file: &mut File) -> io::Result<Header> {
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(&mut *file);
    let mut at = 0u64;
    let mut line = String::new();
    let mut next = |line: &mut String, at: &mut u64| -> io::Result<String> {
        line.clear();
        let read = reader.by_ref().take(512).read_line(line)?;
        if read == 0 || !line.ends_with('\n') {
            return refuse("This is not an update for Saule Linux.");
        }
        *at += read as u64;
        Ok(line.trim_end().to_string())
    };

    if next(&mut line, &mut at)? != MAGIC {
        return refuse("This is not an update for Saule Linux.");
    }
    let mut version = String::new();
    let mut files = Vec::new();
    loop {
        let said = next(&mut line, &mut at)?;
        if said == "end" {
            break;
        }
        let mut words = said.split(' ');
        match (words.next(), words.next(), words.next()) {
            (Some("version"), Some(v), None) => version = v.to_string(),
            (Some("file"), Some(name), Some(length)) if allowed(name) => {
                let length = length
                    .parse::<u64>()
                    .or_else(|_| refuse("The update file is damaged."))?;
                files.push((name.to_string(), length));
            }
            _ => return refuse("The update file is damaged."),
        }
        if files.len() > 64 {
            return refuse("The update file is damaged.");
        }
    }
    Ok(Header {
        version,
        files,
        body_at: at,
    })
}

/// The SHA-512 of all of `file` but the signature at its end, and the
/// signature.
fn digest_and_signature(file: &mut File) -> io::Result<(Sha512, [u8; 64])> {
    let length = file.metadata()?.len();
    if length < SIGNATURE {
        return refuse("The update file is damaged.");
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha512::new();
    let mut left = length - SIGNATURE;
    let mut buffer = vec![0u8; 1 << 20];
    while left > 0 {
        let want = left.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..want])?;
        hash.update(&buffer[..want]);
        left -= want as u64;
    }
    let mut signature = [0u8; 64];
    file.read_exact(&mut signature)?;
    Ok((hash, signature))
}

fn key_from_hex(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read `file`'s header, having made sure the whole file is as its maker
/// signed it. `public` is the key to check against, in hexadecimal.
pub fn check(file: &mut File, public: &str) -> io::Result<Header> {
    let Some(key) = key_from_hex(public).and_then(|key| VerifyingKey::from_bytes(&key).ok()) else {
        return refuse("This system has no key to check updates with.");
    };
    let (hash, signature) = digest_and_signature(file)?;
    let signature = Signature::from_bytes(&signature);
    if key.verify_prehashed(hash, None, &signature).is_err() {
        return refuse("The update is not signed by this system's maker, and was not installed.");
    }
    let header = read_header(file)?;
    let length = file.metadata()?.len();
    let files: u64 = header.files.iter().map(|(_, length)| length).sum();
    if header.body_at + files + SIGNATURE != length {
        return refuse("The update file is damaged.");
    }
    Ok(header)
}

/// Make the update file `out` from the folder `root`, laid out as the
/// stick is, signed with the private key in `key_file`.
pub fn bundle(out: &Path, root: &Path, key_file: &Path, version: &str) -> io::Result<()> {
    let private = fs::read_to_string(key_file)?;
    let Some(private) = key_from_hex(&private) else {
        return refuse(format!("{} is not a signing key", key_file.display()));
    };
    let key = SigningKey::from_bytes(&private);

    let mut names = vec!["vmlinuz".to_string(), "initramfs.cpio.gz".to_string()];
    if root.join("drivers.list").is_file() {
        names.push("drivers.list".to_string());
    }
    let mut packs: Vec<String> = fs::read_dir(root.join("drivers"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| format!("drivers/{}", entry.file_name().to_string_lossy()))
        .filter(|name| allowed(name))
        .collect();
    packs.sort();
    names.extend(packs);

    // The apps a stick keeps beside its initramfs, in front of it.
    let apps = match root.join(DISK).is_dir() {
        true if !root.join(DISK).join(version).is_dir() => {
            return refuse(format!(
                "{} has apps, but none for version {version}",
                root.display()
            ));
        }
        true => cpio::pack(&root.join(DISK).join(version))?,
        false => Vec::new(),
    };

    let mut head = format!("{MAGIC}\nversion {version}\n");
    for name in &names {
        let mut length = fs::metadata(root.join(name))?.len();
        if name == INITRAMFS {
            length += apps.len() as u64;
        }
        head.push_str(&format!("file {name} {length}\n"));
    }
    head.push_str("end\n");

    let part = out.with_extension("part");
    let mut file = File::create(&part)?;
    let mut hash = Sha512::new();
    file.write_all(head.as_bytes())?;
    hash.update(head.as_bytes());
    let mut buffer = vec![0u8; 1 << 20];
    for name in &names {
        if name == INITRAMFS {
            file.write_all(&apps)?;
            hash.update(&apps);
        }
        let mut source = File::open(root.join(name))?;
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])?;
            hash.update(&buffer[..read]);
        }
    }
    let signature = key
        .sign_prehashed(hash, None)
        .or_else(|_| refuse("could not sign the update"))?;
    file.write_all(&signature.to_bytes())?;
    file.sync_all()?;
    fs::rename(&part, out)
}

/// Make a new key: the private half into `private`, for the builder to
/// keep, and the public half into `public`, for the system to check by.
pub fn keygen(private: &Path, public: &Path) -> io::Result<()> {
    if private.exists() {
        return refuse(format!(
            "{} is there already: updates signed with another key would not be taken",
            private.display()
        ));
    }
    let mut seed = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut seed)?;
    let key = SigningKey::from_bytes(&seed);
    if let Some(folder) = private.parent() {
        fs::create_dir_all(folder)?;
    }
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(private)?;
        writeln!(file, "{}", hex(&seed))?;
    }
    fs::write(public, format!("{}\n", hex(key.verifying_key().as_bytes())))
}

/// What an update file says it is, without applying it: checked first.
pub fn version_of(path: &Path) -> io::Result<String> {
    Ok(check(&mut File::open(path)?, PUBLIC)?.version)
}

/// Apply the update in `path` to this machine's system partition.
pub fn apply(path: &Path) -> io::Result<()> {
    println!("step verify");
    let mut file = File::open(path)?;
    let header = check(&mut file, PUBLIC)?;

    let device = disks::all().into_iter().find_map(|disk| {
        let (number, _) = disks::labelled_partition(&disk, SYSTEM_LABEL)?;
        Some(Path::new("/dev").join(layout::partition_name(&disk.name, number)))
    });
    let Some(device) = device else {
        return refuse("This computer has no installed Saule Linux to update.");
    };

    println!("step write");
    // Init keeps this partition mounted, read only, for the apps on it.
    let target = Mounted::writable(&device, Path::new(TARGET_AT))?;
    let running = fs::read_to_string(RELEASE).unwrap_or_default();
    let written = write_into(&mut file, &header, target.path(), running.trim());
    // SAFETY: a plain system call.
    unsafe { libc::sync() };
    target.unmount()?;
    written?;
    println!("done {}", header.version);
    Ok(())
}

/// Put the update's files in `target`, the system partition's folder:
/// each written beside its old self and flushed, then all of them
/// renamed into place. The apps in front of the initramfs go into the
/// folder for the update's version, which is in place before the
/// initramfs is. `running` is the version of the system doing this,
/// whose apps are left where they are.
fn write_into(file: &mut File, header: &Header, target: &Path, running: &str) -> io::Result<()> {
    // The driver packs this machine has: those are replaced, no others
    // are added.
    let has_pack = |name: &str| target.join(name).is_file();
    // A version is a folder's name here: an update that calls itself
    // anything else brings no apps.
    let named = !header.version.is_empty()
        && header
            .version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !header.version.starts_with('.');
    let apps = target.join(DISK).join(&header.version);
    let apps_part = target.join(DISK).join(format!("{}.new", header.version));
    let mut brought_apps = false;

    let mut made: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut at = header.body_at;
    let mut buffer = vec![0u8; 1 << 20];
    for (name, length) in &header.files {
        let wanted = !name.starts_with("drivers/") || has_pack(name);
        if wanted {
            let place = target.join(name);
            let part = target.join(format!("{name}.new"));
            if let Some(folder) = part.parent() {
                fs::create_dir_all(folder)?;
            }
            file.seek(SeekFrom::Start(at))?;
            let mut left = *length;

            // The apps, off the front of the initramfs.
            if name == INITRAMFS && named {
                let mut start = [0u8; 6];
                let peeked = file.read(&mut start)?;
                file.seek(SeekFrom::Start(at))?;
                if cpio::begins(&start[..peeked]) {
                    let _ = fs::remove_dir_all(&apps_part);
                    let mut reader = BufReader::new(&mut *file);
                    let read = cpio::unpack(&mut reader, left, &apps_part)
                        .or_else(|_| refuse("The update file is damaged."))?;
                    drop(reader);
                    file.seek(SeekFrom::Start(at + read))?;
                    left -= read;
                    brought_apps = true;
                }
            }

            let mut out = File::create(&part)?;
            while left > 0 {
                let want = left.min(buffer.len() as u64) as usize;
                file.read_exact(&mut buffer[..want])?;
                out.write_all(&buffer[..want])?;
                left -= want as u64;
            }
            out.sync_all()?;
            made.push((part, place));
        }
        at += length;
    }

    // The apps first: an initramfs is never there before its own are. The
    // running system's are the ones it has open, and an update to the
    // version it already is brings the same.
    if brought_apps {
        if header.version == running && apps.is_dir() {
            fs::remove_dir_all(&apps_part)?;
        } else {
            let _ = fs::remove_dir_all(&apps);
            fs::rename(&apps_part, &apps)?;
        }
        File::open(target.join(DISK))?.sync_all()?;
    }
    for (part, place) in &made {
        fs::rename(part, place)?;
    }
    fs::write(target.join("version"), format!("{}\n", header.version))?;
    File::open(target)?.sync_all()?;

    // Apps of versions that are neither running nor about to: an update
    // before the last one's.
    if brought_apps {
        for folder in fs::read_dir(target.join(DISK))?.flatten() {
            let name = folder.file_name();
            if name != header.version.as_str() && name != running {
                let _ = fs::remove_dir_all(folder.path());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("saule-update-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// A stick's folder, a key, and an update made of the one with the
    /// other. Returns the update's path and the public key.
    fn made(work: &Path, version: &str) -> (PathBuf, String) {
        let root = work.join("root");
        fs::create_dir_all(root.join("drivers")).unwrap();
        fs::write(root.join("vmlinuz"), b"new kernel").unwrap();
        fs::write(root.join("initramfs.cpio.gz"), vec![7u8; 3_000_000]).unwrap();
        fs::write(root.join("drivers.list"), b"8086:* intel\n").unwrap();
        fs::write(root.join("drivers/intel.cpio.gz"), b"new intel").unwrap();
        fs::write(root.join("drivers/gen3.cpio.gz"), b"new gen3").unwrap();
        fs::write(root.join("cmdline"), b"quiet\n").unwrap();
        fs::write(root.join("installer.cpio.gz"), b"not part of an update").unwrap();

        let (private, public) = (work.join("update.key"), work.join("update.pub"));
        if !private.exists() {
            keygen(&private, &public).unwrap();
        }
        let out = work.join("saule.update");
        bundle(&out, &root, &private, version).unwrap();
        (out, fs::read_to_string(public).unwrap())
    }

    #[test]
    fn an_update_replaces_the_system_and_this_machine_s_driver_pack() {
        let work = folder("apply");
        let (update, public) = made(&work, "2026.11.25");

        // The system as installed: an old kernel, and the one pack.
        let target = work.join("target");
        fs::create_dir_all(target.join("drivers")).unwrap();
        fs::create_dir_all(target.join("grub")).unwrap();
        fs::write(target.join("vmlinuz"), b"old kernel").unwrap();
        fs::write(target.join("initramfs.cpio.gz"), b"old initramfs").unwrap();
        fs::write(target.join("drivers/intel.cpio.gz"), b"old intel").unwrap();
        fs::write(target.join("grub/grub.cfg"), b"as it was").unwrap();
        fs::write(target.join("cmdline"), b"quiet video=x\n").unwrap();

        let mut file = File::open(&update).unwrap();
        let header = check(&mut file, &public).unwrap();
        assert_eq!(header.version, "2026.11.25");
        assert_eq!(header.files.len(), 5);
        write_into(&mut file, &header, &target, "2026.11.01").unwrap();

        assert_eq!(fs::read(target.join("vmlinuz")).unwrap(), b"new kernel");
        assert_eq!(
            fs::read(target.join("initramfs.cpio.gz")).unwrap().len(),
            3_000_000
        );
        assert_eq!(
            fs::read(target.join("drivers/intel.cpio.gz")).unwrap(),
            b"new intel"
        );
        // A pack this machine doesn't use isn't added, and what starts
        // the machine is left as it was.
        assert!(!target.join("drivers/gen3.cpio.gz").exists());
        assert_eq!(
            fs::read(target.join("grub/grub.cfg")).unwrap(),
            b"as it was"
        );
        assert_eq!(
            fs::read(target.join("cmdline")).unwrap(),
            b"quiet video=x\n"
        );
        assert_eq!(
            fs::read_to_string(target.join("version")).unwrap(),
            "2026.11.25\n"
        );
        // Nothing half-written is left.
        assert!(!target.join("vmlinuz.new").exists());
        fs::remove_dir_all(work).unwrap();
    }

    /// A stick's folder with apps beside its initramfs, for `version`.
    fn with_apps(work: &Path, version: &str) {
        use std::os::unix::fs::PermissionsExt;
        let bin = work.join("root/disk").join(version).join("bin");
        fs::create_dir_all(bin.join("apps")).unwrap();
        fs::write(bin.join("apps/editor"), format!("editor {version}")).unwrap();
        fs::write(bin.join("apps/files"), vec![3u8; 5000]).unwrap();
        fs::write(bin.join("saule-pdf"), b"draws pages").unwrap();
        for program in ["apps/editor", "apps/files", "saule-pdf"] {
            fs::set_permissions(bin.join(program), fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn the_apps_go_on_the_disk_and_the_rest_is_the_initramfs() {
        use std::os::unix::fs::PermissionsExt;
        let work = folder("apps");
        fs::create_dir_all(work.join("root")).unwrap();
        with_apps(&work, "2026.12.01");
        let (update, public) = made(&work, "2026.12.01");

        // Installed, two updates ago: the running system's apps, and
        // those of the one before it.
        let target = work.join("target");
        for old in ["2026.10.01", "2026.11.01"] {
            fs::create_dir_all(target.join("disk").join(old).join("bin/apps")).unwrap();
            fs::write(target.join("disk").join(old).join("bin/apps/editor"), old).unwrap();
        }
        fs::write(target.join("vmlinuz"), b"old kernel").unwrap();
        fs::write(target.join("initramfs.cpio.gz"), b"old initramfs").unwrap();

        let mut file = File::open(&update).unwrap();
        let header = check(&mut file, &public).unwrap();
        write_into(&mut file, &header, &target, "2026.11.01").unwrap();

        // The initramfs is the stick's own, with nothing in front of it.
        assert_eq!(
            fs::read(target.join("initramfs.cpio.gz")).unwrap(),
            vec![7u8; 3_000_000]
        );
        let apps = target.join("disk/2026.12.01/bin");
        assert_eq!(
            fs::read(apps.join("apps/editor")).unwrap(),
            b"editor 2026.12.01"
        );
        assert_eq!(fs::read(apps.join("apps/files")).unwrap().len(), 5000);
        assert_eq!(fs::read(apps.join("saule-pdf")).unwrap(), b"draws pages");
        let mode = fs::metadata(apps.join("apps/editor"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        // The running system's apps stay; the ones before them go.
        assert!(target.join("disk/2026.11.01/bin/apps/editor").exists());
        assert!(!target.join("disk/2026.10.01").exists());
        assert!(!target.join("disk/2026.12.01.new").exists());

        // The same update again, by the system it made: its apps are the
        // ones in use, and are left as they are.
        fs::write(apps.join("apps/editor"), b"in use").unwrap();
        write_into(&mut file, &header, &target, "2026.12.01").unwrap();
        assert_eq!(fs::read(apps.join("apps/editor")).unwrap(), b"in use");
        assert!(!target.join("disk/2026.11.01").exists());
        assert!(!target.join("disk/2026.12.01.new").exists());
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    fn an_older_updater_takes_the_same_file_and_starts_it_whole() {
        let work = folder("older");
        fs::create_dir_all(work.join("root")).unwrap();
        with_apps(&work, "2026.12.01");
        let (update, public) = made(&work, "2026.12.01");

        // Nothing in it but the names every updater has known.
        let mut file = File::open(&update).unwrap();
        let header = check(&mut file, &public).unwrap();
        let names: Vec<&str> = header.files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "vmlinuz",
                "initramfs.cpio.gz",
                "drivers.list",
                "drivers/gen3.cpio.gz",
                "drivers/intel.cpio.gz"
            ]
        );

        // What such an updater writes as the initramfs: the file whole,
        // which is two archives the kernel unpacks one after the other,
        // the second starting on a whole four bytes.
        let (_, length) = header.files[1];
        let start = header.body_at + header.files[0].1;
        let mut whole = vec![0u8; length as usize];
        file.seek(SeekFrom::Start(start)).unwrap();
        file.read_exact(&mut whole).unwrap();
        let apps = cpio::pack(&work.join("root/disk/2026.12.01")).unwrap();
        assert!(cpio::begins(&whole));
        assert_eq!(apps.len() % 4, 0);
        assert_eq!(&whole[..apps.len()], &apps[..]);
        assert_eq!(&whole[apps.len()..], &vec![7u8; 3_000_000][..]);
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    fn apps_for_another_version_than_the_update_s_are_a_mistake() {
        let work = folder("mismatch");
        fs::create_dir_all(work.join("root")).unwrap();
        with_apps(&work, "2026.12.01");
        let root = work.join("root");
        fs::create_dir_all(root.join("drivers")).unwrap();
        fs::write(root.join("vmlinuz"), b"k").unwrap();
        fs::write(root.join("initramfs.cpio.gz"), b"i").unwrap();
        keygen(&work.join("update.key"), &work.join("update.pub")).unwrap();
        let out = work.join("saule.update");
        assert!(bundle(&out, &root, &work.join("update.key"), "2026.12.02").is_err());
        assert!(bundle(&out, &root, &work.join("update.key"), "2026.12.01").is_ok());
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    fn an_update_changed_or_signed_by_another_is_refused() {
        let work = folder("refuse");
        let (update, public) = made(&work, "1");

        // One byte of the kernel changed.
        let mut bytes = fs::read(&update).unwrap();
        let at = bytes.windows(10).position(|w| w == b"new kernel").unwrap();
        bytes[at] ^= 1;
        let changed = work.join("changed.update");
        fs::write(&changed, &bytes).unwrap();
        let error = check(&mut File::open(&changed).unwrap(), &public).unwrap_err();
        assert!(error.to_string().contains("not signed"));

        // Another key's public half.
        let other = folder("refuse-other");
        keygen(&other.join("k"), &other.join("p")).unwrap();
        let theirs = fs::read_to_string(other.join("p")).unwrap();
        assert!(check(&mut File::open(&update).unwrap(), &theirs).is_err());

        // Cut short, and not an update at all.
        fs::write(work.join("short"), &bytes[..40]).unwrap();
        assert!(check(&mut File::open(work.join("short")).unwrap(), &public).is_err());
        assert!(check(&mut File::open(&update).unwrap(), "not a key").is_err());

        // The real one is taken.
        assert!(check(&mut File::open(&update).unwrap(), &public).is_ok());
        fs::remove_dir_all(work).unwrap();
        fs::remove_dir_all(other).unwrap();
    }

    #[test]
    fn only_the_system_s_own_files_can_be_in_an_update() {
        assert!(allowed("vmlinuz"));
        assert!(allowed("initramfs.cpio.gz"));
        assert!(allowed("drivers/intel.cpio.gz"));
        assert!(!allowed("grub/grub.cfg"));
        assert!(!allowed("drivers/../../etc/passwd.cpio.gz"));
        assert!(!allowed("drivers/.cpio.gz"));
        assert!(!allowed("/vmlinuz"));
        assert!(!allowed("cmdline"));
    }

    #[test]
    fn a_key_is_not_made_over_another() {
        let work = folder("keygen");
        keygen(&work.join("update.key"), &work.join("update.pub")).unwrap();
        assert!(keygen(&work.join("update.key"), &work.join("update.pub")).is_err());
        assert_eq!(
            fs::read_to_string(work.join("update.pub"))
                .unwrap()
                .trim()
                .len(),
            64
        );
        fs::remove_dir_all(work).unwrap();
    }
}

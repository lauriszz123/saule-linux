//! saule-install — puts Saule Linux on a disk.
//!
//! The installer's window is a Saule program; this is what it runs to
//! look at the machine's disks and to write to one. It is also what the
//! build machine makes a USB stick's image with, so the disk a stick
//! boots from and the disk it installs are laid out by the same code.
//!
//!   saule-install disks
//!       every disk, one per line, tab-separated:
//!       name, bytes, `internal` or `usb`, what is on it, model
//!
//!   saule-install install DISK [--keep-data] [--answers FILE] [--from FOLDER]
//!       erase DISK (`sda`) and install onto it; see `target.rs` for what
//!       it prints. Needs root.
//!
//!   saule-install image OUT ROOT --efi IMAGE [--megabytes N]
//!       make the image OUT of a stick holding the folder ROOT, with the
//!       file system IMAGE as its partition for UEFI. Needs only `mke2fs`.
//!
//!   saule-install update FILE
//!       replace the installed system with the one in the update FILE, if
//!       it is signed with the key this was built with (`update.rs`).
//!       `update-version FILE` checks one and prints its version.
//!
//!   saule-install bundle OUT ROOT --key KEY --version VERSION
//!       make the update file OUT from the folder ROOT, laid out as the
//!       stick is, signed with the private key in KEY. `keygen PRIVATE
//!       PUBLIC` makes a key.
//!
//!   saule-install live-drivers
//!       unpack the graphics driver pack this machine's chip needs over
//!       the running system, from the stick. Init runs it before the
//!       installer starts. Needs root.
//!
//!   saule-install config OPTIONS [OVERLAY...]
//!       GRUB's configuration for a kernel started with OPTIONS and these
//!       archives unpacked over the initramfs: what the build machine
//!       writes on a stick.
//!
//!   saule-install driver
//!       the graphics driver pack this machine's chip needs, going by
//!       the list given on standard input.

mod answers;
mod cpio;
mod disks;
mod drivers;
mod grub;
mod image;
mod layout;
mod sys;
mod target;
mod update;

use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match words.as_slice() {
        ["disks"] => {
            for disk in disks::all() {
                println!("{}", disk.line());
            }
            Ok(())
        }
        ["install", disk, options @ ..] => install(disk, options),
        ["image", out, root, options @ ..] => image(out, root, options),
        ["update", file] => update::apply(Path::new(file)),
        ["update-version", file] => update::version_of(Path::new(file)).map(|v| println!("{v}")),
        ["bundle", out, root, "--key", key, "--version", version] => {
            update::bundle(Path::new(out), Path::new(root), Path::new(key), version)
        }
        ["keygen", private, public] => update::keygen(Path::new(private), Path::new(public)),
        ["live-drivers"] => target::live_drivers(),
        ["config", options, overlays @ ..] => {
            let overlays: Vec<String> = overlays.iter().map(|o| o.to_string()).collect();
            print!("{}", grub::config(options, &overlays));
            Ok(())
        }
        ["driver"] => {
            let mut list = String::new();
            let _ = std::io::stdin().read_to_string(&mut list);
            match drivers::pack_for(&list, &drivers::chips(), |_| true) {
                Some(pack) => println!("{pack}"),
                None => println!("none"),
            }
            Ok(())
        }
        _ => {
            eprintln!("usage: saule-install disks");
            eprintln!(
                "       saule-install install DISK [--keep-data] [--answers FILE] [--from FOLDER]"
            );
            eprintln!("       saule-install image OUT ROOT --efi IMAGE [--megabytes N]");
            eprintln!("       saule-install update FILE");
            eprintln!("       saule-install bundle OUT ROOT --key KEY --version VERSION");
            eprintln!("       saule-install keygen PRIVATE PUBLIC");
            eprintln!("       saule-install live-drivers");
            eprintln!("       saule-install config OPTIONS [OVERLAY...]");
            eprintln!("       saule-install driver < drivers.list");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn install(disk: &str, options: &[&str]) -> std::io::Result<()> {
    let mut request = target::Request {
        disk,
        keep_data: false,
        from: None,
        answers: answers::Answers::default(),
    };
    let mut rest = options.iter();
    while let Some(option) = rest.next() {
        match *option {
            "--keep-data" => request.keep_data = true,
            "--answers" => match rest.next() {
                Some(file) => request.answers = answers::Answers::take(Path::new(file))?,
                None => return Err(std::io::Error::other("--answers needs a file")),
            },
            "--from" => match rest.next() {
                Some(folder) => request.from = Some(Path::new(folder)),
                None => return Err(std::io::Error::other("--from needs a folder")),
            },
            other => return Err(std::io::Error::other(format!("unknown option {other}"))),
        }
    }
    target::install(&request)
}

fn image(out: &str, root: &str, options: &[&str]) -> std::io::Result<()> {
    let mut megabytes = None;
    let mut efi = None;
    let mut rest = options.iter();
    while let Some(option) = rest.next() {
        match (*option, rest.next()) {
            ("--megabytes", Some(count)) => {
                megabytes = Some(
                    count
                        .parse()
                        .map_err(|_| std::io::Error::other("--megabytes needs a number"))?,
                );
            }
            ("--efi", Some(file)) => efi = Some(Path::new(file)),
            (other, _) => {
                return Err(std::io::Error::other(format!(
                    "unknown option {other}, or nothing after it"
                )));
            }
        }
    }
    let Some(efi) = efi else {
        return Err(std::io::Error::other("--efi IMAGE is needed"));
    };
    let bytes = image::make(Path::new(out), Path::new(root), efi, megabytes)?;
    println!("image: {out} ({} MB)", bytes >> 20);
    Ok(())
}

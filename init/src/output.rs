//! What the session prints: on every console, and kept.
//!
//! A session that draws covers the terminal it runs on, and with more
//! than one console (the screen and a serial line, as QEMU and a debugging
//! setup have) its errors, the one thing needed when it fails, would be on
//! a console nobody could read. So its output goes through a pipe that
//! init copies to every console, and to a file ([`KEPT`]) that a report
//! of what went wrong is made from.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;

/// The pipe a session's output goes through on its way to every console.
pub struct Output {
    read_end: libc::c_int,
    write_end: libc::c_int,
    consoles: Vec<String>,
}

impl Output {
    /// A pipe to `consoles`, however many there are, and to the file.
    pub fn for_consoles(consoles: Vec<String>) -> std::io::Result<Option<Output>> {
        let mut fds = [0; 2];
        // Close-on-exec: the session gets the write end as its stdout and
        // stderr (`dup2` clears the flag on those), and nothing else it
        // runs inherits the originals.
        // SAFETY: `fds` has room for the two descriptors pipe2 writes.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Some(Output {
            read_end: fds[0],
            write_end: fds[1],
            consoles,
        }))
    }

    /// The end the session writes to, to `dup2` over its stdout and stderr.
    pub fn write_end(&self) -> libc::c_int {
        self.write_end
    }

    /// The session never started: nothing will use the pipe.
    pub fn close(self) {
        // SAFETY: both were made by `for_consoles` and nothing else closes
        // them.
        unsafe {
            libc::close(self.read_end);
            libc::close(self.write_end);
        }
    }

    /// Once the session has started: let go of init's write end, so the
    /// pipe ends when the session and the apps it started have closed
    /// theirs, and copy what arrives to every console until then.
    pub fn copy_in_background(self) {
        // SAFETY: both descriptors were made by `for_consoles` and are
        // owned here; each is closed exactly once (`File` closes the read
        // end on drop).
        unsafe { libc::close(self.write_end) };
        let mut pipe = unsafe { File::from_raw_fd(self.read_end) };

        let mut consoles: Vec<File> = self
            .consoles
            .iter()
            .filter_map(|path| OpenOptions::new().write(true).open(path).ok())
            .collect();

        std::thread::spawn(move || {
            let mut kept = Kept::open();
            let mut buffer = [0u8; 4096];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        for console in &mut consoles {
                            let _ = console.write_all(&buffer[..n]);
                        }
                        kept.write(&buffer[..n]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
    }
}

/// Where what the session writes is kept as well, for a report of what
/// went wrong (saule-system's `report`): the consoles are behind the
/// desktop, and scroll away.
pub const KEPT: &str = "/run/saule/session.log";

/// The most of it kept, in bytes. Past that the file is set aside as
/// `session.log.old` (replacing the one before) and begun again, so the
/// latest half a megabyte or more is always there and memory, which is
/// where `/run` is, is not filled.
const KEPT_BYTES: usize = 512 * 1024;

struct Kept {
    file: Option<File>,
    written: usize,
}

impl Kept {
    fn open() -> Kept {
        let _ = std::fs::create_dir_all("/run/saule");
        Kept {
            file: File::create(KEPT).ok(),
            written: 0,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.written + bytes.len() > KEPT_BYTES {
            let _ = std::fs::rename(KEPT, format!("{KEPT}.old"));
            self.file = File::create(KEPT).ok();
            self.written = 0;
        }
        if let Some(file) = &mut self.file {
            let _ = file.write_all(bytes);
            self.written += bytes.len();
        }
    }
}

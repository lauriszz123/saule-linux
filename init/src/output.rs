//! What the session prints, on every console.
//!
//! With more than one console — the screen and a serial line, as QEMU and
//! a debugging setup have — the session runs on the screen's terminal, and
//! a session that draws covers that terminal: its errors, the one thing
//! needed when it fails, were on a console nobody could read. So its output
//! goes through a pipe that init copies to every console. With one console
//! it writes to its terminal directly, and there is no pipe.

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
    /// A pipe to `consoles` when there is more than one of them, `None`
    /// when the session's own terminal is the only console.
    pub fn for_consoles(consoles: Vec<String>) -> std::io::Result<Option<Output>> {
        if consoles.len() < 2 {
            return Ok(None);
        }

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
            let mut buffer = [0u8; 4096];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        for console in &mut consoles {
                            let _ = console.write_all(&buffer[..n]);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
    }
}

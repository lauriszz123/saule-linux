//! Requests: files dropped in a folder, each a line of text.

use std::ffi::CString;
use std::fs;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::Path;

use crate::files;

/// The longest request read: a Wi-Fi network's name and password fit in
/// far less.
const LONGEST: u64 = 1024;

/// Act on every request, as it comes, for as long as the machine runs.
pub fn serve(mut act: impl FnMut(&str)) -> ! {
    let folder = files::requests();
    let watch = watch(&folder);
    if watch.is_none() {
        eprintln!("saule-system: can't watch for requests; looking twice a second instead");
    }

    let mut buffer = [0u8; 4096];
    loop {
        for request in take(&folder) {
            act(&request);
        }

        match &watch {
            Some(file) => {
                // Blocks until something is dropped in the folder. What
                // the events say is not looked at: the folder is.
                // SAFETY: reads into a buffer of that length.
                unsafe {
                    libc::read(file.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len());
                }
            }
            None => std::thread::sleep(std::time::Duration::from_millis(500)),
        }
    }
}

fn watch(folder: &Path) -> Option<fs::File> {
    let path = CString::new(folder.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: plain system calls; the path is NUL-terminated.
    unsafe {
        let fd = libc::inotify_init1(libc::IN_CLOEXEC);
        if fd < 0 {
            return None;
        }
        let file = fs::File::from_raw_fd(fd);
        let events = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO;
        (libc::inotify_add_watch(fd, path.as_ptr(), events) >= 0).then_some(file)
    }
}

/// Every request waiting in `folder`, oldest first, each removed as it is
/// read. A file still being written (`.part`, renamed when whole) is left.
pub fn take(folder: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut waiting: Vec<_> = entries
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".part"))
        .map(|entry| {
            let made = entry.metadata().and_then(|m| m.modified()).ok();
            (made, entry.path())
        })
        .collect();
    waiting.sort();

    let mut requests = Vec::new();
    for (_, path) in waiting {
        use std::io::Read;
        let mut text = String::new();
        if let Ok(file) = fs::File::open(&path) {
            let _ = file.take(LONGEST).read_to_string(&mut text);
        }
        let _ = fs::remove_file(&path);
        if let Some(line) = text.lines().next().map(str::trim).filter(|l| !l.is_empty()) {
            requests.push(line.to_string());
        }
    }
    requests
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_taken_once_and_whole_ones_only() {
        let folder = std::env::temp_dir().join(format!("saule-system-req-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("1"), "volume 40\nsecond line ignored\n").unwrap();
        fs::write(folder.join("2.part"), "sle").unwrap();
        fs::write(folder.join("3"), "\n").unwrap();

        assert_eq!(take(&folder), ["volume 40"]);
        // Taken means gone; the half-written one is still there.
        assert!(take(&folder).is_empty());
        assert!(folder.join("2.part").exists());
        assert!(!folder.join("1").exists());
        assert!(!folder.join("3").exists());

        fs::remove_dir_all(folder).unwrap();
    }
}

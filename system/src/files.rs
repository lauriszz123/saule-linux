//! The folder this is spoken to through, and writing how things stand.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const FOLDER: &str = "/run/saule/system";

pub fn requests() -> PathBuf {
    Path::new(FOLDER).join("requests")
}

/// Make the folder: readable by all, and its `requests` the session
/// account's alone (`owner`: its user and group ids).
pub fn prepare(owner: Option<(u32, u32)>) -> io::Result<()> {
    fs::create_dir_all(FOLDER)?;
    fs::set_permissions(FOLDER, fs::Permissions::from_mode(0o755))?;
    if let Some(parent) = Path::new(FOLDER).parent() {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o755))?;
    }

    let requests = requests();
    fs::create_dir_all(&requests)?;
    fs::set_permissions(&requests, fs::Permissions::from_mode(0o700))?;
    if let Some((uid, gid)) = owner {
        let path = CString::new(requests.as_os_str().as_encoded_bytes())?;
        // SAFETY: a NUL-terminated path that outlives the call.
        if unsafe { libc::chown(path.as_ptr(), uid, gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Say how the thing called `name` stands: `lines`, each a name and its
/// value. Written beside and renamed over, so a reader never sees half.
pub fn say(name: &str, lines: &[String]) {
    say_in(Path::new(FOLDER), name, lines);
}

pub fn say_in(folder: &Path, name: &str, lines: &[String]) {
    let path = folder.join(name);
    let part = folder.join(format!(".{name}.part"));
    let mut text = lines.join("\n");
    text.push('\n');

    // Unchanged: left alone, so a reader watching the file's time sees
    // changes and nothing else.
    if fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return;
    }
    if fs::write(&part, text).is_ok() {
        let _ = fs::set_permissions(&part, fs::Permissions::from_mode(0o644));
        let _ = fs::rename(&part, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_said_is_the_file_s_lines() {
        let folder = std::env::temp_dir().join(format!("saule-system-say-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();

        say_in(&folder, "sound", &["volume 60".into(), "muted 0".into()]);
        assert_eq!(
            fs::read_to_string(folder.join("sound")).unwrap(),
            "volume 60\nmuted 0\n"
        );

        say_in(&folder, "sound", &["volume 20".into()]);
        assert_eq!(
            fs::read_to_string(folder.join("sound")).unwrap(),
            "volume 20\n"
        );
        // Nothing half-written is left beside it.
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);

        fs::remove_dir_all(folder).unwrap();
    }
}

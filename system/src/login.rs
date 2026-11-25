//! The person of the machine: their name, and whether a password is
//! theirs.
//!
//! The installer asks who the machine is for and writes it on the data
//! disk, in `/data/account`: `name`, readable, and `password`, which only
//! root can read and which holds no password but what `saule-password`
//! makes of one. The session can't read that file, so it can't check a
//! password itself: it asks here, and is told yes or no.
//!
//! The folder is root's, on a disk whose top is root's: the session can
//! neither change what is in it nor put another in its place.
//!
//! How it stands, in `login`:
//!
//!   name Ada Lovelace     who the machine is for; nothing after the word
//!                         where nobody was named
//!   password 1|0          whether there is a password to ask for
//!   tried N right|wrong   how many passwords have been tried, and how the
//!                         last one fared. A lock screen asks, waits for N
//!                         to go up, and reads the answer
//!   changed N done|wrong|failed   the same for changes
//!
//! The requests (a tab between two things, as neither can hold one):
//!
//!   unlock PASSWORD
//!   password OLD<tab>NEW      NEW empty: no password from now on
//!   person NAME
//!
//! A wrong password costs a wait before the answer, and a longer one each
//! time in a row, so trying many takes long.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::files;

pub const FOLDER: &str = "/data/account";

/// The longest a row of wrong passwords makes the next answer wait.
const LONGEST_WAIT: Duration = Duration::from_secs(8);

pub struct Login {
    folder: PathBuf,
    tried: u64,
    last: &'static str,
    changed: u64,
    change: &'static str,
    /// Wrong passwords in a row.
    wrong: u32,
    /// Whether a wrong one is waited after: not in tests.
    waits: bool,
}

pub fn start() -> Login {
    let login = Login::new(Path::new(FOLDER), true);
    files::say("login", &login.lines());
    login
}

impl Login {
    fn new(folder: &Path, waits: bool) -> Login {
        Login {
            folder: folder.to_path_buf(),
            tried: 0,
            last: "none",
            changed: 0,
            change: "none",
            wrong: 0,
            waits,
        }
    }

    pub fn ask(&mut self, word: &str, rest: &str) {
        match word {
            "unlock" => self.unlock(rest),
            "password" => {
                let (old, new) = rest.split_once('\t').unwrap_or((rest, ""));
                self.change(old, new);
            }
            "person" => self.name(rest),
            _ => {}
        }
        files::say("login", &self.lines());
    }

    fn lines(&self) -> Vec<String> {
        vec![
            format!("name {}", self.person()),
            format!("password {}", u8::from(self.kept().is_some())),
            format!("tried {} {}", self.tried, self.last),
            format!("changed {} {}", self.changed, self.change),
        ]
    }

    fn person(&self) -> String {
        fs::read_to_string(self.folder.join("name"))
            .ok()
            .and_then(|text| text.lines().next().map(|line| line.trim().to_string()))
            .unwrap_or_default()
    }

    /// What is kept of the password, if there is one.
    fn kept(&self) -> Option<String> {
        fs::read_to_string(self.folder.join("password"))
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|line| !line.is_empty())
    }

    /// Whether `password` is the person's. With no password kept, any is.
    fn right(&mut self, password: &str) -> bool {
        let right = match self.kept() {
            Some(kept) => saule_password::is(&kept, password),
            None => true,
        };
        if right {
            self.wrong = 0;
        } else {
            self.wrong += 1;
            if self.waits {
                std::thread::sleep(wait_after(self.wrong));
            }
        }
        right
    }

    fn unlock(&mut self, password: &str) {
        self.last = if self.right(password) {
            "right"
        } else {
            "wrong"
        };
        self.tried += 1;
    }

    fn change(&mut self, old: &str, new: &str) {
        self.changed += 1;
        if !self.right(old) {
            self.change = "wrong";
            return;
        }
        let done = if new.is_empty() {
            let gone = fs::remove_file(self.folder.join("password"));
            gone.is_ok() || !self.folder.join("password").exists()
        } else {
            saule_password::keep(new)
                .and_then(|line| self.write("password", &line, 0o600))
                .is_ok()
        };
        self.change = if done { "done" } else { "failed" };
    }

    fn name(&mut self, name: &str) {
        let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
        if let Err(error) = self.write("name", name.trim(), 0o644) {
            eprintln!("saule-system: could not keep the person's name: {error}");
        }
    }

    /// Keep `line` as the file `name`, written beside and renamed over so
    /// that a power cut leaves the old one or the new one.
    fn write(&self, name: &str, line: &str, mode: u32) -> std::io::Result<()> {
        fs::create_dir_all(&self.folder)?;
        fs::set_permissions(&self.folder, fs::Permissions::from_mode(0o755))?;
        let part = self.folder.join(format!(".{name}.part"));
        fs::write(&part, format!("{line}\n"))?;
        fs::set_permissions(&part, fs::Permissions::from_mode(mode))?;
        fs::rename(&part, self.folder.join(name))
    }
}

/// How long the answer to the `wrong`th wrong password in a row waits:
/// one second, then twice as long each time, up to [`LONGEST_WAIT`].
fn wait_after(wrong: u32) -> Duration {
    let seconds = 1u64 << wrong.saturating_sub(1).min(8);
    Duration::from_secs(seconds).min(LONGEST_WAIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("saule-login-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn with_no_password_anything_unlocks() {
        let folder = folder("none");
        let mut login = Login::new(&folder, false);
        assert_eq!(login.lines()[1], "password 0");
        login.unlock("");
        assert_eq!(login.lines()[2], "tried 1 right");
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_password_is_set_asked_for_changed_and_removed() {
        let folder = folder("set");
        let mut login = Login::new(&folder, false);

        login.change("", "first one");
        assert_eq!(login.lines()[3], "changed 1 done");
        assert_eq!(login.lines()[1], "password 1");
        let mode = fs::metadata(folder.join("password"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // What is kept is not the password.
        assert!(!fs::read_to_string(folder.join("password"))
            .unwrap()
            .contains("first one"));

        login.unlock("first");
        assert_eq!(login.lines()[2], "tried 1 wrong");
        login.unlock("first one");
        assert_eq!(login.lines()[2], "tried 2 right");

        // Changing it takes the old one.
        login.change("guess", "second");
        assert_eq!(login.lines()[3], "changed 2 wrong");
        login.change("first one", "second");
        assert_eq!(login.lines()[3], "changed 3 done");
        login.unlock("first one");
        assert_eq!(login.lines()[2], "tried 3 wrong");
        login.unlock("second");
        assert_eq!(login.lines()[2], "tried 4 right");

        login.change("second", "");
        assert_eq!(login.lines()[3], "changed 4 done");
        assert_eq!(login.lines()[1], "password 0");
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn the_person_s_name_is_one_line_of_what_was_typed() {
        let folder = folder("name");
        let mut login = Login::new(&folder, false);
        assert_eq!(login.lines()[0], "name ");
        login.name("  Ada\tLovelace \u{7}");
        assert_eq!(login.lines()[0], "name AdaLovelace");
        login.name("Ada Lovelace");
        assert_eq!(login.lines()[0], "name Ada Lovelace");
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn wrong_passwords_in_a_row_wait_longer_and_longer() {
        assert_eq!(wait_after(1), Duration::from_secs(1));
        assert_eq!(wait_after(2), Duration::from_secs(2));
        assert_eq!(wait_after(3), Duration::from_secs(4));
        assert_eq!(wait_after(4), LONGEST_WAIT);
        assert_eq!(wait_after(400), LONGEST_WAIT);
    }
}

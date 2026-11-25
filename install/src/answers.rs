//! What the person told the installer, put on the new system's data disk.
//!
//! The installer's window asks who the machine is for, for a password and
//! for the keyboard's layout, before anything is written. It hands the
//! answers over in a file (`--answers FILE`), a line each:
//!
//! ```text
//!   person Ada Lovelace
//!   account ada
//!   password what was typed
//!   keyboard lt
//! ```
//!
//! not as arguments, which any program can read of any other. The file is
//! removed as soon as it has been read. On the data disk they become:
//!
//!   account/name        who the machine is for
//!   account/user        the short name their account goes by: what their
//!                       home is called, and what a terminal's prompt
//!                       says. Init renames the session's account to it
//!                       at each start (../init/src/person.rs)
//!   account/password    what `saule-password` keeps of the password, for
//!                       root alone to read; `saule-system` checks against
//!                       it (../system/src/login.rs)
//!   system/keyboard     the layout, which the window manager types with
//!
//! An answer not given changes nothing: installing again with the files
//! kept, and no name typed, leaves whose machine it was.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Answers {
    pub person: Option<String>,
    pub account: Option<String>,
    pub password: Option<String>,
    pub keyboard: Option<String>,
}

impl Answers {
    /// Read the answers in `file`, and remove it: it holds a password.
    pub fn take(file: &Path) -> io::Result<Answers> {
        let text = fs::read_to_string(file);
        let _ = fs::remove_file(file);
        Ok(Answers::parse(&text?))
    }

    fn parse(text: &str) -> Answers {
        let mut answers = Answers::default();
        for line in text.lines() {
            let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
            let value = rest.trim();
            if value.is_empty() {
                continue;
            }
            match word {
                "person" => answers.person = Some(value.to_string()),
                // A small letter, then small letters, digits, hyphens and
                // underscores: what init takes for an account's name.
                "account"
                    if value.len() <= 32
                        && value.starts_with(|c: char| c.is_ascii_lowercase())
                        && value.chars().all(|c| {
                            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
                        }) =>
                {
                    answers.account = Some(value.to_string());
                }
                "password" => answers.password = Some(value.to_string()),
                // A layout's name is a few letters, perhaps with a variant
                // in brackets: nothing that could name a file elsewhere.
                "keyboard"
                    if value
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "()_-".contains(c)) =>
                {
                    answers.keyboard = Some(value.to_string());
                }
                _ => {}
            }
        }
        answers
    }

    pub fn is_empty(&self) -> bool {
        *self == Answers::default()
    }

    /// Write them into the data disk mounted at `data`.
    pub fn keep(&self, data: &Path) -> io::Result<()> {
        if self.person.is_some() || self.password.is_some() || self.account.is_some() {
            let account = data.join("account");
            fs::create_dir_all(&account)?;
            fs::set_permissions(&account, fs::Permissions::from_mode(0o755))?;
            if let Some(person) = &self.person {
                write(&account.join("name"), person, 0o644)?;
            }
            if let Some(user) = &self.account {
                write(&account.join("user"), user, 0o644)?;
            }
            if let Some(password) = &self.password {
                write(
                    &account.join("password"),
                    &saule_password::keep(password)?,
                    0o600,
                )?;
            }
        }
        if let Some(keyboard) = &self.keyboard {
            let system = data.join("system");
            fs::create_dir_all(&system)?;
            write(&system.join("keyboard"), keyboard, 0o644)?;
        }
        Ok(())
    }
}

fn write(path: &Path, line: &str, mode: u32) -> io::Result<()> {
    // Made with its mode, not given it afterwards: the password's file is
    // never one others could have opened.
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = fs::remove_file(path);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let folder =
            std::env::temp_dir().join(format!("saule-answers-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn the_answers_are_read_and_their_file_is_gone() {
        let folder = folder("take");
        let file = folder.join("answers");
        fs::write(
            &file,
            "person Ada Lovelace\naccount ada\npassword two words \nkeyboard lt\n",
        )
        .unwrap();

        let answers = Answers::take(&file).unwrap();
        assert_eq!(answers.person.as_deref(), Some("Ada Lovelace"));
        assert_eq!(answers.account.as_deref(), Some("ada"));
        assert_eq!(answers.password.as_deref(), Some("two words"));
        assert_eq!(answers.keyboard.as_deref(), Some("lt"));
        assert!(!file.exists());
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn an_answer_not_given_is_not_one() {
        let answers = Answers::parse(
            "person \npassword\nkeyboard ../../etc\naccount Ada L\naccount ../x\nother thing\n",
        );
        assert!(answers.is_empty());
        assert_eq!(
            Answers::parse("keyboard de(nodeadkeys)\n")
                .keyboard
                .as_deref(),
            Some("de(nodeadkeys)")
        );
    }

    #[test]
    fn they_are_kept_where_the_system_looks_for_them() {
        let data = folder("keep");
        let answers = Answers {
            person: Some("Ada Lovelace".into()),
            account: Some("ada".into()),
            password: Some("two words".into()),
            keyboard: Some("lt".into()),
        };
        answers.keep(&data).unwrap();

        assert_eq!(
            fs::read_to_string(data.join("account/name")).unwrap(),
            "Ada Lovelace\n"
        );
        let kept = fs::read_to_string(data.join("account/password")).unwrap();
        assert!(saule_password::is(&kept, "two words"));
        assert!(!kept.contains("two words"));
        let mode = fs::metadata(data.join("account/password"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(
            fs::read_to_string(data.join("system/keyboard")).unwrap(),
            "lt\n"
        );
        assert_eq!(
            fs::read_to_string(data.join("account/user")).unwrap(),
            "ada\n"
        );

        // Given again, they replace what was there; not given, they leave it.
        Answers {
            person: Some("Grace Hopper".into()),
            ..Answers::default()
        }
        .keep(&data)
        .unwrap();
        assert_eq!(
            fs::read_to_string(data.join("account/name")).unwrap(),
            "Grace Hopper\n"
        );
        assert!(saule_password::is(
            &fs::read_to_string(data.join("account/password")).unwrap(),
            "two words"
        ));
        fs::remove_dir_all(data).unwrap();
    }
}

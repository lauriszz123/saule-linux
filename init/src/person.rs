//! The session's account, named after the person the machine is for.
//!
//! The image has one ordinary account, `display`, which the session runs
//! as. The installer asks who the machine is for and writes a short name
//! for them on the data disk (`/data/account/user`: `ada`), and from then
//! on that is what the account is called: the home is `/data/home/ada`,
//! and a terminal's prompt says `ada`.
//!
//! It is the same account under another name (the same user and group
//! numbers), renamed where accounts are listed, `/etc/passwd` and
//! `/etc/group`. Those are in memory, as the whole system is, so this is
//! done at every start, before anything that looks an account up. A home
//! kept under the old name is moved to the new one.
//!
//! A machine nobody was named for (started from the stick, or installed
//! before the installer asked) keeps `display`. One with a full name and
//! no short one gets the full name run together in small letters, as a
//! Mac makes one: "Ada Lovelace" is `adalovelace`.

use std::fs;
use std::path::Path;

use crate::log;
use crate::storage;

const PASSWD: &str = "/etc/passwd";
const GROUP: &str = "/etc/group";

/// The longest a short name is: what every tool that prints one expects.
const LONGEST: usize = 32;

/// Rename the account called `default` after the person, if the data disk
/// names one. The name the session's account has from now on.
pub fn adopt(default: &str) -> String {
    if !storage::mounted() {
        return default.to_string();
    }
    let folder = Path::new(storage::MOUNT).join("account");
    let read = |name: &str| {
        fs::read_to_string(folder.join(name))
            .ok()
            .and_then(|text| text.lines().next().map(|line| line.trim().to_string()))
            .filter(|line| !line.is_empty())
    };
    let wanted = read("user")
        .filter(|name| valid(name))
        .or_else(|| read("name").and_then(|full| short_from(&full)));
    let Some(wanted) = wanted else {
        return default.to_string();
    };
    if wanted == default {
        return wanted;
    }

    let (Ok(passwd), Ok(group)) = (fs::read_to_string(PASSWD), fs::read_to_string(GROUP)) else {
        return default.to_string();
    };
    let full = read("name").unwrap_or_else(|| wanted.clone());
    let Some(passwd) = renamed_in_passwd(&passwd, default, &wanted, &full) else {
        log::warn(&format!(
            "the account can't be called {wanted}: it stays {default}"
        ));
        return default.to_string();
    };
    let group = renamed_in_group(&group, default, &wanted);
    if write(PASSWD, &passwd).is_err() || write(GROUP, &group).is_err() {
        log::warn("could not rename the session's account");
        return default.to_string();
    }

    // Its home in memory, where caches go, and its home on the disk.
    let _ = fs::rename(
        Path::new("/home").join(default),
        Path::new("/home").join(&wanted),
    );
    let homes = Path::new(storage::MOUNT).join("home");
    let (old, new) = (homes.join(default), homes.join(&wanted));
    if old.is_dir() && !new.exists() {
        if let Err(error) = fs::rename(&old, &new) {
            log::warn(&format!(
                "could not move {} to {}: {error}",
                old.display(),
                new.display()
            ));
        }
    }

    log::info(&format!("this machine is {wanted}'s"));
    wanted
}

/// Written beside and renamed over: a reader sees the old list or the new.
fn write(path: &str, text: &str) -> std::io::Result<()> {
    let part = format!("{path}.part");
    fs::write(&part, text)?;
    fs::rename(&part, path)
}

/// Whether `name` can be an account's: a small letter, then small
/// letters, digits, hyphens and underscores.
pub fn valid(name: &str) -> bool {
    let mut chars = name.chars();
    let starts = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    starts
        && name.len() <= LONGEST
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A short name made of a full one: its letters and digits, small, run
/// together, a letter with a mark on it as the letter without ("Šarūnas"
/// is `sarunas`). `None` when it has none that an account's name can
/// start with (a name written in another alphabet): the account keeps
/// its own.
pub fn short_from(full: &str) -> Option<String> {
    let short: String = full
        .chars()
        .flat_map(char::to_lowercase)
        .map(unmarked)
        .filter(|c| c.is_ascii_alphanumeric())
        .skip_while(|c| c.is_ascii_digit())
        .take(LONGEST)
        .collect();
    valid(&short).then_some(short)
}

/// A small Latin letter without its mark; any other character as it is.
/// The installer's window does the same as a name is typed
/// (../installer/src/model/Installation.sau), so what it shows is what
/// this makes.
fn unmarked(c: char) -> char {
    const LETTERS: [(&str, char); 18] = [
        ("àáâãäåāăą", 'a'),
        ("çćĉċč", 'c'),
        ("ďđ", 'd'),
        ("èéêëēĕėęě", 'e'),
        ("ĝğġģ", 'g'),
        ("ĥħ", 'h'),
        ("ìíîïĩīĭįı", 'i'),
        ("ĵ", 'j'),
        ("ķ", 'k'),
        ("ĺļľŀł", 'l'),
        ("ñńņň", 'n'),
        ("òóôõöøōŏő", 'o'),
        ("ŕŗř", 'r'),
        ("śŝşšß", 's'),
        ("ţťŧ", 't'),
        ("ùúûüũūŭůűų", 'u'),
        ("ýÿŷ", 'y'),
        ("źżž", 'z'),
    ];
    LETTERS
        .iter()
        .find(|(marked, _)| marked.contains(c))
        .map_or(c, |(_, plain)| *plain)
}

/// `/etc/passwd` with the account `old` called `new`, described by the
/// person's full name, its home beside the others under the new name.
/// `None` when there is no `old`, or another account is already called
/// `new`.
fn renamed_in_passwd(passwd: &str, old: &str, new: &str, full: &str) -> Option<String> {
    let name_of = |line: &str| line.split(':').next().unwrap_or("").to_string();
    if passwd.lines().any(|line| name_of(line) == new) {
        return None;
    }
    let mut found = false;
    let lines: Vec<String> = passwd
        .lines()
        .map(|line| {
            let mut fields: Vec<String> = line.split(':').map(str::to_string).collect();
            if fields.len() < 7 || fields[0] != old {
                return line.to_string();
            }
            found = true;
            fields[0] = new.to_string();
            // A colon ends a field and a comma a part of this one.
            fields[4] = full.chars().filter(|c| !":,\n".contains(*c)).collect();
            if let Some(parent) = Path::new(&fields[5]).parent() {
                fields[5] = parent.join(new).to_string_lossy().into_owned();
            }
            fields.join(":")
        })
        .collect();
    found.then(|| lines.join("\n") + "\n")
}

/// `/etc/group` with `old` called `new`: where it is a member, and its
/// own group.
fn renamed_in_group(group: &str, old: &str, new: &str) -> String {
    let lines: Vec<String> = group
        .lines()
        .map(|line| {
            let mut fields: Vec<String> = line.split(':').map(str::to_string).collect();
            if fields.len() < 4 {
                return line.to_string();
            }
            if fields[0] == old {
                fields[0] = new.to_string();
            }
            let members: Vec<&str> = fields[3]
                .split(',')
                .map(|member| if member == old { new } else { member })
                .collect();
            fields[3] = members.join(",");
            fields.join(":")
        })
        .collect();
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/sh\n\
                          display:x:100:100:display server:/home/display:/usr/bin/sh\n";
    const GROUP: &str = "root:x:0:\nvideo:x:44:display\ninput:x:45:other,display\ndisplay:x:100:\n";

    #[test]
    fn the_account_takes_the_person_s_name_and_keeps_its_numbers() {
        assert_eq!(
            renamed_in_passwd(PASSWD, "display", "ada", "Ada: Lovelace, Countess").as_deref(),
            Some(
                "root:x:0:0:root:/root:/bin/sh\n\
                 ada:x:100:100:Ada Lovelace Countess:/home/ada:/usr/bin/sh\n"
            )
        );
        assert_eq!(
            renamed_in_group(GROUP, "display", "ada"),
            "root:x:0:\nvideo:x:44:ada\ninput:x:45:other,ada\nada:x:100:\n"
        );
    }

    #[test]
    fn a_name_another_account_has_is_not_taken() {
        assert_eq!(renamed_in_passwd(PASSWD, "display", "root", "Ada"), None);
        assert_eq!(renamed_in_passwd(PASSWD, "nobody", "ada", "Ada"), None);
    }

    #[test]
    fn a_short_name_is_small_letters_digits_and_a_few_marks() {
        assert!(valid("ada"));
        assert!(valid("ada-l_2"));
        assert!(!valid(""));
        assert!(!valid("Ada"));
        assert!(!valid("2ada"));
        assert!(!valid("ada lovelace"));
        assert!(!valid("../etc"));
        assert!(!valid(&"a".repeat(33)));
    }

    #[test]
    fn a_full_name_run_together_is_a_short_one() {
        assert_eq!(short_from("Ada Lovelace").as_deref(), Some("adalovelace"));
        assert_eq!(
            short_from("  Jean-Luc O'Neil 3rd ").as_deref(),
            Some("jeanluconeil3rd")
        );
        assert_eq!(short_from("42 Ada").as_deref(), Some("ada"));
        // Letters with marks are the letters; a name with nothing an
        // account's can be made of is none.
        assert_eq!(
            short_from("Ąžuolas Šarūnas").as_deref(),
            Some("azuolassarunas")
        );
        assert_eq!(short_from("Zoë Müller").as_deref(), Some("zoemuller"));
        assert_eq!(short_from("李雷"), None);
        assert_eq!(short_from(""), None);
    }
}

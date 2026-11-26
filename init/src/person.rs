//! The person's account: who the machine is for, and what apps run as.
//!
//! The image has two ordinary accounts. `display` is the session's: the
//! window manager, in the groups the screen and the keyboard belong to.
//! `user` is the person's: their files are its, and every app runs as it
//! (`launcher.rs`), in none of those groups.
//!
//! The installer asks who the machine is for and writes a short name for
//! them on the data disk (`/data/account/user`: `ada`), and from then on
//! that is what the person's account is called: the home is
//! `/data/home/ada`, and a terminal's prompt says `ada`.
//!
//! It is the same account under another name (the same user and group
//! numbers), renamed where accounts are listed, `/etc/passwd` and
//! `/etc/group`. Those are in memory, as the whole system is, so this is
//! done at every start, before anything that looks an account up.
//!
//! A machine nobody was named for (started from the stick, or installed
//! before the installer asked) keeps `user`. One with a full name and no
//! short one gets the full name run together in small letters, as a Mac
//! makes one: "Ada Lovelace" is `adalovelace`.
//!
//! Before apps had an account of their own, the session's was the
//! person's too, and one home held the person's files and the window
//! manager's settings alike. [`settle`] takes such a home apart, once.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::account::{self, Account};
use crate::log;
use crate::storage;

/// What the person's account is called until it is named after them,
/// and its number, which it keeps whatever it is called.
pub const ACCOUNT: &str = "user";
pub const UID: libc::uid_t = 1000;

/// What the window manager keeps in its home: its settings, the one thing
/// of a shared home that is the session's and not the person's.
const SESSIONS_OWN: &str = ".config/saule/wm.conf";

const PASSWD: &str = "/etc/passwd";
const GROUP: &str = "/etc/group";

/// The longest a short name is: what every tool that prints one expects.
const LONGEST: usize = 32;

/// Rename the account called `default` after the person, if the data disk
/// names one. The name the person's account has from now on.
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
        log::warn("could not rename the person's account");
        return default.to_string();
    }

    // Its home in memory, where caches go, and its home on the disk,
    // when it has one under the name it had.
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

/// Give the person a home of their own on the data disk, where the
/// session's account and theirs shared one: a machine installed before
/// apps ran as the person. Done once, as that system first starts under
/// this init; a machine set up since has nothing to take apart.
///
/// The shared home was the session's account's, under the person's name
/// or, where nobody was named, its own. It becomes the person's, every
/// file in it, but for the window manager's settings, which go to the
/// session's home.
pub fn settle(person: &Account, session: &Account) {
    if !storage::mounted() || person.uid == session.uid {
        return;
    }
    let homes = Path::new(storage::MOUNT).join("home");
    let moved = settle_in(&homes, &person.name, &session.name, session.uid, |home| {
        hand_over(home, person.uid, person.gid);
    });
    if moved {
        log::info(&format!(
            "{}'s files are in a home of their own now",
            person.name
        ));
    }
}

/// [`settle`], under `homes`: whether a shared home was taken apart.
/// `hand_over` makes everything in the person's home theirs.
fn settle_in(
    homes: &Path,
    person: &str,
    session: &str,
    session_uid: u32,
    hand_over: impl FnOnce(&Path),
) -> bool {
    let (own, shared) = (homes.join(person), homes.join(session));
    if !own.exists() {
        // Nobody was named: the shared home is under the session's name.
        // Else a new machine, whose homes have never been shared; made
        // now, so that this is only ever asked once.
        let made = if shared.is_dir() {
            fs::rename(&shared, &own)
        } else {
            fs::create_dir_all(&own)
        };
        if let Err(error) = made {
            log::warn(&format!("could not make {}: {error}", own.display()));
            return false;
        }
    }
    // The session's account's still: a home from before.
    if !fs::metadata(&own).is_ok_and(|home| home.uid() == session_uid) {
        return false;
    }
    let (kept, back) = (own.join(SESSIONS_OWN), shared.join(SESSIONS_OWN));
    if kept.is_file() && !back.exists() {
        if let Some(folder) = back.parent() {
            let _ = fs::create_dir_all(folder);
        }
        let _ = fs::rename(&kept, &back);
    }
    hand_over(&own);
    true
}

/// Make `path` and everything under it `uid`'s and `gid`'s. A link is
/// handed over itself, not what it points at, which may be anywhere.
fn hand_over(path: &Path, uid: u32, gid: u32) {
    let _ = std::os::unix::fs::lchown(path, Some(uid), Some(gid));
    let Ok(kind) = fs::symlink_metadata(path) else {
        return;
    };
    if kind.is_dir() {
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            hand_over(&entry.path(), uid, gid);
        }
    }
}

/// Where the person's files are, ready for an app to start in: their home
/// on the data disk when there is one, so what they keep outlives a
/// reboot; else their home in memory.
///
/// `session_gid` is the session's group. It may pass through the home, no
/// more, and write in `Pictures`: where the window manager, which is the
/// one program that can see the screen, puts a picture of it.
pub fn home_of(person: &Account, session_gid: Option<libc::gid_t>) -> PathBuf {
    let home = storage::home_for(&person.name, person.uid, person.gid).unwrap_or_else(|| {
        if person.home.is_dir() {
            person.home.clone()
        } else {
            PathBuf::from("/")
        }
    });
    if let Some(gid) = session_gid.filter(|_| home != Path::new("/")) {
        let pictures = home.join("Pictures");
        let _ = account::chown(&home, person.uid, gid);
        let _ = account::chmod(&home, 0o710);
        // Set-group: a picture made there is the group's, and stays where
        // the person, whose folder it is, can read and remove it.
        if fs::create_dir_all(&pictures).is_ok() {
            let _ = account::chown(&pictures, person.uid, gid);
            let _ = account::chmod(&pictures, 0o2770);
        }
    }
    home
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

    /// A folder of homes to try `settle_in` on, and this process's user,
    /// which owns whatever is made there.
    fn homes(name: &str) -> (PathBuf, u32) {
        let homes = std::env::temp_dir().join(format!("saule-homes-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&homes);
        fs::create_dir_all(&homes).unwrap();
        let uid = fs::metadata(&homes).unwrap().uid();
        (homes, uid)
    }

    fn put(path: PathBuf, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn a_home_shared_under_the_person_s_name_is_taken_apart() {
        let (homes, me) = homes("named");
        put(homes.join("ada/Documents/letter.txt"), "dear");
        put(homes.join("ada/.config/saule/wm.conf"), "wallpaper");
        put(homes.join("ada/.config/saule/editor.conf"), "font");

        let handed = std::cell::Cell::new(false);
        assert!(settle_in(&homes, "ada", "display", me, |home| {
            assert_eq!(home, homes.join("ada"));
            handed.set(true);
        }));
        assert!(handed.get());
        // The person's files stay; the window manager's settings go to
        // its own home.
        assert!(homes.join("ada/Documents/letter.txt").is_file());
        assert!(homes.join("ada/.config/saule/editor.conf").is_file());
        assert!(!homes.join("ada/.config/saule/wm.conf").exists());
        assert_eq!(
            fs::read_to_string(homes.join("display/.config/saule/wm.conf")).unwrap(),
            "wallpaper"
        );
        fs::remove_dir_all(homes).unwrap();
    }

    #[test]
    fn a_home_shared_under_the_session_s_name_becomes_the_person_s() {
        let (homes, me) = homes("unnamed");
        put(homes.join("display/Pictures/shot.png"), "png");
        put(homes.join("display/.config/saule/wm.conf"), "dock");

        assert!(settle_in(&homes, "user", "display", me, |_| {}));
        assert!(homes.join("user/Pictures/shot.png").is_file());
        assert!(!homes.join("display/Pictures").exists());
        assert_eq!(
            fs::read_to_string(homes.join("display/.config/saule/wm.conf")).unwrap(),
            "dock"
        );
        fs::remove_dir_all(homes).unwrap();
    }

    #[test]
    fn homes_that_were_never_shared_are_left_alone() {
        let (homes, me) = homes("new");
        let not_me = me + 1;
        // A new machine: the person's home is made, and nothing moved.
        assert!(!settle_in(&homes, "ada", "display", not_me, |_| panic!()));
        assert!(homes.join("ada").is_dir());
        // The window manager has kept its settings since; the person's
        // home is there, and theirs.
        put(homes.join("display/.config/saule/wm.conf"), "dock");
        put(homes.join("ada/notes.txt"), "mine");
        assert!(!settle_in(&homes, "ada", "display", not_me, |_| panic!()));
        assert!(homes.join("display/.config/saule/wm.conf").is_file());
        assert!(homes.join("ada/notes.txt").is_file());
        fs::remove_dir_all(homes).unwrap();
    }

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

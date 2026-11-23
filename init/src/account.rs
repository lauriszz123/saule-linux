//! The accounts programs run as, from `/etc/passwd` and `/etc/group`.
//!
//! Init is the only process that stays root. The session (the display
//! server) runs as its own system account, `display`, the way macOS runs
//! its WindowServer as `_windowserver`: it can reach the screen and the input
//! devices through its groups, and nothing else root could.

use std::path::{Path, PathBuf};

/// Where each account's private runtime directory lives (`XDG_RUNTIME_DIR`).
/// Saule's services (IPC sockets) are created in it.
const RUNTIME_ROOT: &str = "/run/user";

#[derive(Clone, Debug)]
pub struct Account {
    pub name: String,
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
    /// Supplementary groups: every group in `/etc/group` that lists the account.
    pub groups: Vec<libc::gid_t>,
    pub home: PathBuf,
}

impl Account {
    pub fn is_root(&self) -> bool {
        self.uid == 0
    }

    /// Look `name` up in `/etc/passwd` and `/etc/group`.
    pub fn find(name: &str) -> Option<Account> {
        let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
        let line = passwd
            .lines()
            .map(|line| line.split(':').collect::<Vec<_>>())
            .find(|fields| fields.len() >= 6 && fields[0] == name)?;

        let mut account = Account {
            name: name.into(),
            uid: line[2].parse().ok()?,
            gid: line[3].parse().ok()?,
            groups: Vec::new(),
            home: PathBuf::from(line[5]),
        };

        let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
        for fields in group
            .lines()
            .map(|line| line.split(':').collect::<Vec<_>>())
        {
            if fields.len() < 4 || !fields[3].split(',').any(|member| member == name) {
                continue;
            }
            if let Ok(gid) = fields[2].parse() {
                account.groups.push(gid);
            }
        }

        Some(account)
    }

    /// `XDG_RUNTIME_DIR` for this account.
    pub fn runtime_dir(&self) -> PathBuf {
        Path::new(RUNTIME_ROOT).join(self.uid.to_string())
    }

    /// Create the account's home and runtime directories, mode 0700 and
    /// owned by it. Run before every start: the image lives in RAM, so they
    /// are gone after a reboot.
    pub fn prepare(&self) -> std::io::Result<()> {
        for dir in [&self.home, &self.runtime_dir()] {
            std::fs::create_dir_all(dir)?;
            chown(dir, self.uid, self.gid)?;
            chmod(dir, 0o700)?;
        }
        Ok(())
    }
}

/// The gid of group `name`, from `/etc/group`.
pub fn group_id(name: &str) -> Option<libc::gid_t> {
    let group = std::fs::read_to_string("/etc/group").ok()?;
    group
        .lines()
        .map(|line| line.split(':').collect::<Vec<_>>())
        .find(|fields| fields.len() >= 3 && fields[0] == name)?
        .get(2)?
        .parse()
        .ok()
}

pub fn chown(path: &Path, uid: libc::uid_t, gid: libc::gid_t) -> std::io::Result<()> {
    std::os::unix::fs::chown(path, Some(uid), Some(gid))
}

pub fn chmod(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Everything the child needs to become `account`, prepared before `fork`:
/// between `fork` and `exec` only async-signal-safe calls are allowed, so no
/// allocation happens there.
pub struct Switch {
    uid: libc::uid_t,
    gid: libc::gid_t,
    groups: Vec<libc::gid_t>,
}

impl Switch {
    pub fn new(account: &Account) -> Switch {
        Switch {
            uid: account.uid,
            gid: account.gid,
            groups: account.groups.clone(),
        }
    }

    /// Become the account for good, in the forked child: groups first, then
    /// the group, then the user (after which none of it can be changed
    /// back). `no_new_privs` keeps it that way across `exec`: no set-uid
    /// program can hand root back.
    pub fn apply(&self) -> std::io::Result<()> {
        // SAFETY: plain syscalls; `groups` lives for the call.
        unsafe {
            if libc::setgroups(self.groups.len(), self.groups.as_ptr()) != 0
                || libc::setgid(self.gid) != 0
                || libc::setuid(self.uid) != 0
                || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_dir_is_per_uid() {
        let account = Account {
            name: "display".into(),
            uid: 100,
            gid: 100,
            groups: vec![],
            home: "/home/display".into(),
        };
        assert_eq!(account.runtime_dir(), Path::new("/run/user/100"));
    }
}

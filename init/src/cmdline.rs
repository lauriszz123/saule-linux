//! Options from the kernel command line (`/proc/cmdline`).

/// What init does when the session returns on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnExit {
    PowerOff,
    Reboot,
    Restart,
    Shell,
}

pub struct Cmdline {
    /// The executable started as the session.
    pub session: String,
    /// The account the session runs as (`/etc/passwd`).
    pub user: String,
    pub on_exit: OnExit,
    /// Variables for the session's environment, from `saule.env.NAME=value`:
    /// how to switch on a program's own diagnostics on a machine with no
    /// shell (`saule.env.SHINE_TRACE_FRAMES=1` puts the window manager's
    /// frame times on the console). The session otherwise starts with a
    /// clean environment.
    pub env: Vec<(String, String)>,
    /// Started from the stick Saule Linux is installed from
    /// (`saule.installer=1`, which the stick's GRUB passes): the session is
    /// the installer, as root since it writes to disks, and no disk is
    /// mounted or made into a data disk: the installer decides what
    /// happens to them. `saule.session` and `saule.user` still override.
    pub installer: bool,
}

/// The installer: `installer/` built with `saule build`.
pub const INSTALLER: &str = "/bin/installer";
/// What starts when no option says, and when the installer is left to try
/// the system first.
pub const SESSION: &str = "/bin/session";
pub const USER: &str = "display";

impl Cmdline {
    pub fn read() -> Cmdline {
        let text = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
        Cmdline::parse(&text)
    }

    fn parse(text: &str) -> Cmdline {
        let mut session = None;
        let mut user = None;
        let mut cmdline = Cmdline {
            session: SESSION.into(),
            user: USER.into(),
            on_exit: if crate::session::has_shell() {
                OnExit::Shell
            } else {
                OnExit::PowerOff
            },
            env: Vec::new(),
            installer: false,
        };

        for word in text.split_whitespace() {
            let Some((key, value)) = word.split_once('=') else {
                continue;
            };

            match key {
                "saule.session" => session = Some(value.to_string()),
                "saule.user" => user = Some(value.to_string()),
                "saule.installer" => cmdline.installer = value == "1",
                "saule.on_exit" => match value {
                    "poweroff" => cmdline.on_exit = OnExit::PowerOff,
                    "reboot" => cmdline.on_exit = OnExit::Reboot,
                    "restart" => cmdline.on_exit = OnExit::Restart,
                    "shell" => cmdline.on_exit = OnExit::Shell,
                    other => crate::log::warn(&format!("unknown saule.on_exit={other}, ignored")),
                },
                _ => {
                    let name = key.strip_prefix("saule.env.").unwrap_or("");
                    if !name.is_empty() {
                        cmdline.env.push((name.into(), value.into()));
                    }
                }
            }
        }

        if cmdline.installer {
            cmdline.session = INSTALLER.into();
            cmdline.user = "root".into();
        }
        if let Some(session) = session {
            cmdline.session = session;
        }
        if let Some(user) = user {
            cmdline.user = user;
        }

        cmdline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_without_any_option() {
        let cmdline = Cmdline::parse("console=ttyS0 quiet");
        assert_eq!(cmdline.session, "/bin/session");
        assert_eq!(cmdline.user, "display");
        assert!(cmdline.env.is_empty());
    }

    #[test]
    fn the_session_and_its_account() {
        let cmdline = Cmdline::parse("saule.session=/bin/apps/bench saule.user=root");
        assert_eq!(cmdline.session, "/bin/apps/bench");
        assert_eq!(cmdline.user, "root");
    }

    #[test]
    fn the_stick_starts_the_installer_as_root() {
        let cmdline = Cmdline::parse("quiet saule.installer=1");
        assert!(cmdline.installer);
        assert_eq!(cmdline.session, "/bin/installer");
        assert_eq!(cmdline.user, "root");

        assert!(!Cmdline::parse("quiet").installer);
        assert!(!Cmdline::parse("saule.installer=0").installer);
    }

    #[test]
    fn the_installer_s_session_can_still_be_named() {
        let cmdline = Cmdline::parse("saule.session=/bin/apps/bench saule.installer=1");
        assert!(cmdline.installer);
        assert_eq!(cmdline.session, "/bin/apps/bench");
        assert_eq!(cmdline.user, "root");
    }

    #[test]
    fn every_on_exit() {
        for (word, expected) in [
            ("poweroff", OnExit::PowerOff),
            ("reboot", OnExit::Reboot),
            ("restart", OnExit::Restart),
            ("shell", OnExit::Shell),
        ] {
            let cmdline = Cmdline::parse(&format!("saule.on_exit={word}"));
            assert_eq!(cmdline.on_exit, expected);
        }
    }

    #[test]
    fn an_unknown_on_exit_keeps_the_one_before() {
        let cmdline = Cmdline::parse("saule.on_exit=restart saule.on_exit=explode");
        assert_eq!(cmdline.on_exit, OnExit::Restart);
    }

    #[test]
    fn environment_for_the_session() {
        let cmdline = Cmdline::parse("saule.env.SAULE_LOG=wm=debug saule.env.A=1 saule.env.=x");
        assert_eq!(
            cmdline.env,
            [
                ("SAULE_LOG".to_string(), "wm=debug".to_string()),
                ("A".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn other_options_are_the_kernel_s() {
        let cmdline = Cmdline::parse("video=Virtual-1:1024x600 saule saule.unknown=1");
        assert_eq!(cmdline.session, "/bin/session");
        assert!(cmdline.env.is_empty());
    }
}

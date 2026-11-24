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
}

impl Cmdline {
    pub fn read() -> Cmdline {
        let text = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
        Cmdline::parse(&text)
    }

    fn parse(text: &str) -> Cmdline {
        let mut cmdline = Cmdline {
            session: "/bin/session".into(),
            user: "display".into(),
            on_exit: if crate::session::has_shell() {
                OnExit::Shell
            } else {
                OnExit::PowerOff
            },
            env: Vec::new(),
        };

        for word in text.split_whitespace() {
            let Some((key, value)) = word.split_once('=') else {
                continue;
            };

            match key {
                "saule.session" => cmdline.session = value.into(),
                "saule.user" => cmdline.user = value.into(),
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

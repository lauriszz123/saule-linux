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

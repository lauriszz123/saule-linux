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
    pub on_exit: OnExit,
}

impl Cmdline {
    pub fn read() -> Cmdline {
        let text = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
        Cmdline::parse(&text)
    }

    fn parse(text: &str) -> Cmdline {
        let mut cmdline = Cmdline {
            session: "/bin/session".into(),
            on_exit: if crate::session::has_shell() {
                OnExit::Shell
            } else {
                OnExit::PowerOff
            },
        };

        for word in text.split_whitespace() {
            let Some((key, value)) = word.split_once('=') else {
                continue;
            };

            match key {
                "saule.session" => cmdline.session = value.into(),
                "saule.on_exit" => match value {
                    "poweroff" => cmdline.on_exit = OnExit::PowerOff,
                    "reboot" => cmdline.on_exit = OnExit::Reboot,
                    "restart" => cmdline.on_exit = OnExit::Restart,
                    "shell" => cmdline.on_exit = OnExit::Shell,
                    other => crate::log::warn(&format!("unknown saule.on_exit={other}, ignored")),
                },
                _ => {}
            }
        }

        cmdline
    }
}

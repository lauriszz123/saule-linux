//! Updating the system from a file.
//!
//! An update file is found by Settings (on a drive that is plugged in,
//! or in Downloads) and applied here, as root: `saule-install update`
//! checks that it is signed by the system's maker and writes it over the
//! system's own partition (../install/src/update.rs). The new system is
//! the one the machine starts next.
//!
//! How it stands, in `update`:
//!
//!   version 2026.11.25.1822        the system running now
//!   state idle | installing | done VERSION | failed WHY
//!
//! The request: `update PATH`. Only a file where update files are looked
//! for is taken: on a drive, or in somebody's home.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::files;

const TOOL: &str = "/sbin/saule-install";
const RELEASE: &str = "/etc/saule-release";

#[derive(Clone)]
pub struct Update(Arc<Mutex<String>>);

pub fn start() -> Update {
    let update = Update(Arc::new(Mutex::new("idle".to_string())));
    update.say();
    update
}

/// The version of the system running: the first line of its release
/// file, or "unknown".
fn version() -> String {
    fs::read_to_string(RELEASE)
        .ok()
        .and_then(|text| text.lines().next().map(|line| line.trim().to_string()))
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Whether `path` is somewhere update files are looked for, and names a
/// file with nothing odd in the way to it.
fn acceptable(path: &str) -> bool {
    let places = ["/run/media/", "/data/home/", "/home/"];
    places.iter().any(|place| path.starts_with(place))
        && !path.split('/').any(|part| part == "..")
        && !path.contains('\n')
}

impl Update {
    fn say(&self) {
        let state = self.0.lock().map(|s| s.clone()).unwrap_or_default();
        files::say(
            "update",
            &[format!("version {}", version()), format!("state {state}")],
        );
    }

    fn set(&self, state: String) {
        if let Ok(mut now) = self.0.lock() {
            *now = state;
        }
        self.say();
    }

    pub fn ask(&self, rest: &str) {
        let path = rest.trim().to_string();
        let busy = self.0.lock().is_ok_and(|state| *state == "installing");
        if busy {
            return;
        }
        if !acceptable(&path) || !Path::new(&path).is_file() {
            self.set("failed That is not an update file this system can take.".to_string());
            return;
        }
        if !Path::new(TOOL).exists() {
            self.set("failed This system has no updater.".to_string());
            return;
        }
        self.set("installing".to_string());

        let this = self.clone();
        std::thread::spawn(move || {
            let output = Command::new(TOOL)
                .args(["update", &path])
                .stdin(Stdio::null())
                .output();
            let state = match output {
                Ok(output) if output.status.success() => {
                    let said = String::from_utf8_lossy(&output.stdout);
                    let version = said
                        .lines()
                        .find_map(|line| line.strip_prefix("done "))
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    format!("done {version}")
                }
                Ok(output) => {
                    let why = String::from_utf8_lossy(&output.stderr);
                    let why = why.lines().last().unwrap_or("").trim().to_string();
                    if why.is_empty() {
                        "failed The update could not be installed.".to_string()
                    } else {
                        format!("failed {why}")
                    }
                }
                Err(_) => "failed The updater could not be run.".to_string(),
            };
            this.set(state);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_is_taken_only_from_where_they_are_looked_for() {
        assert!(acceptable("/run/media/PHOTOS/saule-linux.update"));
        assert!(acceptable("/data/home/ada/Downloads/x.update"));
        assert!(!acceptable("/etc/passwd"));
        assert!(!acceptable("/run/media/../../etc/passwd"));
        assert!(!acceptable("relative.update"));
        assert!(!acceptable("/run/media/a\nb"));
    }
}

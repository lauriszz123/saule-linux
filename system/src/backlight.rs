//! The screen's brightness.
//!
//! A laptop's backlight is a folder in `/sys/class/backlight` with the
//! brightness it has and the most it can have; setting it is writing a
//! number, which only root may. Where the firmware, the laptop's own
//! driver and the graphics driver each offer one, the kernel's advice is
//! to take them in that order.
//!
//! How it stands, in `display`:
//!
//!   brightness 0..100|none     none: no backlight to set (a desktop's
//!                              monitor, a virtual machine)
//!
//! The request: `brightness 0..100`. Nothing asked for is dark: the
//! lowest is the dimmest the screen can still be read at.

use std::fs;
use std::path::{Path, PathBuf};

use crate::files;

const FOLDER: &str = "/sys/class/backlight";

pub struct Backlight {
    folder: Option<PathBuf>,
}

pub fn start() -> Backlight {
    let backlight = Backlight {
        folder: find(Path::new(FOLDER)),
    };
    backlight.say();
    backlight
}

impl Backlight {
    pub fn ask(&self, rest: &str) {
        let (Some(folder), Ok(percent)) = (&self.folder, rest.trim().parse::<u32>()) else {
            return;
        };
        if let Some(most) = number(&folder.join("max_brightness")) {
            let _ = fs::write(folder.join("brightness"), raw(percent, most).to_string());
        }
        self.say();
    }

    fn say(&self) {
        let now = self.folder.as_ref().and_then(|folder| {
            let most = number(&folder.join("max_brightness"))?;
            let is = number(&folder.join("brightness"))?;
            Some(percent(is, most))
        });
        let line = match now {
            Some(percent) => format!("brightness {percent}"),
            None => "brightness none".to_string(),
        };
        files::say("display", &[line]);
    }
}

fn number(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The backlight to set, of those in `folder`: the firmware's before the
/// laptop driver's before the graphics driver's.
fn find(folder: &Path) -> Option<PathBuf> {
    let rank = |kind: &str| match kind {
        "firmware" => 0,
        "platform" => 1,
        _ => 2,
    };
    let mut found: Vec<(u8, PathBuf)> = fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| number(&path.join("max_brightness")).is_some_and(|most| most > 0))
        .map(|path| {
            let kind = fs::read_to_string(path.join("type")).unwrap_or_default();
            (rank(kind.trim()), path)
        })
        .collect();
    found.sort();
    found.into_iter().next().map(|(_, path)| path)
}

/// What to write for `percent` of a backlight whose most is `most`: never
/// nothing, which on many laptops is the light off.
fn raw(percent: u32, most: u32) -> u32 {
    let asked = (u64::from(percent.min(100)) * u64::from(most) + 50) / 100;
    let least = (most / 100).max(1);
    (asked as u32).clamp(least, most)
}

fn percent(is: u32, most: u32) -> u32 {
    ((u64::from(is.min(most)) * 100 + u64::from(most) / 2) / u64::from(most.max(1))) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_percentage_is_a_share_of_the_most_and_never_dark() {
        assert_eq!(raw(100, 937), 937);
        assert_eq!(raw(50, 937), 469);
        assert_eq!(raw(0, 937), 9);
        assert_eq!(raw(0, 7), 1);
        assert_eq!(raw(400, 7), 7);
        assert_eq!(percent(469, 937), 50);
        assert_eq!(percent(7, 7), 100);
        assert_eq!(percent(9000, 7), 100);
    }

    #[test]
    fn the_firmware_s_backlight_is_taken_before_the_graphics_driver_s() {
        let folder = std::env::temp_dir().join(format!("saule-backlight-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        for (name, kind) in [("intel_backlight", "raw"), ("acpi_video0", "firmware")] {
            fs::create_dir_all(folder.join(name)).unwrap();
            fs::write(folder.join(name).join("type"), format!("{kind}\n")).unwrap();
            fs::write(folder.join(name).join("max_brightness"), "100\n").unwrap();
        }
        assert_eq!(find(&folder), Some(folder.join("acpi_video0")));
        fs::remove_dir_all(folder.join("acpi_video0")).unwrap();
        assert_eq!(find(&folder), Some(folder.join("intel_backlight")));
        fs::remove_dir_all(&folder).unwrap();
        assert_eq!(find(&folder), None);
    }
}

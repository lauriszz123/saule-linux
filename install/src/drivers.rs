//! Which graphics driver pack this machine needs.
//!
//! The stick carries one pack per kind of chip (`scripts/build-drivers.sh`),
//! and a list of which chip is which (`config/drivers.list`): PCI vendor and
//! device ids, first match wins, `*` for any device of a vendor. The
//! installed system gets the one pack its own chip needs and no other.

use std::fs;

/// A display controller's PCI ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chip {
    pub vendor: u16,
    pub device: u16,
}

/// A display controller as it is found: where it is on the bus, and
/// whether it is the one the firmware put the screen on.
struct Found {
    primary: bool,
    address: String,
    chip: Chip,
}

/// The display controllers of this machine: PCI devices of class 03. The
/// one the screen is on comes first, then the rest in the order they are
/// on the bus: a laptop with a second chip beside its own (an NVIDIA
/// card that only renders, the screen being wired to the Intel one) is
/// asked about the chip that draws on the screen first.
pub fn chips() -> Vec<Chip> {
    let Ok(entries) = fs::read_dir("/sys/bus/pci/devices") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let read = |file: &str| fs::read_to_string(entry.path().join(file)).unwrap_or_default();
        if !read("class").trim().starts_with("0x03") {
            continue;
        }
        if let (Some(vendor), Some(device)) = (hex(&read("vendor")), hex(&read("device"))) {
            found.push(Found {
                primary: read("boot_vga").trim() == "1",
                address: entry.file_name().to_string_lossy().into_owned(),
                chip: Chip { vendor, device },
            });
        }
    }
    in_order(found)
}

fn in_order(mut found: Vec<Found>) -> Vec<Chip> {
    found.sort_by(|a, b| b.primary.cmp(&a.primary).then(a.address.cmp(&b.address)));
    found.into_iter().map(|found| found.chip).collect()
}

fn hex(text: &str) -> Option<u16> {
    u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// The pack to use: for the first of `chips` that `list` names one for
/// which there is (`there`: whether a pack of that name is on the
/// stick). A chip whose pack was never built is passed over for the next
/// chip, not taken as "no pack at all".
pub fn pack_for(list: &str, chips: &[Chip], there: impl Fn(&str) -> bool) -> Option<String> {
    chips
        .iter()
        .filter_map(|chip| pack_for_chip(list, *chip))
        .find(|pack| there(pack))
}

fn pack_for_chip(list: &str, chip: Chip) -> Option<String> {
    for line in list.lines() {
        let line = line.split('#').next().unwrap_or("");
        let mut words = line.split_whitespace();
        let (Some(ids), Some(pack)) = (words.next(), words.next()) else {
            continue;
        };
        let Some((vendor, device)) = ids.split_once(':') else {
            continue;
        };
        if hex(vendor) != Some(chip.vendor) {
            continue;
        }
        if device == "*" || hex(device) == Some(chip.device) {
            return Some(pack.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "\
# vendor:device   pack
8086:a011   gen3     # the netbook's GMA 3150
8086:*      intel
1af4:1050   virtual
";

    fn chip(vendor: u16, device: u16) -> Chip {
        Chip { vendor, device }
    }

    #[test]
    fn the_first_line_that_matches_wins() {
        assert_eq!(
            pack_for(LIST, &[chip(0x8086, 0xa011)], |_| true).as_deref(),
            Some("gen3")
        );
        assert_eq!(
            pack_for(LIST, &[chip(0x8086, 0x0166)], |_| true).as_deref(),
            Some("intel")
        );
        assert_eq!(
            pack_for(LIST, &[chip(0x1af4, 0x1050)], |_| true).as_deref(),
            Some("virtual")
        );
    }

    #[test]
    fn a_chip_nothing_names_has_no_pack() {
        assert_eq!(pack_for(LIST, &[chip(0x1234, 0x1111)], |_| true), None);
        assert_eq!(pack_for(LIST, &[chip(0x1af4, 0x1000)], |_| true), None);
        assert_eq!(pack_for(LIST, &[], |_| true), None);
    }

    #[test]
    fn the_first_chip_the_list_knows_decides() {
        let chips = [chip(0x1234, 0x1111), chip(0x8086, 0xa011)];
        assert_eq!(pack_for(LIST, &chips, |_| true).as_deref(), Some("gen3"));
    }

    /// A laptop with an NVIDIA card beside its Intel chip, and no pack
    /// for the first: the Intel chip's is taken, whichever the bus
    /// lists first, and not "none".
    #[test]
    fn a_chip_without_a_pack_does_not_hide_one_that_has() {
        const BOTH: &str = "8086:*   intel\n10de:*   nvidia\n";
        let built = |pack: &str| pack == "intel";
        let nvidia_first = [chip(0x10de, 0x139b), chip(0x8086, 0x191b)];
        assert_eq!(
            pack_for(BOTH, &nvidia_first, built).as_deref(),
            Some("intel")
        );
        assert_eq!(pack_for(BOTH, &nvidia_first, |_| false), None);
    }

    #[test]
    fn the_chip_the_screen_is_on_is_asked_about_first() {
        let found = vec![
            Found {
                primary: false,
                address: "0000:01:00.0".into(),
                chip: chip(0x10de, 0x139b),
            },
            Found {
                primary: true,
                address: "0000:00:02.0".into(),
                chip: chip(0x8086, 0x191b),
            },
            Found {
                primary: false,
                address: "0000:00:01.0".into(),
                chip: chip(0x1002, 0x1234),
            },
        ];
        assert_eq!(
            in_order(found),
            [
                chip(0x8086, 0x191b),
                chip(0x1002, 0x1234),
                chip(0x10de, 0x139b)
            ]
        );
    }

    #[test]
    fn ids_as_sysfs_writes_them() {
        assert_eq!(hex("0x8086\n"), Some(0x8086));
        assert_eq!(hex("a011"), Some(0xa011));
        assert_eq!(hex("*"), None);
    }
}

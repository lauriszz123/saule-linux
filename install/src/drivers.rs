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

/// The display controllers of this machine: PCI devices of class 03.
pub fn chips() -> Vec<Chip> {
    let Ok(entries) = fs::read_dir("/sys/bus/pci/devices") else {
        return Vec::new();
    };
    let mut chips = Vec::new();
    for entry in entries.flatten() {
        let read = |file: &str| fs::read_to_string(entry.path().join(file)).unwrap_or_default();
        if !read("class").trim().starts_with("0x03") {
            continue;
        }
        if let (Some(vendor), Some(device)) = (hex(&read("vendor")), hex(&read("device"))) {
            chips.push(Chip { vendor, device });
        }
    }
    chips
}

fn hex(text: &str) -> Option<u16> {
    u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// The pack `list` names for the first of `chips` it knows.
pub fn pack_for(list: &str, chips: &[Chip]) -> Option<String> {
    chips.iter().find_map(|chip| pack_for_chip(list, *chip))
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
            pack_for(LIST, &[chip(0x8086, 0xa011)]).as_deref(),
            Some("gen3")
        );
        assert_eq!(
            pack_for(LIST, &[chip(0x8086, 0x0166)]).as_deref(),
            Some("intel")
        );
        assert_eq!(
            pack_for(LIST, &[chip(0x1af4, 0x1050)]).as_deref(),
            Some("virtual")
        );
    }

    #[test]
    fn a_chip_nothing_names_has_no_pack() {
        assert_eq!(pack_for(LIST, &[chip(0x1234, 0x1111)]), None);
        assert_eq!(pack_for(LIST, &[chip(0x1af4, 0x1000)]), None);
        assert_eq!(pack_for(LIST, &[]), None);
    }

    #[test]
    fn the_first_chip_the_list_knows_decides() {
        let chips = [chip(0x1234, 0x1111), chip(0x8086, 0xa011)];
        assert_eq!(pack_for(LIST, &chips).as_deref(), Some("gen3"));
    }

    #[test]
    fn ids_as_sysfs_writes_them() {
        assert_eq!(hex("0x8086\n"), Some(0x8086));
        assert_eq!(hex("a011"), Some(0xa011));
        assert_eq!(hex("*"), None);
    }
}

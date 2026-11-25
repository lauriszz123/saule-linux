//! The volume.
//!
//! A sound card has a control device beside the ones sound is played
//! through (`/dev/snd/controlC0`), with a list of named dials and switches:
//! "Master Playback Volume", "Master Playback Switch", and so on, which
//! vary with the card. This finds the dial that sets how loud the card
//! plays, and its switch, and turns them when asked. It speaks to the
//! device with the kernel's own `ioctl`s: no ALSA library is in the image
//! for anything but playing.
//!
//! How it stands, in `sound`:
//!
//!   card 0|1       whether there is a card with a volume at all
//!   volume 0..100
//!   muted 0|1
//!
//! The volume chosen is kept on the data disk (`/data/system/sound`) and
//! put back when the machine starts: a card wakes up silent, or at
//! whatever its maker liked.

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::files;

/// Where the volume is kept across restarts.
const KEPT: &str = "/data/system/sound";

/// The volume a machine that has never been told plays at.
const FIRST_VOLUME: u8 = 60;

/// The dial that is the volume, in the order looked for: the one that
/// sets everything the card plays, then what stands in for it on cards
/// without one.
const VOLUMES: [&str; 5] = ["Master", "PCM", "Speaker", "Headphone", "Line Out"];

/// What plays behind the volume: turned all the way up and on, so that
/// the one dial is what decides.
const BEHIND: [&str; 6] = [
    "PCM",
    "Speaker",
    "Headphone",
    "Front",
    "Line Out",
    "Bass Speaker",
];

// ---------------------------------------------------------------------------
// The kernel's structures (<sound/asound.h>), as they are on 64-bit Linux
// ---------------------------------------------------------------------------

/// A dial's or a switch's name on the card.
#[repr(C)]
#[derive(Clone, Copy)]
struct ElemId {
    numid: u32,
    iface: i32,
    device: u32,
    subdevice: u32,
    name: [u8; 44],
    index: u32,
}

#[repr(C)]
struct ElemList {
    offset: u32,
    space: u32,
    used: u32,
    count: u32,
    pids: *mut ElemId,
    reserved: [u8; 50],
}

#[repr(C)]
struct ElemInfo {
    id: ElemId,
    kind: i32,
    access: u32,
    count: u32,
    owner: i32,
    /// For a dial: its least and greatest value and its step, as `long`s.
    value: [i64; 16],
    reserved: [u8; 64],
}

#[repr(C)]
struct ElemValue {
    id: ElemId,
    indirect: u32,
    /// One value a channel, as `long`s.
    value: [i64; 128],
    reserved: [u8; 128],
}

/// `_IOWR('U', nr, size)`. The C library's type for a request is a
/// different width on glibc and musl; the number is the same 32 bits.
const fn iowr(nr: u64, size: usize) -> libc::Ioctl {
    ((3u64 << 30) | ((size as u64) << 16) | (0x55 << 8) | nr) as libc::Ioctl
}

const ELEM_LIST: libc::Ioctl = iowr(0x10, std::mem::size_of::<ElemList>());
const ELEM_INFO: libc::Ioctl = iowr(0x11, std::mem::size_of::<ElemInfo>());
const ELEM_READ: libc::Ioctl = iowr(0x12, std::mem::size_of::<ElemValue>());
const ELEM_WRITE: libc::Ioctl = iowr(0x13, std::mem::size_of::<ElemValue>());

/// The kind of control a mixer's dials and switches are.
const IFACE_MIXER: i32 = 2;

// ---------------------------------------------------------------------------
// A card's mixer
// ---------------------------------------------------------------------------

/// A dial or a switch: its name on the card, and for a dial its range.
#[derive(Clone, Copy)]
struct Elem {
    id: ElemId,
    min: i64,
    max: i64,
    channels: usize,
}

struct Mixer {
    card: File,
    volume: Elem,
    /// The switch that goes with the volume, where the card has one.
    switch: Option<Elem>,
}

fn name_of(id: &ElemId) -> String {
    let end = id
        .name
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(id.name.len());
    String::from_utf8_lossy(&id.name[..end]).into_owned()
}

impl Mixer {
    /// The first card with a volume.
    fn open() -> Option<Mixer> {
        (0..8).find_map(|card| {
            let file = File::options()
                .read(true)
                .write(true)
                .open(format!("/dev/snd/controlC{card}"))
                .ok()?;
            Mixer::on(file)
        })
    }

    fn on(card: File) -> Option<Mixer> {
        let elems = elems(&card);
        let find = |name: String| {
            elems
                .iter()
                .find(|id| id.iface == IFACE_MIXER && name_of(id) == name)
                .and_then(|id| info(&card, *id))
        };

        let (volume, switch) = VOLUMES.iter().find_map(|dial| {
            let volume = find(format!("{dial} Playback Volume"))?;
            Some((volume, find(format!("{dial} Playback Switch"))))
        })?;

        // Everything behind the volume: up and on.
        for dial in BEHIND {
            for (suffix, full) in [("Volume", true), ("Switch", false)] {
                let Some(elem) = find(format!("{dial} Playback {suffix}")) else {
                    continue;
                };
                if name_of(&elem.id) == name_of(&volume.id) {
                    continue;
                }
                let value = if full { elem.max } else { 1 };
                write(&card, &elem, value);
            }
        }

        Some(Mixer {
            card,
            volume,
            switch,
        })
    }

    fn volume(&self) -> u8 {
        let raw = read(&self.card, &self.volume).unwrap_or(self.volume.min);
        percent_of(raw, self.volume.min, self.volume.max)
    }

    fn set_volume(&self, percent: u8) {
        let raw = raw_of(percent, self.volume.min, self.volume.max);
        write(&self.card, &self.volume, raw);
    }

    /// Whether the card's own switch says it is silent; `None` on a card
    /// without one, where silence is the volume at nothing.
    fn muted(&self) -> Option<bool> {
        let switch = self.switch.as_ref()?;
        Some(read(&self.card, switch).unwrap_or(1) == 0)
    }

    fn set_muted(&self, muted: bool) {
        if let Some(switch) = &self.switch {
            write(&self.card, switch, i64::from(!muted));
        }
    }
}

/// Every dial and switch the card has.
fn elems(card: &File) -> Vec<ElemId> {
    // SAFETY: zeroed is a valid value of these plain structures, and each
    // ioctl writes within the structure it is given (and, for the second,
    // within the `space` entries `pids` points to).
    unsafe {
        let mut list: ElemList = std::mem::zeroed();
        if libc::ioctl(card.as_raw_fd(), ELEM_LIST, &mut list) != 0 {
            return Vec::new();
        }
        let mut ids: Vec<ElemId> = vec![std::mem::zeroed(); list.count as usize];
        list.space = list.count;
        list.pids = ids.as_mut_ptr();
        if libc::ioctl(card.as_raw_fd(), ELEM_LIST, &mut list) != 0 {
            return Vec::new();
        }
        ids.truncate(list.used as usize);
        ids
    }
}

fn info(card: &File, id: ElemId) -> Option<Elem> {
    // SAFETY: as above.
    unsafe {
        let mut info: ElemInfo = std::mem::zeroed();
        info.id = id;
        if libc::ioctl(card.as_raw_fd(), ELEM_INFO, &mut info) != 0 {
            return None;
        }
        Some(Elem {
            id: info.id,
            min: info.value[0],
            max: info.value[1],
            channels: (info.count as usize).clamp(1, 128),
        })
    }
}

/// The first channel's value.
fn read(card: &File, elem: &Elem) -> Option<i64> {
    // SAFETY: as above.
    unsafe {
        let mut value: ElemValue = std::mem::zeroed();
        value.id = elem.id;
        if libc::ioctl(card.as_raw_fd(), ELEM_READ, &mut value) != 0 {
            return None;
        }
        Some(value.value[0])
    }
}

/// `to` on every channel.
fn write(card: &File, elem: &Elem, to: i64) {
    // SAFETY: as above.
    unsafe {
        let mut value: ElemValue = std::mem::zeroed();
        value.id = elem.id;
        for channel in value.value.iter_mut().take(elem.channels) {
            *channel = to;
        }
        libc::ioctl(card.as_raw_fd(), ELEM_WRITE, &mut value);
    }
}

fn percent_of(raw: i64, min: i64, max: i64) -> u8 {
    if max <= min {
        return 0;
    }
    (((raw - min) * 100 + (max - min) / 2) / (max - min)).clamp(0, 100) as u8
}

fn raw_of(percent: u8, min: i64, max: i64) -> i64 {
    let percent = i64::from(percent.min(100));
    min + ((max - min) * percent + 50) / 100
}

// ---------------------------------------------------------------------------
// What the session asks of it
// ---------------------------------------------------------------------------

struct Stands {
    mixer: Option<Mixer>,
    volume: u8,
    muted: bool,
}

#[derive(Clone)]
pub struct Sound(Arc<Mutex<Stands>>);

/// Find the card, put the kept volume back, and keep an eye on both: a
/// card may come later (USB), and its volume may be turned by other means.
pub fn start() -> Sound {
    let (volume, muted) = kept().unwrap_or((FIRST_VOLUME, false));
    let sound = Sound(Arc::new(Mutex::new(Stands {
        mixer: None,
        volume,
        muted,
    })));

    let watching = sound.clone();
    std::thread::spawn(move || loop {
        watching.look();
        std::thread::sleep(Duration::from_secs(3));
    });
    sound
}

impl Sound {
    /// `volume N` or `mute 0|1`.
    pub fn ask(&self, word: &str, rest: &str) {
        let Ok(mut stands) = self.0.lock() else {
            return;
        };
        match (word, rest.trim().parse::<u8>()) {
            ("volume", Ok(volume)) => {
                stands.volume = volume.min(100);
                // Turning it up is wanting to hear it.
                if stands.volume > 0 {
                    stands.muted = false;
                }
            }
            ("mute", Ok(muted)) => stands.muted = muted != 0,
            _ => return,
        }
        apply(&stands);
        keep(&stands);
        say(&stands);
    }

    /// Find a card if there is none yet, and take in what the card says
    /// now.
    fn look(&self) {
        let Ok(mut stands) = self.0.lock() else {
            return;
        };
        if stands.mixer.is_none() {
            stands.mixer = Mixer::open();
            // A card just found plays as was chosen, not as it woke.
            apply(&stands);
        } else if let Some(mixer) = &stands.mixer {
            let muted = mixer.muted();
            // With no switch, silence is the volume at nothing: what the
            // card says then is not the volume to go back to.
            if muted.is_some() || !stands.muted {
                stands.volume = mixer.volume();
            }
            if let Some(muted) = muted {
                stands.muted = muted;
            }
        }
        say(&stands);
    }
}

fn apply(stands: &Stands) {
    let Some(mixer) = &stands.mixer else {
        return;
    };
    if mixer.switch.is_some() {
        mixer.set_volume(stands.volume);
        mixer.set_muted(stands.muted);
    } else {
        mixer.set_volume(if stands.muted { 0 } else { stands.volume });
    }
}

fn say(stands: &Stands) {
    files::say(
        "sound",
        &lines(stands.mixer.is_some(), stands.volume, stands.muted),
    );
}

fn lines(card: bool, volume: u8, muted: bool) -> Vec<String> {
    vec![
        format!("card {}", u8::from(card)),
        format!("volume {volume}"),
        format!("muted {}", u8::from(muted)),
    ]
}

fn keep(stands: &Stands) {
    if std::path::Path::new("/data/system").is_dir() {
        let text = format!(
            "volume {}\nmuted {}\n",
            stands.volume,
            u8::from(stands.muted)
        );
        let _ = fs::write(KEPT, text);
    }
}

fn kept() -> Option<(u8, bool)> {
    parse_kept(&fs::read_to_string(KEPT).ok()?)
}

fn parse_kept(text: &str) -> Option<(u8, bool)> {
    let value = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.trim().parse::<u8>().ok())
    };
    Some((
        value("volume ")?.min(100),
        value("muted ").unwrap_or(0) != 0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_structures_are_the_kernel_s_size() {
        assert_eq!(std::mem::size_of::<ElemId>(), 64);
        assert_eq!(std::mem::size_of::<ElemList>(), 80);
        assert_eq!(std::mem::size_of::<ElemInfo>(), 272);
        assert_eq!(std::mem::size_of::<ElemValue>(), 1224);
        // And so the requests are the numbers <sound/asound.h> works out.
        assert_eq!(ELEM_LIST as u32, 0xc050_5510);
        assert_eq!(ELEM_INFO as u32, 0xc110_5511);
        assert_eq!(ELEM_READ as u32, 0xc4c8_5512);
        assert_eq!(ELEM_WRITE as u32, 0xc4c8_5513);
    }

    #[test]
    fn a_volume_is_a_share_of_the_dial_s_range() {
        // A Realtek master: 0 to 87.
        assert_eq!(raw_of(0, 0, 87), 0);
        assert_eq!(raw_of(100, 0, 87), 87);
        assert_eq!(raw_of(50, 0, 87), 44);
        assert_eq!(percent_of(44, 0, 87), 51);
        assert_eq!(percent_of(87, 0, 87), 100);
        // A range that doesn't start at nothing.
        assert_eq!(raw_of(50, -64, 0), -32);
        assert_eq!(percent_of(-32, -64, 0), 50);
        // No range at all.
        assert_eq!(percent_of(5, 3, 3), 0);
    }

    #[test]
    fn every_volume_survives_the_trip_to_the_dial_and_back() {
        // On a dial with at least as many steps as there are percents.
        for percent in 0..=100u8 {
            assert_eq!(percent_of(raw_of(percent, 0, 255), 0, 255), percent);
        }
    }

    #[test]
    fn what_was_kept_is_read_back() {
        assert_eq!(parse_kept("volume 35\nmuted 1\n"), Some((35, true)));
        assert_eq!(parse_kept("volume 250\n"), Some((100, false)));
        assert_eq!(parse_kept("muted 1\n"), None);
        assert_eq!(parse_kept(""), None);
    }

    #[test]
    fn how_it_stands_is_three_lines() {
        assert_eq!(lines(true, 60, false), ["card 1", "volume 60", "muted 0"]);
        assert_eq!(lines(false, 0, true), ["card 0", "volume 0", "muted 1"]);
    }

    #[test]
    fn a_name_ends_at_its_first_nothing() {
        let mut id: ElemId = unsafe { std::mem::zeroed() };
        id.name[..6].copy_from_slice(b"Master");
        assert_eq!(name_of(&id), "Master");
    }
}

//! saule-system — what the session may ask of the machine but not do
//! itself.
//!
//! The session runs as an ordinary account: it can't put the machine to
//! sleep, set the sound card's volume or bring up a network. This runs as
//! root beside it (init starts it and keeps it running) and does those
//! things when asked, and says how each of them stands, for the window
//! manager's bar to show.
//!
//! It is spoken to through files, in `/run/saule/system`:
//!
//!   requests/   a file dropped here is one request, a line of text; it is
//!               read, removed and acted on. Only the session's account
//!               can write here, so only the session can ask
//!   power       how each thing stands, a file each, rewritten as it
//!   sound       changes: a few `name value` lines, read by whoever wants
//!   network     them
//!   display
//!   login
//!   disks
//!   clock
//!   bluetooth
//!
//! Files rather than a socket because a Saule program can read and write
//! them with nothing but `Io`, and because how things stand is then there
//! to be looked at with `cat`.
//!
//! The requests:
//!
//!   sleep                    suspend to memory (`power.rs`)
//!   volume 0..100            the sound card's volume (`sound.rs`)
//!   mute 0|1
//!   input 0..100             the microphone's level; `input-mute 0|1`
//!   output N                 play through sound card N
//!   brightness 0..100        the screen's backlight (`backlight.rs`)
//!   eject DEVICE             let go of a drive that is plugged in
//!                            (`disks.rs`)
//!   clock sync               ask a time server the time now (`clock.rs`)
//!   bluetooth on|off|scan    the radio, and looking for devices
//!   bluetooth pair|connect|disconnect|forget ADDRESS   (`bluetooth.rs`)
//!   unlock PASSWORD          is this the person's password? (`login.rs`)
//!   password OLD<tab>NEW     change it
//!   person NAME              who the machine is for
//!   wifi on|off              the radio (`wifi.rs`)
//!   wifi scan                look for networks again
//!   wifi join SSID[<tab>PASSWORD]
//!   wifi forget SSID
//!
//! Usage: saule-system [ACCOUNT]    the session's account (default: display)

mod account;
mod backlight;
mod bluetooth;
mod clock;
mod disks;
mod files;
mod login;
mod network;
mod power;
mod requests;
mod sound;
mod wifi;

fn main() {
    let account = std::env::args().nth(1).unwrap_or_else(|| "display".into());
    let owner = account::ids(&account);
    if owner.is_none() {
        eprintln!("saule-system: no account `{account}`: nobody can ask anything");
    }

    if let Err(error) = files::prepare(owner) {
        eprintln!("saule-system: could not make {}: {error}", files::FOLDER);
        std::process::exit(1);
    }

    let sound = sound::start();
    let backlight = backlight::start();
    let disks = disks::start(owner);
    let clock = clock::start();
    let bluetooth = bluetooth::start();
    let mut login = login::start();
    let wifi = wifi::start();
    network::start(wifi.clone());
    power::watch_lid();

    requests::serve(|request| {
        let (word, rest) = request.split_once(' ').unwrap_or((request, ""));
        match word {
            "sleep" => power::sleep(),
            "volume" | "mute" | "input" | "input-mute" | "output" => sound.ask(word, rest),
            "brightness" => backlight.ask(rest),
            "eject" => disks.ask(word, rest),
            "clock" => clock.ask(rest),
            "bluetooth" => bluetooth.ask(rest),
            "unlock" | "password" | "person" => login.ask(word, rest),
            "wifi" => wifi.ask(rest),
            _ => eprintln!("saule-system: unknown request `{word}`"),
        }
    });
}

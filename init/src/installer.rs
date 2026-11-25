//! Started from the stick: what init does before the installer starts.
//!
//! An installed system gets the graphics driver pack its chip needs from
//! GRUB, unpacked over the initramfs before init runs. A stick can't know
//! what machine it will be put in, so it carries every pack and the
//! system's own tool picks one now (`saule-install live-drivers`, in
//! ../install). Without it the installer still runs: Shine draws on the
//! CPU.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::log;

const TOOL: &str = "/sbin/saule-install";

pub fn prepare() {
    if !Path::new(TOOL).exists() {
        return;
    }

    let output = Command::new(TOOL)
        .arg("live-drivers")
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let pack = String::from_utf8_lossy(&output.stdout);
            log::info(&format!("graphics driver pack: {}", pack.trim()));
        }
        Ok(output) => {
            let said = String::from_utf8_lossy(&output.stderr);
            log::warn(&format!("no graphics driver pack: {}", said.trim()));
        }
        Err(error) => log::warn(&format!("could not run {TOOL}: {error}")),
    }
}

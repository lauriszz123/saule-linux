# Saule Linux

A minimal Linux distribution that boots straight into a Saule program.

There's no systemd, no package manager, no X11 or Wayland: a Linux kernel,
a small init written in Rust, and one Saule project — the **session** —
built into a single executable with `saule build`. The session is the
display server and window manager, [saule-wm](https://github.com/lauriszz123/saule-wm),
written in Saule (see [ROADMAP.md](ROADMAP.md)). Without a `../saule-wm`
checkout, `session/` — a hello-world — is built instead.

```
$ make APP=session run
...
[init] no data disk: nothing is kept across a reboot
[init] starting session: /bin/session as display

  Hello from Saule on Linux 6.18.64-saule
  uptime  0.82 s
  memory  2051192 kB

[init] session ended
[init] powering off
```

In QEMU with KVM the kernel hands over to init in under a second. The
session then starts at once when there is a [data disk](#storage), and
after init has waited a second and a half for one when there isn't. The
kernel is 4.9 MB; the image with the window manager, its apps, the
wallpapers and a graphics driver pack is 32 MB.

## Quick start

Host packages (Ubuntu):

```sh
sudo apt install build-essential flex bison bc libelf-dev libssl-dev cpio \
                 qemu-system-x86 busybox-static
rustup target add x86_64-unknown-linux-musl
sudo usermod -aG kvm $USER     # optional: KVM makes QEMU fast (log in again)
```

Then:

| Command | What it does |
|---|---|
| `make` | download and build the kernel, build init and the session (`../saule-wm`, else `session/`), pack the initramfs |
| `make run` | boot in QEMU on this terminal (`Ctrl-A X` quits QEMU) |
| `make run-gui` | boot in a QEMU window: the screen the display server will draw on. On a Mac the window captures every shortcut, Cmd+Space included; `QEMU_DISPLAY=…` sets another `-display` |
| `make run-vnc` | the same, served over VNC on 127.0.0.1:5901 (password `saule`, `VNC_PASSWORD=…` to change it) for a viewer that scales it to its window; on a Mac it opens Screen Sharing, where the launcher is Cmd+D |
| `make run-netbook` | boot in QEMU dressed as the HP Mini 210-1001sa (N450, 1 GB, SeaBIOS, 1024×600, a 250 GB SATA hard disk, USB 2.0, PS/2, a USB tablet as the pointer), with KVM when available: `scripts/qemu-netbook.sh`. The disk is `disks/netbook.qcow2`, made empty on the first run and kept: see [Storage](#storage). On a Mac, in QEMU's own window at the netbook's size, one netbook pixel to a point (QEMU runs from a non-Retina copy of itself, `build/qemu-netbook.app`) |
| `make SLOW=1 run-netbook` | the same at about the netbook's speed: KVM capped at 8% of one core (an estimate; `SLOW=12` sets another percentage). Calibrate with `examples/bench` |
| `make GL=0 run-netbook` | the same with no 3D in its virtual GPU, so the system draws on its processor. Otherwise the GPU has 3D wherever this machine's QEMU can do it, and the window manager draws on this machine's GPU: on Linux and under WSL with QEMU's OpenGL modules (`qemu-system-modules-opengl` on Debian and Ubuntu); not with Homebrew's QEMU on a Mac, which is built without OpenGL. The script says which it is as it starts. Needs the `virtual` driver pack: `make drivers` builds it once (meson, ninja, python3-mako and python3-yaml on the host) |
| `make MEMORY=2G run-netbook` | the netbook with its memory upgraded |
| `make run-vesa` | the netbook with no graphics driver: the screen the BIOS set up, as a chip with no driver would show |
| `make DEBUG=1 run` | add BusyBox at `/bin`: a shell when the session ends |
| `make APP=examples/compositor-demo APPS="examples/shine-demo" run-netbook` | a session plus apps, each built into `/bin/apps/<name>`, named after its `saule.config` `name:`. Without `APPS=`, every app in [`../saule-linux-apps`](https://github.com/lauriszz123/saule-linux-apps), the ones that ship; none starts by itself, the launcher opens them |
| `make TOOLS=0 …` | leave out the command-line tools (BusyBox in `/usr/bin`, there by default for a terminal app's shell) |
| `make APPEND="saule.on_exit=restart" run` | pass kernel command line options |
| `make apps` | build only the apps, into `build/apps`: what the window manager opens when it runs nested on a desktop |
| `make usb-image` | `build/saule-linux.img`, a USB stick as a file: see [Installing](#installing). On a Mac it is made in the container and copied to `out/saule-linux.img` |
| `make usb DEVICE=/dev/sdX` | write that image to a stick (as root; the stick is erased). Not on a Mac, where it says how: balenaEtcher, or `dd` to the stick's `/dev/rdiskN` |
| `make run-usb` | the QEMU netbook started from the stick, into the installer. `DISK=file.qcow2` gives it a disk to practise on instead of `disks/netbook.qcow2` |
| `make run-disk` | the QEMU netbook started from its hard disk alone: what the installer put there |
| `make update` | a signed update file for systems already installed (see [Updating](#updating)) |
| `make test` | run the tests of init, the installer tool, the system service and the password crate |
| `make menuconfig` | change kernel options; copy the keepers into `config/kernel.config` |
| `make check-config` | list options `config/kernel.config` asked for but didn't get |
| `make clean` / `make distclean` | remove build output / everything including the kernel source. Neither touches `disks/` |

`SAULE=/path/to/saule` (a Saule with `saule build`), `APP=/path/to/project`
(the session), `WM=/path/to/saule-wm`, `BUSYBOX=…` and `QEMU=…` override what
gets used.

## How it boots

1. **The kernel** (`config/kernel.config` on top of `allnoconfig`) unpacks the
   initramfs into RAM and runs `/init`.
2. **`/init`** (`init/`, Rust, static musl binary) is PID 1:
   - mounts `/proc`, `/sys`, `/dev`, `/dev/pts` (pseudo-terminals, for a
     terminal app), `/run` and `/tmp`;
   - sets the hostname and prints `/etc/motd`;
   - mounts the data disk at `/data`, when there is one: see
     [Storage](#storage);
   - hands the screen and input devices to the `video` and `input` groups,
     including devices plugged in later;
   - runs `/bin/session` on the console as the **`display`** account (not
     root) with a clean environment (`HOME=/data/home/display` on the
     data disk, `/home/display` in memory without one;
     `XDG_RUNTIME_DIR=/run/user/100`). See [Security](#security);
   - with more than one console (`console=ttyS0 console=tty1`, as
     `make run-gui` boots), copies what the session and its apps print to
     all of them, so a session that covers the screen with its drawing still
     has its errors on the serial line. Init's own messages go to all of
     them too;
   - reaps orphaned processes;
   - when the session ends, it follows `saule.on_exit`. A crash restarts the
     session after a second; five quick crashes in a row give up.
   - `poweroff`/`reboot` (signals `SIGUSR2`/`SIGTERM`) and Ctrl-Alt-Del shut
     down cleanly.
3. **The session** (`session/`) is an ordinary Saule project. `make` builds
   it with `saule build` into one executable — bytecode, the Saule runtime,
   its native packages and bundled files — which becomes `/bin/session`.
   No Saule toolchain or source is in the image. Bundled files unpack to
   `~/.cache/saule/apps/` on first start.

Kernel command line options read by init:

| Option | Default | |
|---|---|---|
| `saule.session=PATH` | `/bin/session` | the executable to run as the session |
| `saule.user=NAME` | `display` | the account it runs as; `root` only when asked for |
| `saule.on_exit=…` | `shell` if `/bin/sh` exists, else `poweroff` | `poweroff`, `reboot`, `restart` or `shell` |
| `saule.env.NAME=value` | | adds `NAME=value` to the session's otherwise clean environment, e.g. `saule.env.SHINE_TRACE_FRAMES=1` for its frame times on the console |

How the session ends is a request init acts on, since the session runs as
`display` and may not power the machine off itself (the window manager's
Shut Down, Restart and Log Out use this):

| Exit code | |
|---|---|
| 0 | the session ended: `saule.on_exit` decides what next |
| 80 | power off |
| 81 | reboot |
| 82 | log out: start a new session at once |
| anything else | a crash: restart it after a second (five quick ones in a row give up) |

## Installing

`make usb-image` builds `build/saule-linux.img`: a USB stick, as one file.
Write it to a stick (`make usb DEVICE=/dev/sdX` as root; from Windows, Rufus
in its DD mode or balenaEtcher), and a PC starts from it: by its BIOS, as
the netbook does, or by UEFI where there is no BIOS, which is most PCs made
since about 2012. With UEFI, **Secure Boot has to be off** in the
firmware's settings: nobody has signed this GRUB.

The stick starts the **installer** (`installer/`, a Saule program):

1. **Welcome**: install, or *Try It First*, which starts Saule Linux as it
   runs from the stick and touches no disk.
2. **Choose the keyboard**: its layout, from every one XKB describes, with
   a search and a field to try the keys in. Asked first, because the next
   page has a password typed on it.
3. **Who is this computer for?**: a name, the short name the account
   goes by (made from the name as it is typed, and there to be changed),
   and a password to ask for when the computer starts. The password can
   be left out.
4. **Choose a disk**: every disk but the stick (SATA, NVMe, eMMC, USB),
   with its size and what is on it: empty, Windows, another Linux, both,
   Saule Linux. A system that no longer starts is still told for what it
   was: its partitions say whose they are after their file systems are
   gone, and a GPT damaged at the start of the disk is read from its copy
   at the end.
5. **Confirm**: said to fit the disk. An empty one is simply installed on.
   One with Windows on it says so, and asks to erase with a button Enter
   can't press. One with Saule Linux already offers to keep its files:
   the system is written again and `/data` is left alone.
6. **Installing**, then **Restart**, with the stick taken out first.

The answers go to `saule-install` in a file it removes as it reads it, and
from there onto the new data disk: the layout as `/data/system/keyboard`,
the name and the password in `/data/account` (see
[Logging in](#logging-in)). Installing again with the files kept replaces
only what was answered this time.

What it installs:

```
sector 0          GRUB's first stage, and the partition table (MBR)
sectors 1–2047    GRUB itself, for a BIOS
partition 1       SAULE-BOOT, ext2, 512 MB: the kernel, the initramfs, one
                  graphics driver pack, GRUB's configuration
partition 2       SAULE, ext4: /data, the rest of the disk
partition 3       GRUB for UEFI: FAT32, the last 34 MB
```

One disk starts either kind of machine, so a disk moved from an old PC to
a new one still starts.

The system still runs from memory once installed: GRUB loads the kernel and
the initramfs from the first partition, which is then never mounted, so no
power cut and no program can leave a machine that doesn't start. The second
partition is the [data disk](#storage).

Until it is given a name, a machine is called after what it is, as its
maker's firmware says: an Inspiron 7559 is `Inspiron-7559`
(`init/src/system.rs`), and that is the name the first start offers.

**After installing**, the first start is already typed in the layout
chosen, and asks three things, over the desktop
they are about to change (the window manager's first-start setup): a name
for the computer, the time zone, and the look. Each takes effect as it is
chosen, and is kept on the data disk: `/data/system/hostname` and
`timezone`, which init acts on as they change (`init/src/system.rs`). A PC
that had Windows on it keeps local time in its own clock where Linux
expects UTC; the installer notes that (`/data/system/clock`), and the time
comes out right once a zone is chosen.

**One driver pack per machine.** The stick carries every graphics driver
pack that was built (`make drivers DRIVERS=gen3`), each as an archive the
kernel unpacks over the initramfs. The installer looks at the machine's
graphics chip (`config/drivers.list`), copies the one pack it needs and
names it in GRUB's configuration; the stick itself unpacks the right one
before the installer starts, so the installer draws on the GPU too. A chip
with no pack is drawn for on the CPU, and a chip the kernel has no driver
for at all (it has Intel's, and a virtual machine's) on the screen as GRUB
left it: slower, and at the size the firmware chose, but there.

A machine with two graphics chips (a laptop's own and an NVIDIA or AMD
card beside it) is asked about the chip its screen is on first
(`boot_vga`), and a chip whose pack was not built is passed over for the
next: such a laptop runs on its Intel chip, and the card beside it, for
which there is no driver here, is left alone.

**When graphics misbehave.** GRUB waits a second showing nothing; Esc in
that second (or Shift held) brings up its menu, whose second entry starts
the system with the processor drawing everything (`SHINE_GPU=0`). And
whatever does not work, Mod+Shift+R (or Settings, under About) writes
**a report** to the USB drive plugged in: every PCI and USB device with
its ids and the driver that took it, what draws the screen, what the
kernel said, and what the session and its apps printed, which init keeps
in `/run/saule/session.log` (`system/src/report.rs`).

Three packs are built so far: `gen3` (the netbook's GMA 3150), `intel`
(every Intel chip from 2006's GMA X3000 to 2023's Meteor Lake) and `virtual`
(QEMU's). `make drivers DRIVERS=intel` builds one; the stick takes every
one that is built.

The writing is done by `saule-install` (`install/`, Rust), which the
installer runs and watches: `saule-install disks` lists what it shows, and
`saule-install install sda` does the rest. The same program makes the
stick's image on the build machine, so both disks are laid out by one piece
of code. GRUB's two files are built by `scripts/mkgrub.sh`.

In QEMU: `make run-usb` starts the netbook from a stick, and what it
installs on is the netbook's disk, `disks/netbook.qcow2` — or
`DISK=build/practice.qcow2`, made empty the first time. QEMU can't take
the stick out, so after installing, shut down and `make run-disk`. Both
take `EFI=1`: the same machine with UEFI firmware (OVMF) and no BIOS. To see
the installer's pages on a desktop without installing anything:
`cd installer && SAULE_INSTALL=dev/fake-install saule run .`

A stick's kernel options are `BOOT_OPTIONS` (default `quiet`), which the
installed system inherits.

## Updating

An installed system is updated from a file, `saule-linux-<version>.update`:
the kernel, the initramfs and the driver packs, as the stick carries them,
with a signature at its end. `make update` makes one (after `make
update-key`, once, has made the key it is signed with: the private half in
`keys/`, which is not in the repository and wants keeping safe, and the
public half in `config/update.pub`, which is built into `saule-install`).

Put on a USB drive, or in Downloads, the file is found by Settings
(General, Software Update), which shows the version running and the one
offered. Installing it asks `saule-system`, which runs `saule-install
update`: the file is checked against the key before anything is written,
the new files are written beside the old ones in the system's partition
and flushed, and then renamed over them, so a power cut leaves the old
system or the new. The computer starts the new one the next time it
starts. What is kept on the data disk is not touched, and the kernel
options the machine starts with stay as they are.

There is nowhere updates are downloaded from yet: the file is carried.

## Logging in

A computer installed with a password starts at a login screen, and asks
for the password again whenever its screen is unlocked: after Lock Screen,
and after sleep. One installed without starts straight to the desktop, and
a password can be set later in Settings.

There is one person to a computer. Who they are and what is kept of their
password are files in `/data/account`, which is root's:

- `name`, which anyone can read;
- `user`, the short name their account goes by. The image has one
  ordinary account, `display`; at each start init renames it to this
  (`init/src/person.rs`: the same user and group numbers under another
  name, in `/etc/passwd` and `/etc/group`), so the home is
  `/data/home/ada`, a terminal's prompt says `ada`, and no `display` is
  left on an installed system. A home kept under the old name is moved.
  Started from the stick, the installer runs as root and *Try It First*
  as `display`: neither is on the disk;
- `password`, which only root can. It holds no password but what
  `password/` makes of one: PBKDF2 with HMAC-SHA-256, salted, 60 000
  rounds, as a line that also says how it was made, so the rounds can be
  raised later without old passwords ceasing to be right.

The session can't read that file, so the lock screen can't check a
password itself. It asks `saule-system` (`unlock PASSWORD`), which answers
yes or no in `/run/saule/system/login`. A wrong password costs a wait
before the answer, twice as long for each one in a row, up to eight
seconds. Changing the password goes the same way and takes the old one.

What this keeps out is somebody at the keyboard. The disk is not
encrypted: taken out and read elsewhere, its files are there to read, and
its password file can be replaced.

## Sound, the screen, network and sleep

The session runs as an ordinary account, which can't put the machine to
sleep, turn the sound card's volume or bring up a network. `saule-system`
(`system/`, Rust) runs as root beside it for that; init starts it and keeps
it running. It is spoken to through files in `/run/saule/system`:

- a file dropped in `requests/` is one request, a line of text (`sleep`,
  `volume 40`, `mute 1`, `brightness 70`, `wifi join Home<tab>password`,
  `unlock PASSWORD`). Only the session's account can write there;
- `power`, `sound`, `display`, `network`, `login`, `disks`, `clock` and
  `bluetooth` say how each stands, a few lines each,
  rewritten as they change. `cat /run/saule/system/network` shows what
  the window manager's bar is showing.

What it does:

- **Sleep**: suspends to memory when asked and when the lid closes. The
  window manager locks the screen first, so what wakes is locked.
- **Sound**: finds the card's volume and its switch among the mixer's
  controls and turns them, with the kernel's own requests (no ALSA
  library but for playing). The volume chosen is kept on the data disk.
- **The microphone, the headphones and the cards**: the level and the
  switch of what the card records; whether headphones are in their
  socket, where the card says; and, with more than one card that has a
  volume (a USB headset beside the laptop's own), which one plays, told
  to the sound library in `/etc/asound.conf` for programs started after.
- **Drives that are plugged in** (`disks.rs`): a USB stick's or card's
  partitions are mounted under `/run/media`, by their label, FAT, exFAT,
  NTFS or ext, for the session's account to read and write; `eject sdb1`
  flushes and lets go. The file browser lists them.
- **The clock** (`clock.rs`): once the machine is on a network the time
  is asked of a time server (SNTP), the system's clock is set by it, and
  the PC's own clock with it, in UTC; a note in the settings tells init
  that the PC's clock is no longer in local time. Asked again every six
  hours.
- **Bluetooth** (`bluetooth.rs`): where the machine has a radio, BlueZ's
  daemon is started, with a D-Bus daemon for it that only root may use,
  and asked through `bluetoothctl`: the radio, a search, pairing (a
  keyboard's digits are passed on to be shown), connecting. What was
  paired is kept on the data disk. `scripts/fetch-bluetooth.sh` gathers
  BlueZ, D-Bus and Intel's radios' firmware; `BLUETOOTH=0` leaves them
  out. For mice and keyboards: sound over Bluetooth needs a sound server
  the system doesn't have. **None of it has met a radio**: QEMU has none.
- **The screen's brightness**: a laptop's backlight
  (`/sys/class/backlight`), the firmware's before the laptop driver's
  before the graphics driver's. Never to nothing: the lowest is the
  dimmest the screen can still be read at.
- **A cable**: an interface with one in it is brought up and asked an
  address for by DHCP (BusyBox's `udhcpc`, so `TOOLS=1`).
- **Wi-Fi**: runs `wpa_supplicant` on the machine's radio and talks to it
  over its control socket: the networks in range, joining one, the ones
  known (kept on the data disk). The radios' firmware files and
  `wpa_supplicant` itself are gathered by `scripts/fetch-wifi.sh`; `WIFI=0`
  leaves them out of the image. The kernel has Intel's radios, Atheros'
  (ath9k), Broadcom's BCM4313 and 43224/5 (brcmsmac) and Broadcom's older
  cards (b43: the HP Mini 210-1000's BCM4312). The older cards' firmware is
  in no repository, Broadcom never having allowed it to be passed on: it
  is cut out of Broadcom's own driver as the image is built
  (`b43-fwcutter`, as distributions' installers do), which makes that
  image one for its builder's machines and not for handing out. `B43=0`
  leaves it out.

**The keys with pictures on them.** Volume up, down and mute, brightness
up and down, sleep and the power button arrive as keys among the input
devices: from the keyboard itself, from ACPI's video device, or from a
maker's own driver (Dell's and HP's are in the kernel). Shine names them
(`volumeup`, `brightnessdown`, …) and the window manager acts on them and
shows the new level on the screen for a moment.

**Keyboard layouts.** The image carries XKB's descriptions
(`/usr/share/X11/xkb`, from the build machine's xkb-data) and
libxkbcommon to read them; Shine types by the layout chosen. `XKB=` builds
without them, and the system then types as a US keyboard and offers no
other.

The QEMU netbook has a network cable plugged in (`NET=0` takes its card
out), its sound is played through this machine's sound system (PipeWire or
PulseAudio on Linux, WSLg's under WSL, Core Audio on a Mac; `SOUND=0`
throws it away), and `make STICK=stick.img run-netbook` plugs in a USB
stick with that disk image as its contents. QEMU has no radio, no lid, no
battery and no backlight, so Wi-Fi, Bluetooth, the battery and brightness
are seen only on real hardware; and its graphics card comes back from sleep
with a blank screen (the real ones restore theirs).

## Storage

The system runs from memory: the initramfs is all of it, and what it writes
there is gone at power-off. What should last goes on the **data disk**,
which init (`init/src/storage.rs`) mounts at `/data`:

- an ext2/3/4 file system labelled `SAULE`, on any disk or partition, is
  the data disk: the [installer](#installing) makes one, and
  `mke2fs -L SAULE /dev/sdX1` makes one by hand;
- failing that, an internal disk that is **completely empty** — nothing in
  its first megabyte — gets one partition with such a file system: ext4,
  journaled;
- a disk with anything on it is never touched, and neither is one on a USB
  port. The netbook's disk may still hold Windows. Started from the stick,
  no disk is touched at all until the installer is told to.

With a data disk the session's home is `/data/home/display`, and it gets
`SAULE_DATA=/data`. The window manager keeps its settings in
`~/.config/saule/wm.conf`, so they survive a restart; Settings → Storage
shows the disks and what is on them. Without one everything works as
before and nothing is kept.

A machine can lose power at any moment, so the disk is treated that way:

- it is mounted synchronously: what is written is on the disk when the
  write returns, so a setting changed a second before the power went is
  kept. It holds little, written rarely, so the slower writes cost little;
- it is checked with `e2fsck` before it is mounted, and mended if need be;
- caches stay in memory (`XDG_CACHE_HOME=/home/display/.cache`). A cache
  is rebuilt when it is missing, but a damaged one on the disk would be
  trusted on every boot: a built Saule app unpacks its libraries into
  one, and a session that can't load them never starts;
- it is unmounted before power-off and reboot.

`mke2fs` and `e2fsck` are the build machine's own (e2fsprogs), copied into
the image with their libraries. An image built without them formats with
BusyBox's `mke2fs` instead (`TOOLS=1`): ext2, no journal, no check.

`make run-netbook` has the netbook's disk: `disks/netbook.qcow2`, 250 GB
as the guest sees it, as large as what was written to it on this machine.
Delete the file to start again with an empty disk;
`scripts/qemu-netbook.sh --no-disk` boots without one, `--disk FILE` with
another. `make run` and `make run-gui` have no disk.

## Layout

```
Makefile                 the whole build, and the `run` targets
config/
  kernel.config          every kernel option turned on, grouped and explained
  drivers.list           which graphics driver pack each graphics chip needs
init/                    PID 1 (Rust)
  src/main.rs            the boot sequence
  src/mounts.rs          /proc /sys /dev /run /tmp
  src/storage.rs         the data disk: find it, or make it, and mount /data
  src/installer.rs       started from the stick: the driver pack, unpacked
  src/services.rs        the programs kept running beside the session
  src/system.rs          the machine's name, time zone and clock, as chosen
  src/zone.rs            what a time zone is right now
  src/console.rs         the real terminal behind /dev/console (for Ctrl-C)
  src/session.rs         start the session, restart it, reap orphans
  src/output.rs          what the session prints, copied to every console
  src/account.rs         accounts from /etc/passwd, dropping root for the session
  src/person.rs          the session's account, named after the person
  src/seat.rs            screen and input devices to video/input, hotplug included
  src/power.rs           poweroff / reboot and the signals for them
  src/cmdline.rs         saule.* kernel options
  src/log.rs             init's own messages
install/                 saule-install (Rust): lays out a disk, puts GRUB on
                         it, installs; also makes the stick's image
  src/layout.rs          where things are on a disk
  src/grub.rs            GRUB's two files, written where the BIOS looks
  src/disks.rs           the machine's disks, and what is on each
  src/drivers.rs         which driver pack a graphics chip needs
  src/target.rs          installing onto this machine's disk
  src/update.rs          an update file: made, checked, applied
  src/answers.rs         what the installer was told, put on the data disk
  src/image.rs           a stick's image, made on the build machine
password/                how a password is kept (Rust): made by
                         saule-install, checked by saule-system
system/                  saule-system (Rust): what the session asks the
                         machine for
  src/requests.rs        requests, as files dropped in a folder
  src/power.rs           sleep, and the lid
  src/sound.rs           the volume, through the card's control device
  src/backlight.rs       the screen's brightness
  src/disks.rs           drives plugged in: mounted, and let go of
  src/clock.rs           the time, from a time server
  src/bluetooth.rs       BlueZ, started and asked
  src/update.rs          an update file installed when asked
  src/login.rs           the person's name; checking and changing the
                         password
  src/network.rs         a cable's interface, and how every one stands
  src/wifi.rs            wpa_supplicant, started and talked to
  dhcp                   what udhcpc runs when it is given an address
installer/               the installer's window, a Saule project (uikit)
  src/model/             the disks, the installation under way, the tool
  src/pages/             one view per step: welcome, keyboard, account,
                         disk, confirm, installing, done
  dev/fake-install       stands in for saule-install on a desktop
session/                 the fallback session: a hello-world Saule project,
                         built when there is no ../saule-wm
examples/                small sessions and apps for trying one thing
  bench/                 a fixed amount of work, timed: calibrates SLOW=
  compositor-demo/       a test display server for Shine's compositor
  shine-demo/            a smoke test for Shine on the bare screen
  gpu-load/              an app that draws a steady load, to see where
                         drawing runs (CPU or GPU)
  pty-check/             a shell behind a pseudo-terminal, as a terminal
                         app runs one
rootfs/                  copied into the image as is
  etc/                   hostname, motd, passwd, group, profile
scripts/
  mkinitramfs.sh         assemble the tree (+ shared libraries) and pack it
  mkoverlay.sh           pack what is unpacked over it at boot: a driver
                         pack, the installer
  stage.sh               what those two share
  fetch-wifi.sh          wpa_supplicant and the radios' firmware, gathered
  fetch-debs.sh          a package and what it needs that the build machine
                         lacks, unpacked, never installed: what the fetch
                         scripts and mkgrub.sh take from apt
  fetch-graphics-firmware.sh  what Intel's graphics chips load (their
                         display's power states), gathered
  mkgrub.sh              GRUB for a BIOS and for UEFI, as the files a disk
                         boots with
  patches/               changes made to Mesa, each saying why
  check-config.sh        catch kernel options that were silently dropped
  build-drivers.sh       build the graphics driver packs (Mesa, libdrm)
  qemu-netbook.sh        QEMU dressed as the HP Mini 210-1001sa
  docker.sh              the build in a container, for a Mac
docker/Dockerfile        that container
build/                   downloads, kernel source and output (not in git)
disks/                   the emulated netbook's hard disk (not in git, and
                         not removed by `make clean` or `make distclean`)
```

## Security

The plan follows macOS (see ROADMAP.md, "Security"). Today:

- **Only init is root.** The session runs as an ordinary account (the person's; `display` until somebody is named), the way macOS runs
  its WindowServer as `_windowserver`. It can use the screen and input
  devices, through the `video` and `input` groups, and its own
  home, but it can't write system files or read other accounts'.
- **No way back to root:** the session starts with `no_new_privs`, so
  even a set-uid program can't raise it.
- **It fails closed:** if the account is missing, the session doesn't start
  as root instead. `saule.user=root` has to be passed on purpose.
- The debug shell (`DEBUG=1`) is root, as a debugging tool.

- **The password is root's to read.** What is kept of it is in a file the
  session can't open; the session asks whether a password is right and is
  told yes or no (see [Logging in](#logging-in)).

Apps the window manager starts today run as `display` too. Separating them
is the next step on the roadmap.

## The two layers

The Rust layer is what a Saule program can't be: init, the kernel's
configuration, `saule-install`, native Saule packages and `saule build`.
Everything a person sees is the Saule layer: the session, the display
server and window manager, the apps, and the installer's window.

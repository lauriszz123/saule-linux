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
| `make GL=1 run-netbook` | the same with a 3D virtual GPU, so the window manager draws on the GPU. Needs the `virtual` driver pack: `make drivers` builds it once (meson, ninja, python3-mako and python3-yaml on the host) |
| `make MEMORY=2G run-netbook` | the netbook with its memory upgraded |
| `make run-vesa` | the netbook with no graphics driver: the screen the BIOS set up, as a chip with no driver would show |
| `make DEBUG=1 run` | add BusyBox at `/bin`: a shell when the session ends |
| `make APP=examples/compositor-demo APPS="examples/shine-demo" run-netbook` | a session plus apps, each built into `/bin/apps/<name>`, named after its `saule.config` `name:`. Without `APPS=`, every app in [`../saule-linux-apps`](https://github.com/lauriszz123/saule-linux-apps), the ones that ship; none starts by itself, the launcher opens them |
| `make TOOLS=0 …` | leave out the command-line tools (BusyBox in `/usr/bin`, there by default for a terminal app's shell) |
| `make APPEND="saule.on_exit=restart" run` | pass kernel command line options |
| `make apps` | build only the apps, into `build/apps`: what the window manager opens when it runs nested on a desktop |
| `make usb-image` | `build/saule-linux.img`, a USB stick as a file: see [Installing](#installing) |
| `make usb DEVICE=/dev/sdX` | write that image to a stick (as root; the stick is erased) |
| `make run-usb` | the QEMU netbook started from the stick, into the installer. `DISK=file.qcow2` gives it a disk to practise on instead of `disks/netbook.qcow2` |
| `make run-disk` | the QEMU netbook started from its hard disk alone: what the installer put there |
| `make test` | run init's and the installer tool's tests |
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
2. **Choose a disk**: every disk but the stick (SATA, NVMe, eMMC, USB),
   with its size and what is on it: empty, Windows, another Linux, both,
   Saule Linux. A system that no longer starts is still told for what it
   was: its partitions say whose they are after their file systems are
   gone, and a GPT damaged at the start of the disk is read from its copy
   at the end.
3. **Confirm**: said to fit the disk. An empty one is simply installed on.
   One with Windows on it says so, and asks to erase with a button Enter
   can't press. One with Saule Linux already offers to keep its files:
   the system is written again and `/data` is left alone.
4. **Installing**, then **Restart**, with the stick taken out first.

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

**After installing**, the first start asks three things, over the desktop
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

## Sound, network and sleep

The session runs as an ordinary account, which can't put the machine to
sleep, turn the sound card's volume or bring up a network. `saule-system`
(`system/`, Rust) runs as root beside it for that; init starts it and keeps
it running. It is spoken to through files in `/run/saule/system`:

- a file dropped in `requests/` is one request, a line of text (`sleep`,
  `volume 40`, `mute 1`, `wifi join Home<tab>password`). Only the session's
  account can write there;
- `power`, `sound` and `network` say how each stands, a few lines each,
  rewritten as they change. `cat /run/saule/system/network` shows what
  the window manager's bar is showing.

What it does:

- **Sleep**: suspends to memory when asked and when the lid closes. The
  window manager locks the screen first, so what wakes is locked.
- **Sound**: finds the card's volume and its switch among the mixer's
  controls and turns them, with the kernel's own requests (no ALSA
  library but for playing). The volume chosen is kept on the data disk.
- **A cable**: an interface with one in it is brought up and asked an
  address for by DHCP (BusyBox's `udhcpc`, so `TOOLS=1`).
- **Wi-Fi**: runs `wpa_supplicant` on the machine's radio and talks to it
  over its control socket: the networks in range, joining one, the ones
  known (kept on the data disk). The radios' firmware files and
  `wpa_supplicant` itself are gathered by `scripts/fetch-wifi.sh`; `WIFI=0`
  leaves them out of the image.

`make NET=1 run-netbook` plugs a cable into the QEMU netbook. QEMU has no
radio, no lid and no battery, and its default graphics card comes back
from sleep with a blank screen (the real ones restore theirs).

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
  src/image.rs           a stick's image, made on the build machine
system/                  saule-system (Rust): what the session asks the
                         machine for
  src/requests.rs        requests, as files dropped in a folder
  src/power.rs           sleep, and the lid
  src/sound.rs           the volume, through the card's control device
  src/network.rs         a cable's interface, and how every one stands
  src/wifi.rs            wpa_supplicant, started and talked to
  dhcp                   what udhcpc runs when it is given an address
installer/               the installer's window, a Saule project (uikit)
  src/model/             the disks, the installation under way, the tool
  src/pages/             one view per step
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

- **Only init is root.** The session runs as `display`, the way macOS runs
  its WindowServer as `_windowserver`. It can use the screen and input
  devices, through the `video` and `input` groups, and its own
  home, but it can't write system files or read other accounts'.
- **No way back to root:** the session starts with `no_new_privs`, so
  even a set-uid program can't raise it.
- **It fails closed:** if the account is missing, the session doesn't start
  as root instead. `saule.user=root` has to be passed on purpose.
- The debug shell (`DEBUG=1`) is root, as a debugging tool.

Apps the window manager starts today run as `display` too. Separating them
is the next step on the roadmap.

## The two layers

The Rust layer is what a Saule program can't be: init, the kernel's
configuration, `saule-install`, native Saule packages and `saule build`.
Everything a person sees is the Saule layer: the session, the display
server and window manager, the apps, and the installer's window.

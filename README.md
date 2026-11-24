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
| `make test` | run init's tests |
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
| `saule.user=NAME` | `display` | the account it runs as; `root` only when you ask for it |
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

## Storage

The system runs from memory: the initramfs is all of it, and what it writes
there is gone at power-off. What should last goes on the **data disk**,
which init (`init/src/storage.rs`) mounts at `/data`:

- an ext2/3/4 file system labelled `SAULE`, on any disk or partition, is
  the data disk (`mke2fs -L SAULE /dev/sdX1` makes one by hand);
- failing that, an internal disk that is **completely empty** — nothing in
  its first megabyte — gets one partition with such a file system: ext4,
  journaled;
- a disk with anything on it is never touched, and neither is one on a USB
  port. The netbook's disk may still hold Windows.

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
  kernel.config          every kernel option we turn on, grouped and explained
  drivers.list           which graphics driver pack each graphics chip needs
init/                    PID 1 (Rust)
  src/main.rs            the boot sequence
  src/mounts.rs          /proc /sys /dev /run /tmp
  src/storage.rs         the data disk: find it, or make it, and mount /data
  src/console.rs         the real terminal behind /dev/console (for Ctrl-C)
  src/session.rs         start the session, restart it, reap orphans
  src/output.rs          what the session prints, copied to every console
  src/account.rs         accounts from /etc/passwd, dropping root for the session
  src/seat.rs            screen and input devices to video/input, hotplug included
  src/power.rs           poweroff / reboot and the signals for them
  src/cmdline.rs         saule.* kernel options
  src/log.rs             init's own messages
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

## Who writes what

The Rust layer (init, kernel config, native Saule packages, `saule build`)
is written by Claude. The Saule layer (the session, the display server, the
window manager) is written by the user, with Claude's guidance.

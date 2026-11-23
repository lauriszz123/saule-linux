# Saule Linux

A minimal Linux distribution that boots straight into a Saule program.

There's no systemd, no package manager, no X11 or Wayland: a Linux kernel,
a small init written in Rust, and one Saule project — the **session** —
built into a single executable with `saule build`. Today the session prints
a greeting. Later it becomes a display server and window manager written in
Saule (see [ROADMAP.md](ROADMAP.md)).

```
$ make run
...
[init] starting session: /bin/session

  Hello from Saule on Linux 6.18.64-saule
  uptime  0.82 s
  memory  2051192 kB

[init] session ended
[init] powering off
```

It boots in under a second. The kernel is 3.1 MB, and the initramfs is 4.5 MB
(5.6 MB with the debug shell).

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
| `make` | download and build the kernel, build init and the session, pack the initramfs |
| `make run` | boot in QEMU on this terminal (`Ctrl-A X` quits QEMU) |
| `make run-gui` | boot in a QEMU window: the screen the display server will draw on |
| `make run-netbook` | boot in QEMU dressed as the HP Mini 210 (N450, 2 GB, SeaBIOS, 1024×600, USB 2.0, PS/2): `scripts/qemu-netbook.sh` |
| `make DEBUG=1 run` | add BusyBox: a shell when the session ends |
| `make APPEND="saule.on_exit=restart" run` | pass kernel command line options |
| `make menuconfig` | change kernel options; copy the keepers into `config/kernel.config` |
| `make check-config` | list options `config/kernel.config` asked for but didn't get |
| `make clean` / `make distclean` | remove build output / everything including the kernel source |

`SAULE=/path/to/saule` (a Saule with `saule build`), `APP=/path/to/project`,
`BUSYBOX=…` and `QEMU=…` override what gets used.

## How it boots

1. **The kernel** (`config/kernel.config` on top of `allnoconfig`) unpacks the
   initramfs into RAM and runs `/init`.
2. **`/init`** (`init/`, Rust, static musl binary) is PID 1:
   - mounts `/proc`, `/sys`, `/dev`, `/run` and `/tmp`;
   - sets the hostname and prints `/etc/motd`;
   - runs `/bin/session` on the console with a clean environment
     (`HOME=/root`);
   - reaps orphaned processes;
   - when the session ends, it follows `saule.on_exit`. A crash restarts the
     session after a second; five quick crashes in a row give up.
   - `poweroff`/`reboot` (signals `SIGUSR2`/`SIGTERM`) and Ctrl-Alt-Del shut
     down cleanly.
3. **The session** (`session/`) is an ordinary Saule project. `make` builds
   it with `saule build` into one executable — bytecode, the Saule runtime,
   its native packages and bundled files — which becomes `/bin/session`.
   No Saule toolchain or source is in the image. Bundled files unpack to
   `/root/.cache/saule/apps/` on first start.

Kernel command line options read by init:

| Option | Default | |
|---|---|---|
| `saule.session=PATH` | `/bin/session` | the executable to run as the session |
| `saule.on_exit=…` | `shell` if `/bin/sh` exists, else `poweroff` | `poweroff`, `reboot`, `restart` or `shell` |

## Layout

```
Makefile                 the whole build
config/kernel.config     every kernel option we turn on, grouped and explained
init/                    PID 1 (Rust)
  src/main.rs            the boot sequence
  src/mounts.rs          /proc /sys /dev /run /tmp
  src/console.rs         the real terminal behind /dev/console (for Ctrl-C)
  src/session.rs         start the session, restart it, reap orphans
  src/power.rs           poweroff / reboot and the signals for them
  src/cmdline.rs         saule.* kernel options
session/                 the session: a Saule project (built with `saule build`)
rootfs/                  copied into the image as is
  etc/                   hostname, motd, passwd, group, profile
scripts/
  mkinitramfs.sh         assemble the tree (+ shared libraries) and pack it
  check-config.sh        catch kernel options that were silently dropped
build/                   downloads, kernel source and output (not in git)
```

## Who writes what

The Rust layer (init, kernel config, native Saule packages, `saule build`)
is written by Claude. The Saule layer (the session, the display server, the
window manager) is written by the user, with Claude's guidance.

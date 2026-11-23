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
[init] starting session: /bin/session as display

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
| `make` | download and build the kernel, build init and the session (`../saule-wm`, else `session/`), pack the initramfs |
| `make run` | boot in QEMU on this terminal (`Ctrl-A X` quits QEMU) |
| `make run-gui` | boot in a QEMU window: the screen the display server will draw on |
| `make run-netbook` | boot in QEMU dressed as the HP Mini 210-1001sa (N450, 1 GB, SeaBIOS, 1024×600, USB 2.0, PS/2, a USB tablet as the pointer), with KVM when available: `scripts/qemu-netbook.sh` |
| `make SLOW=1 run-netbook` | the same at about the netbook's speed: KVM capped at 8% of one core (an estimate; `SLOW=12` sets another percentage). Calibrate with `examples/bench` |
| `make DEBUG=1 run` | add BusyBox at `/bin`: a shell when the session ends |
| `make APP=examples/compositor-demo APPS="examples/shine-demo" run-netbook` | a session plus apps, each built into `/bin/apps/<name>` |
| `make TOOLS=0 …` | leave out the command-line tools (BusyBox in `/usr/bin`, there by default for a terminal app's shell) |
| `make APPEND="saule.on_exit=restart" run` | pass kernel command line options |
| `make menuconfig` | change kernel options; copy the keepers into `config/kernel.config` |
| `make check-config` | list options `config/kernel.config` asked for but didn't get |
| `make clean` / `make distclean` | remove build output / everything including the kernel source |

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
   - hands the screen and input devices to the `video` and `input` groups,
     including devices plugged in later;
   - runs `/bin/session` on the console as the **`display`** account (not
     root) with a clean environment (`HOME=/home/display`,
     `XDG_RUNTIME_DIR=/run/user/100`). See [Security](#security);
   - reaps orphaned processes;
   - when the session ends, it follows `saule.on_exit`. A crash restarts the
     session after a second; five quick crashes in a row give up.
   - `poweroff`/`reboot` (signals `SIGUSR2`/`SIGTERM`) and Ctrl-Alt-Del shut
     down cleanly.
3. **The session** (`session/`) is an ordinary Saule project. `make` builds
   it with `saule build` into one executable — bytecode, the Saule runtime,
   its native packages and bundled files — which becomes `/bin/session`.
   No Saule toolchain or source is in the image. Bundled files unpack to
   `/home/display/.cache/saule/apps/` on first start.

Kernel command line options read by init:

| Option | Default | |
|---|---|---|
| `saule.session=PATH` | `/bin/session` | the executable to run as the session |
| `saule.user=NAME` | `display` | the account it runs as; `root` only when you ask for it |
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
  src/account.rs         accounts from /etc/passwd, dropping root for the session
  src/seat.rs            screen and input devices to video/input, hotplug included
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

## Security

The plan follows macOS (see ROADMAP.md, "Security"). Today:

- **Only init is root.** The session runs as `display`, the way macOS runs
  its WindowServer as `_windowserver`. It can use the screen and input
  devices, through the `video` and `input` groups, and its own
  `/home/display`, but it can't write system files or read other accounts'.
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

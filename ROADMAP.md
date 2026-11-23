# Roadmap

Where Saule Linux is going, in the order it gets built. Each phase ends with
something that runs.

**Owners:** 🦀 = Rust, written by Claude. 🌙 = Saule, written by the user
with Claude's guidance.

```
 ┌──────────────────────────────────────────────────────────────────┐
 │ apps: gamemaker, games, tools           (Saule, unchanged)  🌙   │
 ├──────────────────────────────────────────────────────────────────┤
 │ display server + window manager         (Saule)             🌙   │  phase 4–6
 ├──────────────────────────────────────────────────────────────────┤
 │ Shine backends · IPC · shared buffers   (Rust natives)      🦀   │  phase 3
 ├──────────────────────────────────────────────────────────────────┤
 │ saule runtime · `saule build` bundles   (Rust)              🦀   │  phase 2
 ├──────────────────────────────────────────────────────────────────┤
 │ init · kernel config · images           (Rust, make)        🦀   │  phase 0–1
 └──────────────────────────────────────────────────────────────────┘
```

---

## Phase 0: it boots ✅

- [x] Kernel 6.18 LTS from `allnoconfig` + `config/kernel.config`. It's 3.1 MB
      and already has DRM (bochs and simpledrm), evdev input and Unix sockets
      for the later phases.
- [x] `saule-init`: mounts, hostname, motd, runs the session, restarts it on a
      crash, reaps orphans, powers off and reboots on signals.
- [x] `session/`: a Saule session that reads `/proc` and prints.
- [x] `make run` boots in QEMU to the session in under a second.
      `make DEBUG=1 run` falls back to a BusyBox shell.

## Phase 1: a real distro 🦀

- [ ] **`make image`**: build the initramfs *into* the kernel
      (`CONFIG_INITRAMFS_SOURCE`) so the whole system is one
      `BOOTX64.EFI`.
- [ ] **`make run-efi`**: boot that file under UEFI firmware (OVMF), the
      way a real PC will.
- [ ] **`make usb DEVICE=…`**: write a FAT32 EFI partition to a stick, and
      boot a real PC from it.
- [ ] **Persistent storage**: a `/data` partition mounted by init, so saved
      projects survive a reboot. This needs disk drivers in the kernel
      (virtio-blk, AHCI, NVMe, USB storage), a file system, and a disk
      image for `make run`.
- [ ] **Logs**:
  - init's messages go to `/dev/kmsg`, so they show in `dmesg`;
  - the session's stderr is kept in `/run/session.log`.
- [ ] **Real-time clock**, so `Os.date()` is right.
- [ ] **No glibc in the image**: needs the static musl runner (phase 2).

## Phase 2: `saule build` 🦀 (in ~/saule) — first version ✅

`saule build` turns a project into one executable that runs without the
toolchain.

```
saule build [DIR | FILE] [-o OUT] [--keep-source]
```

- [x] **Bytecode format** (`saule-vm/src/bytecode/`): the whole compiled
      program; folded stdlib and native-package constants are written as
      paths and found again on load. Chunks are verified on load, and the
      header pins the exact Saule build.
- [x] **Bundle**: the `saule` binary + an appended archive (bytecode,
      native packages, the files under `bundle:` in saule.config, default
      `["assets"]`) + a footer the runner finds at start-up.
- [x] **Assets and native packages** unpack once to
      `~/.cache/saule/apps/<name>-<hash>/`, which is `Project.root` and the
      app's Saule home.
- [x] **No source in the file**: errors keep file, line and column
      (`--keep-source` keeps the text too).
- [x] **The distro uses it**: `session/` is built into `/bin/session`; the
      image carries no toolchain and no Saule source.
- [x] **Checked**: all 127 `tests/run` fixtures print the same built as run
      (`scripts/test_bytecode.sh` in ~/saule); gamemaker builds into one
      9.8 MB file and renders identically.
- [ ] **Smaller runner**: today the runtime part is the full `saule` binary
      (6.2 MB, including fmt/check/install). A `saule-runner` without them.
- [ ] **Static, musl**: native packages linked into the runner instead of
      `dlopen`ed, so a build has no glibc dependency.
- [ ] **Cross builds** (`--target`): Windows, then macOS.

## Phase 3: the platform layer 🦀

What the display server needs from Rust. Everything above it is Saule.

- [ ] **Shine backend interface**: split today's minifb code behind a trait
      (window, present, input, clipboard, cursor) so backends can be swapped.
- [ ] **Console backend** (bare screen, no X11):
  - drawing to the screen through DRM/KMS dumb buffers, double-buffered
    and in sync with the monitor's refresh (vsync);
  - keyboard and mouse from `/dev/input/event*`, translating key codes into
    Shine's key names, with a US layout first;
  - the mouse cursor drawn by the server.
- [ ] **IPC package**:
  - non-blocking Unix sockets (listen, accept, connect, send and receive
    `Bytes`);
  - **shared buffers**: memfd + mmap, passed between processes over the
    socket, and used in Saule as a Shine `Image`;
  - waiting on several sockets at once, instead of busy-looping.
- [ ] **Shine client mode**:
  - when `SAULE_DISPLAY` is set, `Window.create` connects to the display
    server instead of opening a real window;
  - `present` tells the server a new frame is ready;
  - input arrives from the server.

  uikit and every app work unchanged.
- [ ] **Byte packing for the protocol**: check what `Bytes` already offers,
      and add reading and writing of u16/u32/f32 if needed.

## Phase 4: the display server 🌙

- [ ] Decide where it lives (a repo of its own, or inside this one).
- [ ] **Protocol**: messages and their encoding.
  - client → server: `hello`, `createWindow(title, w, h)`, `commit(damage)`,
    `setTitle`, `destroyWindow`;
  - server → client: `configure(w, h)`, input events, `focus`,
    `closeRequested`, `frameDone`.

  `configure` matters from day one: in tiling mode the **server** decides a
  window's size, and the client redraws at that size.
- [ ] **Server loop**: accept clients, read messages, keep the window list,
      composite into the screen, and route input to the focused window.
- [ ] **Nested mode first**: the server runs as an ordinary Shine window on
      the desktop (WSLg), so it can be developed before the console
      backend exists.
- [ ] **Test client**: a window with a bouncing square, which prints the
      input it receives.
- [ ] **Milestone**: two test clients side by side, each getting its own
      clicks.

## Phase 5: tiling window manager 🌙

- [ ] **Layout model**: either a split tree (i3-style) or master and stack
      (dwm-style); decide at the start of the phase.
- [ ] **Focus and shortcuts**:
  - `Super+Enter` opens the launcher;
  - `Super+arrows` moves focus;
  - `Super+Shift+arrows` moves windows;
  - `Super+Q` closes a window.
- [ ] **Decorations** (server-side title bars and borders), in the style of
      gamemaker's theme.
- [ ] **Workspaces**.
- [ ] **Launcher**: start a Saule app as a client process.
- [ ] **Milestone**: Saule Linux boots straight into the window manager.

## Phase 6: floating mode, switchable 🌙

- [ ] Toggle per workspace and per window.
- [ ] Move, resize, stacking order, minimise and maximise. Most of this logic
      already exists in gamemaker's MDI.
- [ ] Tiled windows remember their place when a window floats and comes
      back.

## Phase 7: apps 🌙

- [ ] Gamemaker running as a client, unchanged.
- [ ] A terminal (needs a pty package 🦀).
- [ ] Settings, a file browser, …

---

## Open decisions

| Decision | When |
|---|---|
| Where the display server lives | start of phase 4 |
| Tiling style: split tree or master/stack | start of phase 5 |
| Whether IPC is part of Shine or a separate `saule-ipc` package | phase 3 |
| Bundle format details (bytecode or sources, compression) | phase 2 |
| Keyboard layouts beyond US | phase 3 |

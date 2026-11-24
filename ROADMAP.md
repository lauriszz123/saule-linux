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

The bootable stick waits: everything is developed in QEMU for now
(`make run-netbook` dresses QEMU as the HP Mini 210-1001sa; `SLOW=1` runs it
at about its speed).

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
- [x] **Graphics for the netbook**, as far as QEMU can check it: i915
      (with AGP_INTEL's GTT) for its GMA 3150, built in, no firmware
      files; it first runs on the real machine, since QEMU can't emulate
      the chip. The fallback when a chip has no driver, the screen the
      BIOS set up through simpledrm, runs in `make run-vesa`. About This
      System names the card (PCI ids, `pci.ids` where there is one).
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

- [x] **Shine backend interface**: minifb is behind a trait
      (`saule-shine/src/state/backend.rs`); the desktop and the bare screen
      are two backends, picked by `Window.create`.
- [x] **Console backend** (bare screen, no X11), `saule-shine/src/state/console/`:
  - drawing to the screen through DRM/KMS dumb buffers, double-buffered
    with page flips in sync with the display;
  - keyboard and mouse from `/dev/input/event*` (mice, PS/2 touchpads,
    tablets), key codes to Shine's key names, US layout;
  - the hardware cursor where the driver has one (i915), drawn into the
    frames where not (QEMU's bochs);
  - the text console switched off while Shine owns the screen, and put
    back on exit (and by init after a crash); Ctrl-Alt-Del still reboots.
  - Checked in QEMU as the netbook: `examples/shine-demo` at 1024×600,
    60 FPS, typing and mouse working.
  - No GPU acceleration: the netbook's GMA 3150 only does OpenGL ES 2
    through Mesa, slower than Shine's own renderer for 2D.
- **IPC, part of the language** (like Android's AIDL/Binder or macOS's XPC):
  services are objects behind an `extern interface`, and calling another
  process is calling a method.
  ```saule
  export extern interface Display
    fn createWindow(title: string, width: integer, height: integer, events: WindowEvents) -> Window
  end
  export extern interface WindowEvents
    async fn configure(width: integer, height: integer)   -- no reply: never waits on an app
  end
  local display = Services.connect(Display, "saule.display")
  ```
  `fn` waits for its reply (serving incoming calls meanwhile, so callbacks
  cannot deadlock); `async fn` with no result is fire-and-forget; `async fn
  -> T` returns a `Task<T>` to `await` — anywhere, not only inside `async`
  code. Plain data classes are copied; `extern interface` values pass by
  reference.
  - [x] **1. Transport** in the Saule runtime (`saule/crates/saule-runtime/src/ipc/`):
        whole messages and handles over Unix sockets (Linux, macOS) or named
        pipes (Windows), shared memory, services by name visible only to
        their user, peer identity from the kernel, calls with and without
        replies, objects by reference with counted release, a peer that
        stops reading cut off rather than blocking the sender.
  - [x] **2.** `extern interface`, `fn` and no-result `async fn`:
        parser, formatter, checker (what can cross a process boundary),
        compiler, runtime proxies and exports; `Iface.connect`,
        `Iface.publish` and `Looper`. Remote failures are catchable.
        Fixtures in `saule/tests/run/ipc` and `tests/errors/ipc`.
  - [ ] **3.** `async fn -> T`, `Task<T>` and `await`.
- [x] **Shine client mode and `Compositor`** (`saule-shine/src/protocol.rs`):
  - when `SAULE_DISPLAY` is set, `Window.create` opens the window inside
    the display server; frames go through shared memory, input and the
    window's size come from the server. Apps run unchanged.
  - a display server written in Saule uses `Compositor`: `start`, `poll`
    (opened / frame / title / closed), `image(id)` to draw a window,
    `configure`, and `sendKey` / `sendText` / `sendPointer` / `sendButton`
    / `sendWheel` / `sendFocus` / `requestClose`.
  - the transport moved into its own crate, `saule-ipc`, used by both
    Saule's runtime and Shine.
  - Checked on macOS (as a desktop window) and in QEMU as the netbook:
    `examples/compositor-demo` running `examples/shine-demo` unchanged,
    typing and mouse forwarded.
- [x] **Several programs in one image**: `make APPS="examples/a examples/b"`
      builds each into `/bin/apps/<name>`.

## Phase 4: the display server 🌙

- [x] Where it lives: its own repo, [saule-wm](https://github.com/lauriszz123/saule-wm).
      `make` builds `../saule-wm` as the session when it's there (`WM=…`
      points elsewhere), and init starts it as the `display` account.
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
- [ ] **Launcher**: start a Saule app as a client process. Through init's
      launcher service once it exists (see Security, step 2), not
      `Os.spawn`, so apps don't inherit the window manager's access.
- [ ] **Milestone**: Saule Linux boots straight into the window manager.

## Phase 6: floating mode, switchable 🌙

- [ ] Toggle per workspace and per window.
- [ ] Move, resize, stacking order, minimise and maximise. Most of this logic
      already exists in gamemaker's MDI.
- [ ] Tiled windows remember their place when a window floats and comes
      back.

## Phase 7: apps 🌙

- [ ] Gamemaker running as a client, unchanged.
- [ ] A terminal. What it needs from Rust is done 🦀: `Os.spawnPty` runs a
      shell behind a pseudo-terminal (Unix pty / Windows ConPTY), the kernel
      has pseudo-terminals and init mounts `/dev/pts`, and every image has
      BusyBox's tools in `/usr/bin` (`TOOLS=1`, the default). Checked on
      Saule Linux with `examples/pty-check`.
- [ ] Settings, a file browser, …

## Security: the macOS model 🦀🌙

Decided: Saule Linux follows macOS. The person has one account, and apps run
as that account. Each app is sandboxed on its own, though: it gets what its
manifest asks for and what the person allows, and nothing else. In Linux
terms:

| macOS | Saule Linux |
|---|---|
| `launchd`, PID 1, starts everything | `saule-init` |
| WindowServer as `_windowserver` | the window manager as `display` |
| the user's account | one account for the person, `user` |
| App Sandbox (Seatbelt) | Landlock (files), a network namespace or seccomp (network), no device access |
| app container `~/Library/Containers/<id>` | `~/Containers/<app-id>`, the only place an app writes by default |
| entitlements, signed into the app | `entitlements: [...]` in `saule.config`, packed by `saule build` |
| TCC: "X wants to use the camera" | a permission store in init; the prompt is drawn by the window manager |
| Powerbox: the open/save panel runs outside the app | the file dialog is drawn by the window manager; the app receives only the chosen file, as an open handle over IPC |
| code signing, Gatekeeper | `saule build --sign`; init refuses an app whose signature doesn't match |

Rules for every step:
- **The kernel enforces, not Saule.** An app can bring its own native
  library, which ignores anything the runtime checks. So the sandbox is
  built from kernel features and set up by init before the app's first
  instruction runs.
- **It fails closed.** A missing account, manifest or signature means the
  app doesn't start, rather than starting with more access.

Steps:
- [x] **1. Only init is root** 🦀. The session runs as `display` (uid 100)
      with `no_new_privs`, in a private 0700 home and `XDG_RUNTIME_DIR`.
      Screen and input devices go to the `video` and `input` groups, fixed
      again on hotplug from the kernel's uevents (`init/src/seat.rs`). It
      refuses to start as root unless `saule.user=root` is passed.
- [ ] **2. init launches apps** 🦀. init gets a `saule.launcher` service (an
      `extern interface`), which only `display` may call: the kernel's peer
      identity says who is calling. Apps run as `user` (uid 1000), with no
      `video` or `input` group, so they can't read the keyboard behind the
      window manager's back. The window manager's launcher calls it 🌙.
- [ ] **3. App sandbox** 🦀:
  - the kernel options: `SECURITY_LANDLOCK`, `SECCOMP_FILTER`, `NET_NS`;
  - each app is confined by Landlock to its bundle, its container and the
    system's read-only files;
  - no network unless it is entitled to it.
- [ ] **4. Entitlements** 🦀: `entitlements:` in `saule.config` (e.g.
      `network.client`, `files.user-selected`, `audio.output`), checked by
      `saule build` and carried in the bundle's manifest. init grants
      exactly those.
- [ ] **5. Privacy prompts** 🦀🌙: the first time an app asks for something
      sensitive (camera, microphone, files outside its container), init asks
      the window manager to show a prompt and remembers the answer.
      Settings lists and revokes them. This needs persistent `/data`
      (phase 1).
- [ ] **6. File dialogs (Powerbox)** 🦀🌙: an app asks for a file to be
      opened or saved; the window manager draws the dialog and passes
      the app only the chosen file, as a handle.
- [ ] **7. Signing** 🦀: `saule build --sign` with a developer key, checked
      by init at launch, so a built app's code and entitlements can't be
      altered afterwards.

---

## Open decisions

| Decision | When |
|---|---|
| Where the display server lives | **decided: its own repo, `saule-wm`** |
| Tiling style: split tree or master/stack | **decided: split tree (i3-style)** |
| Whether IPC is part of Shine or a separate `saule-ipc` package | **decided: part of the language** (`extern interface`) |
| Bundle format details (bytecode or sources, compression) | phase 2 |
| Keyboard layouts beyond US | phase 3 |
| Security model | **decided: macOS-style** (one account, an app sandbox, entitlements, prompts) |

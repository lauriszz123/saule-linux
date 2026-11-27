# Roadmap

Where Saule Linux is going, in the order it gets built. Each phase ends with
something that runs.

**Layers:** 🦀 = Rust: init, the kernel's configuration, natives. 🌙 =
Saule: the session, the window manager, the apps.

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

The netbook has a BIOS and no EFI; most newer PCs have UEFI and no BIOS. The
stick and the disk it installs boot through GRUB on both. Everything is
developed in QEMU (`make run-netbook` dresses
QEMU as the HP Mini 210-1001sa; `SLOW=1` runs it at about its speed).

- [x] **`make usb-image`**: a USB stick as one file, made without root: an
      MBR, GRUB, and an ext2 file system holding the kernel, the initramfs,
      the installer and the driver packs.
- [x] **`make run-usb`**: the QEMU netbook started from that stick through
      SeaBIOS, the way the real one will be. `EFI=1`: through UEFI
      firmware, the way a newer PC is.
- [x] **UEFI**: GRUB for UEFI in a FAT partition of its own, on the stick
      and on the disk. Secure Boot has to be off.
  - [ ] signed, so that it needn't be.
- [x] **`make usb DEVICE=…`**: write it to a stick.
- [x] **The installer** (`installer/`, `install/`): choose a disk, see what
      is on it, install, restart. It keeps the files of a Saule Linux
      already there when asked, and gives the machine the one graphics
      driver pack its chip needs.
  - [x] a first start after installing: the computer's name, the time
        zone, the look (saule-wm's first-start setup, `init/src/system.rs`);
  - [x] what was on a disk told from its partitions as well as its file
        systems, so a system that no longer starts is still named;
  - [x] the keyboard's layout, chosen on the stick before anything is
        typed, and typed with from the first start on (XKB's layouts,
        read by libxkbcommon in Shine);
  - [x] a person and a password, asked by the installer: a login screen
        when the computer starts, and the password to unlock the screen
        (`password/`, `system/src/login.rs`, saule-wm's lock screen);
  - [x] the account called after the person, not `display`: its home, a
        terminal's prompt (`init/src/person.rs`);
  - [x] changing the name, the time zone, the password and the layouts
        later, in Settings;
  - [ ] more than one person to a computer, each with a home of their own;
  - [ ] the disk encrypted, so that the password also keeps out somebody
        who takes the disk;
  - [x] the clock set from a time server once there is a network
        (`system/src/clock.rs`);
  - [ ] setting the clock by hand;
  - [x] updating an installed system from a signed file, found by
        Settings on a drive or in Downloads (`install/src/update.rs`,
        `make update`);
  - [ ] somewhere updates are downloaded from, and being told there is
        one;
  - [x] the size of everything chosen in Settings, or by the screen's
        resolution (Shine's `Window.setScale`; apps drawn at the same
        scale, pixel for pixel);
  - [ ] installing beside Windows instead of over it.
- [ ] **On the real netbook** (an HP Mini 210-1000): the stick boots from
      its BIOS, the desktop comes up, apps open, the battery, the lid, the
      backlight, the touchpad and Bluetooth's radio are found. From its
      report (Mod+Shift+R):
  - [x] its GMA 3150 draws the screen: the report says `i915 (chipset:
        Pineview M)`. Mesa's i915 had turned down four of Shine's six
        shaders (too many instructions and rounds of texture reads, a
        branch, a loop). They are compiled in small variants now, and the
        blur is a shader of uikit's, five reads a pass; every one links
        against the driver with no chip present
        (`build/testing/gen3check`). The window manager is drawn by the
        chip; programs drawing on it too is still a trial, the third
        entry in GRUB's menu;
  - [x] drawn by the processor, motions only fade, and a full-screen
        fade no longer goes pixel by pixel through the sampled path
        (0.27 s a frame, now 0.04);
  - [x] its Wi-Fi card is a BCM4312 (14e4:4315): b43 loads its firmware
        and the card joins a network and stays on it;
  - [ ] its webcam has no driver (USB video class is not built).
- [ ] **Other machines**: none has been tried on either. What is there for
      them, all of it unproven on the hardware it is for:
  - [x] the `intel` driver pack, for every Intel chip from Gen4 to Meteor
        Lake. Built from Mesa 24.0, the last whose Intel drivers build
        without an OpenCL compiler (`scripts/build-drivers.sh`); the chips
        after need the newer Mesa and that compiler;
  - [x] a newer laptop's touchpad: HID over I2C, with the chipset drivers
        under it for Intel from Bay Trail to Meteor Lake and for AMD.
        Shine makes the gestures: two fingers scroll, a tap clicks, two
        fingers click the secondary button;
  - [x] processors that run cool: frequency scaling and idle states, and
        the firmware's thermal limits;
  - [ ] the firmware files newer Intel graphics load for their deeper
        power states (i915 works without them, and warns);
  - [ ] kernel drivers for AMD and NVIDIA graphics (they get the screen as
        GRUB left it, drawn on the CPU);
  - [x] sleep, from the menu and by the lid; the battery's level; sound
        and its volume; a cable's network; Wi-Fi with Intel's and
        Atheros's radios (`system/`, and the window manager's bar). Of
        these only sleep, the volume and a cable have run anywhere, in
        QEMU;
  - [x] the keys for volume and brightness, and the screen's brightness
        (`system/src/backlight.rs`; ACPI's video device and Dell's and
        HP's own key drivers in the kernel). The volume keys have run in
        QEMU; nothing there has a backlight;
  - [ ] the key drivers of other makers (Lenovo's, ASUS's, Acer's);
  - [x] Bluetooth: the kernel's stack, BlueZ and a D-Bus daemon for it
        in the image, radio, search, pairing and connecting from Settings
        (`system/src/bluetooth.rs`). For mice and keyboards. Tried on the
        kernel's radio that is a program (`/dev/vhci`, played by BlueZ's
        `btvirt`), in QEMU; no real device paired on a real radio yet;
  - [ ] sound over Bluetooth, which wants a sound server;
  - [x] the microphone's level, whether headphones are in, and which
        card plays (`system/src/sound.rs`);
  - [ ] Wi-Fi radios other than those (Broadcom's, Realtek's, MediaTek's),
        and the firmware as a pack the installer chooses, as the graphics
        drivers are.
- [x] **Persistent storage**: init mounts a data disk at `/data` (an ext
      file system labelled `SAULE`; an internal disk that is completely
      empty is partitioned and formatted for it, one with anything on it is
      never touched), and the session's home is on it, so settings survive
      a reboot. The kernel has AHCI, IDE-mode SATA and USB storage, and
      ext4; `make run-netbook` has the netbook's 250 GB disk
      (`disks/netbook.qcow2`, kept between runs).
  - [x] safe against a power cut: ext4 with a journal, `e2fsck` before
        mounting, synchronous writes, and caches kept in memory;
  - [x] NVMe, eMMC and virtio-blk, for other machines.
- [ ] **Logs**:
  - init's messages go to `/dev/kmsg`, so they show in `dmesg`;
  - the session's stderr is kept in `/run/session.log`.
- [x] **Graphics for the netbook**, as far as QEMU can check it: i915
      (with AGP_INTEL's GTT) for its GMA 3150, built in, no firmware
      files; it first runs on the real machine, since QEMU can't emulate
      the chip. The fallback when a chip has no driver, the screen the
      BIOS set up through simpledrm, runs in `make run-vesa`. About This
      System names the card (PCI ids, `pci.ids` where there is one).
      Its driver pack, `gen3`, is built (`make drivers DRIVERS=gen3`): Mesa's
      i915 without LLVM, which would be 100 MB in a machine with 1 GB
      (`scripts/patches/mesa-26.0.8-i915-without-llvm.patch`). In QEMU the pack
      loads and draws through its software driver; the chip's own driver
      first runs on the chip.
- [x] **The clock**: the kernel reads the PC's clock when it starts, so
      `Os.date()` is right, in UTC.
  - [x] a time zone, chosen at the first start. Saule's `Os.date` is UTC
        only: init says what the zone is at present (`/run/saule/time`) and
        the window manager's clock adds it.
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
      image carries no Saule source.
- [x] **One runtime for every program** (`saule build --shared=/bin/saule`):
      the output is the bytecode and the bundled files behind a first line
      that names the saule to run it, and its native packages are loaded
      from that saule's home. The session, the installer and the ten apps
      were twelve megabytes each, all of it in memory from the start; they
      are a few hundred kilobytes, and the image has one `/bin/saule` and
      one Shine. What the initramfs unpacks to went from 210 MB to 69 MB.
- [x] **Checked**: all 127 `tests/run` fixtures print the same built as run
      (`scripts/test_bytecode.sh` in ~/saule); gamemaker builds into one
      9.8 MB file and renders identically.
- [ ] **Smaller runner**: today the runtime part is the full `saule` binary
      (6.9 MB, including fmt/check/install), which is also what the image
      has as `/bin/saule`. A `saule-runner` without them.
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
- [x] **Protocol**: messages and their encoding. It is Shine's own
      (client mode and `Compositor`, phase 3), so an app needs no change.
  - client → server: `hello`, `createWindow(title, w, h)`, `commit(damage)`,
    `setTitle`, `destroyWindow`;
  - server → client: `configure(w, h)`, input events, `focus`,
    `closeRequested`, `frameDone`.

  `configure` matters from day one: in tiling mode the **server** decides a
  window's size, and the client redraws at that size.
- [x] **Server loop**: accept clients, read messages, keep the window list,
      composite into the screen, and route input to the focused window.
- [x] **Nested mode first**: the server runs as an ordinary Shine window on
      the desktop (WSLg), so it can be developed before the console
      backend exists.
- [x] **Test client**: `examples/shine-demo`.
- [x] **Milestone**: two test clients side by side, each getting its own
      clicks.

## Phase 5: tiling window manager 🌙

- [x] **Layout model**: a split tree (i3-style).
- [x] **Focus and shortcuts** (the keys as built are in saule-wm's README;
      they follow the desktop it runs on and can be changed in Settings):
  - `Super+Enter` opens the launcher;
  - `Super+arrows` moves focus;
  - `Super+Shift+arrows` moves windows;
  - `Super+Q` closes a window.
- [x] **Decorations** (server-side title bars and borders), in the style of
      gamemaker's theme.
- [x] **Workspaces**.
- [x] **Launcher**: start a Saule app as a client process.
  - [x] through init (see Security, step 2), not `Os.spawn`, so apps
        don't inherit the window manager's access.
- [x] **Milestone**: Saule Linux boots straight into the window manager.

## Phase 6: floating mode, switchable 🌙

- [x] Floating windows, beside the tiled ones.
- [x] Move, resize, stacking order and zoom.
  - [x] minimise: a window goes to the Dock, past its divider, and comes
        back when clicked there (the button, the app menu, Mod+M).
- [x] Tiled windows remember their place when a window floats and comes
      back, and the whole layout comes back from windowed mode
      (saule-wm's `Home`).
- [x] Windows come, go and make room with a motion (`WindowMotion`), and
      a window switcher on Mod+Tab (`Switcher`).
- [x] Left alone, the screen dims (the backlight turned down in steps,
      and what is on it blurred a little) and then locks; how long each
      takes, how dim, and whether to blur are chosen in Settings →
      Displays.

## Phase 7: apps 🌙

- [x] Gamemaker running as a client, unchanged.
- [x] A terminal (saule-linux-apps' `terminal/`): BusyBox's shell, `vi`
      and `top` run in it. Under it: `Os.spawnPty` runs a shell behind a
      pseudo-terminal (Unix pty / Windows ConPTY), the kernel has
      pseudo-terminals and init mounts `/dev/pts`, and every image has
      BusyBox's tools in `/usr/bin` (`TOOLS=1`, the default).
  - [ ] tabs; wide characters; the mouse reported to programs.
- [x] Settings and About This System
      ([saule-linux-apps](https://github.com/lauriszz123/saule-linux-apps)).
- [x] A file browser and a text editor (`files/`, `editor/`), sharing how
      a folder is read and the open and save panel (`lib/fileskit`). One
      app opens a file in another by asking the window manager
      (`WmSettings.launch`).
  - [ ] Saule's own call for what a file is (its size, its date, whether
        it is a folder): the file browser asks the system's `stat`, and
        copies and removes folders with `cp` and `rm`;
  - [ ] choosing several files at once, dragging them, and copies that
        don't hold the window;
  - [x] USB sticks and other disks mounted and shown, and ejected
        (`system/src/disks.rs`; FAT, exFAT, NTFS and ext in the kernel);
  - [x] an app told its window is closing in time to ask about unsaved
        changes (Shine's `Window.holdClose`, uikit's `App.onCloseRequest`);
  - [x] an app for what else is in a folder: pictures, PDFs and Markdown
        (`preview/`).
- [x] **Apps on the disk, not in memory**: a stick and an installed system
      keep the apps as files beside the initramfs
      (`disk/<version>/bin/apps`), which init mounts read only, and
      again when a stick that left is back (`init/src/disk.rs`). The
      installer copies them, and an update
      brings them in front of its initramfs, where an older updater
      leaves them for the kernel to unpack.

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
- [x] **2. init launches apps** 🦀. The window manager asks init for an
      app by a file dropped in a folder that is `display`'s alone
      (`/run/saule/launcher/requests`), as `saule-system` is asked: the
      folder's owner is the kernel's word for who is calling. Apps run as
      the person's account (`user`, uid 1000, named after them), with no
      `video` or `input` group, so they can't read the keyboard behind the
      window manager's back; only what is in `/bin/apps` is started. The
      window manager's services are opened to that account by init
      (`init/src/launcher.rs`), and a home the two accounts once shared
      is made the person's (`init/src/person.rs`).
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

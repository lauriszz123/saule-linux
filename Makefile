# ============================================================================
# Saule Linux
# ============================================================================
#
#   make              build the kernel and the initramfs
#   make run          boot it in QEMU, console on this terminal (Ctrl-A X quits)
#   make run-gui      boot it in a QEMU window (the screen the display server
#                     will draw on)
#   make run-netbook  boot it in QEMU dressed as the HP Mini 210-1001sa
#                     (SLOW=1: at about its speed; GL=1: with a 3D GPU;
#                     MEMORY=2G: with its memory upgraded). Its hard disk
#                     is disks/netbook.qcow2, kept between runs
#   make run-vesa     the same with no graphics driver: the BIOS's VESA
#                     screen, the fallback for a chip with no driver
#   make DEBUG=1 run  include BusyBox: a shell when the session ends
#   make drivers      build the graphics driver pack DRIVERS (default virtual,
#                     QEMU's GPU) that the image then carries
#   make apps         build only the apps, into build/apps (for the window
#                     manager nested on a desktop: see saule-wm's README)
#
#   make usb-image    build/saule-linux.img: a USB stick, as a file. Written
#                     to a stick, a PC starts from it (by its BIOS, or by
#                     UEFI with Secure Boot off) into the installer, which
#                     puts Saule Linux on the PC's disk
#   make usb DEVICE=/dev/sdX
#                     write that image to the stick at DEVICE (as root;
#                     everything on the stick is erased). From Windows: write
#                     build/saule-linux.img with Rufus, in its DD mode
#   make run-usb      start the QEMU netbook from the stick: the installer,
#                     with the netbook's hard disk (DISK=file for another)
#   make run-disk     start the QEMU netbook from its hard disk alone: what
#                     the installer put there
#                     (EFI=1 with either: a PC with UEFI firmware and no
#                     BIOS, which the netbook isn't and most newer PCs are)
#
#   make test         run init's and the installer tool's tests
#
#   make menuconfig   change kernel options (copy keepers to config/kernel.config)
#   make check-config list options config/kernel.config asked for but didn't get
#   make clean        remove what was built, keep the kernel source
#   make distclean    remove build/ entirely
#
# Neither clean touches disks/: what the system saved on the netbook's hard
# disk is not build output. Delete disks/netbook.qcow2 for an empty disk.
#
# Host packages (Ubuntu): build-essential flex bison bc libelf-dev libssl-dev
# cpio qemu-system-x86 busybox-static, plus
# `rustup target add x86_64-unknown-linux-musl`. busybox-static is the
# command-line tools (TOOLS=1, the default) and the DEBUG=1 shell. A stick
# also needs GRUB (grub-common grub-pc-bin grub-efi-amd64-bin) and, for its
# FAT partition, dosfstools and mtools: scripts/mkgrub.sh fetches for itself
# whichever aren't installed. EFI=1 needs UEFI firmware for QEMU (ovmf).

KERNEL_VERSION ?= 6.18.64
JOBS           ?= $(shell nproc 2>/dev/null || sysctl -n hw.ncpu)
DEBUG          ?= 0

SAULE   ?= $(shell command -v saule)
BUSYBOX ?= $(shell command -v busybox)
QEMU    ?= qemu-system-x86_64
# The session: the window manager when ../saule-wm is there, the hello-world
# in session/ otherwise. APP=… boots any other project.
WM      ?= ../saule-wm
APP     ?= $(if $(wildcard $(WM)/saule.config),$(WM),session)
# Saule projects to build into the image as /bin/apps/<name>: the apps the
# launcher opens. By default every app in ../saule-linux-apps, the ones that
# ship with the system (one project per folder); APPS="…" for others.
APPS_REPO ?= ../saule-linux-apps
APPS    ?= $(patsubst %/saule.config,%,$(wildcard $(APPS_REPO)/*/saule.config))

# Where cargo leaves a crate's build: the crate's own target/, or
# CARGO_TARGET_DIR when that is set, which every crate then shares. The
# container sets it (scripts/docker.sh); a path into a crate's own folder
# there names a file that was never made, and the program it names was left
# out of the image without a word (saule-system, on every Mac-built image).
cargo_out = $(or $(CARGO_TARGET_DIR),$(1)/target)

BUILD     := build
KSRC      := $(BUILD)/linux-$(KERNEL_VERSION)
KOUT      := $(abspath $(BUILD)/kernel)
KCONFIG   := $(abspath config/kernel.config)
BZIMAGE   := $(KOUT)/arch/x86/boot/bzImage
INIT      := $(call cargo_out,init)/x86_64-unknown-linux-musl/release/saule-init
GEN_CPIO  := $(BUILD)/tools/gen_init_cpio
INITRAMFS := $(BUILD)/initramfs.cpio.gz
SESSION   := $(BUILD)/session
APPS_OUT  := $(BUILD)/apps
TARBALL   := $(BUILD)/downloads/linux-$(KERNEL_VERSION).tar.xz

# The installer: its window (installer/, a Saule project) and the program
# that does its writing (install/, Rust): for the netbook, static, and for
# this machine, where it makes the stick's image.
INSTALLER    := $(BUILD)/installer
INSTALL_TOOL := $(call cargo_out,install)/x86_64-unknown-linux-musl/release/saule-install
INSTALL_HOST := $(call cargo_out,install)/release/saule-install
# The system's version: the day and minute it was built, unless said.
# What an update file says it is, and what Settings says is running. Not
# called VERSION: that is the kernel's own, and a variable given on the
# command line reaches the kernel's build too.
SYSTEM_VERSION ?= $(shell date +%Y.%m.%d.%H%M)
# The key update files are signed with (`make update-key` makes one; its
# public half is config/update.pub, built into saule-install), and the
# update file `make update` makes.
UPDATE_KEY ?= keys/update.key

# The USB stick: what goes on it is put together in build/usb, and the stick
# itself is one file, USB_IMAGE.
USB       := $(BUILD)/usb
USB_IMAGE ?= $(BUILD)/saule-linux.img
# The kernel options the stick starts with and hands on to the system it
# installs.
BOOT_OPTIONS ?= quiet
# More for the stick alone, not for what it installs. While the system is
# being brought up on real machines, the stick's own session notes where
# its frames spend their time and which way they are shown (Shine's
# traces, a line every two seconds for each program): a report made
# from it (Mod+Shift+R) then says what is slow.
STICK_OPTIONS ?= saule.env.SHINE_TRACE_FRAMES=1 saule.env.SHINE_TRACE_PRESENT=1
# The graphics driver packs the stick carries: every one that is built
# (`make drivers DRIVERS=gen3`). The installer gives a machine the one its
# chip needs (config/drivers.list).
PACKS ?= $(filter-out libdrm,$(notdir $(patsubst %/usr,%,$(wildcard $(BUILD)/drivers/*/usr))))
KERNEL_URL := https://cdn.kernel.org/pub/linux/kernel/v$(firstword $(subst ., ,$(KERNEL_VERSION))).x/linux-$(KERNEL_VERSION).tar.xz

ifeq ($(DEBUG),1)
  INITRAMFS_BUSYBOX := $(BUSYBOX)
endif

# The usual command-line tools — BusyBox's sh, ls, cat, vi, … — in /usr/bin,
# for a terminal app to run. Not /bin/sh: a /bin/sh is what turns init's
# fallback into a debug shell, and that stays DEBUG=1's. TOOLS=0 leaves
# them out.
TOOLS ?= 1
ifeq ($(TOOLS),1)
  INITRAMFS_TOOLS := $(BUSYBOX)
endif

# What init makes and checks the data disk's file system with
# (init/src/storage.rs): this machine's mke2fs and e2fsck (e2fsprogs), copied
# into the image with the libraries they load. Without them an empty disk
# is formatted by BusyBox's mke2fs (TOOLS=1) as ext2, with no journal and
# nothing to mend it after a power cut; with neither, not at all.
FS_TOOLS ?= $(foreach tool,mke2fs e2fsck,$(shell command -v $(tool)))

# Native Saule packages the session may bundle (Shine, …): the image needs
# the shared libraries they load. And the fonts Shine looks for.
SAULE_HOME  ?= $(HOME)/.saule
NATIVE_LIBS ?= $(wildcard $(SAULE_HOME)/native_packages/*.so)
# The graphics driver pack put in the image (scripts/build-drivers.sh; `make
# drivers` builds it): `virtual` is QEMU's GPU, `gen3` the netbook's. An
# image without one draws on the CPU, as it always could.
DRIVERS     ?= virtual
DRIVERS_DIR := $(wildcard $(BUILD)/drivers/$(DRIVERS))
# What the session asks the machine for (system/, Rust): sleep, the volume,
# the network. Init keeps it running.
SYSTEM := $(call cargo_out,system)/x86_64-unknown-linux-musl/release/saule-system
# Wi-Fi: wpa_supplicant and the radios' firmware, gathered by
# scripts/fetch-wifi.sh into build/wifi. WIFI=0 leaves them out: 18 MB that
# a machine with no radio, or one whose radio needs no firmware, has no use
# for.
WIFI     ?= 1
WIFI_DIR := $(if $(filter 1,$(WIFI)),$(BUILD)/wifi)
# What Intel's graphics chips load when they start
# (scripts/fetch-graphics-firmware.sh), and the list PCI devices are
# named from (the pci.ids package), of which the graphics chips' makers
# are kept.
GRAPHICS_FIRMWARE := $(BUILD)/graphics-firmware
PCI_IDS ?= $(wildcard /usr/share/misc/pci.ids)
# Bluetooth: BlueZ, a D-Bus daemon for it and Intel's radios' firmware,
# gathered by scripts/fetch-bluetooth.sh into build/bluetooth.
# BLUETOOTH=0 leaves them out.
BLUETOOTH     ?= 1
BLUETOOTH_DIR := $(if $(filter 1,$(BLUETOOTH)),$(BUILD)/bluetooth)
# The sound library's configuration (the alsa-lib package's data).
ALSA     ?= $(wildcard /usr/share/alsa)
# The time zone database (the tzdata package): init tells a zone's time by
# it, and the first-start setup lists its zones.
ZONEINFO    ?= $(wildcard /usr/share/zoneinfo)
# Keyboard layouts (the xkb-data package), with the host's libxkbcommon to
# read them: what is typed on a keyboard that isn't a US one.
XKB         ?= $(wildcard /usr/share/X11/xkb)
FONTS       ?= $(wildcard /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf \
                          /usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf)
# Where each font's common part is made (scripts/common-fonts.sh): the
# few hundred characters most text is made of, which is all a program
# reads of a font as it starts.
FONTS_COMMON ?= $(BUILD)/fonts

# KVM when this user may use it (members of the `kvm` group may), plain
# emulation otherwise.
QEMU_ACCEL := $(shell [ -w /dev/kvm ] && echo "-enable-kvm -cpu host")
QEMU_FLAGS ?= -m 2G -smp 2 -no-reboot $(QEMU_ACCEL) \
              -kernel $(BZIMAGE) -initrd $(INITRAMFS)
APPEND     ?=

# The window `make run-gui` opens. Empty: QEMU's own choice.
QEMU_DISPLAY ?=

# `make run-vnc`: the screen served over VNC, on this machine only, behind
# this password (a Mac's Screen Sharing refuses a server without one).
VNC_PASSWORD ?= saule
VNC_OPEN     ?=

# On a Mac nothing here can be built natively (it needs a Linux toolchain and
# an x86_64 Linux saule), so the build targets run in an x86_64 Ubuntu
# container (scripts/docker.sh, Docker needed) and QEMU runs on the Mac, on
# the kernel and initramfs the container copies to out/.
HOST_OS := $(shell uname -s)
ifeq ($(HOST_OS),Darwin)
  BZIMAGE   := out/bzImage
  INITRAMFS := out/initramfs.cpio.gz
  QEMU_FLAGS = -m 2G -smp 2 -no-reboot -kernel $(BZIMAGE) -initrd $(INITRAMFS)
  # Full grab, so that Cmd+Space and the other shortcuts macOS would take
  # reach the window manager inside. Not zoom-to-fit: QEMU 11's scaled
  # window keeps rescaling itself, and everything on screen shakes. For a
  # bigger picture, `make run-vnc` and Screen Sharing.
  QEMU_DISPLAY = -display cocoa,full-grab=on
  VNC_OPEN = open vnc://127.0.0.1:5901
endif

.PHONY: all kernel init session apps drivers initramfs test
.PHONY: installer usb-image usb run-usb run-disk
.PHONY: run run-gui run-vnc run-netbook run-vesa
.PHONY: menuconfig check-config clean distclean

ifeq ($(HOST_OS),Darwin)

# APP must be inside this repo or ../saule-wm: only those, ../saule and
# ../saule-shine reach the container, and the repositories of the apps in
# APPS beside the repo (`../saule-linux-apps` for
# `../saule-linux-apps/settings`), which scripts/docker.sh mounts.
#
# The stick (usb-image, and run-usb's), the installer, the driver packs and
# update files are made there too, and what QEMU or a person needs of them
# is copied to out/ beside the kernel: out/saule-linux.img and the update
# file. Writing a stick (`make usb`) is the Mac's own business: see `usb`
# below.
#
# Those targets' settings are passed on only when given here (`make
# usb-image BOOT_OPTIONS=…`), so the container's own defaults stand
# otherwise: PACKS, for one, is what the container's build/drivers holds.
FORWARD := USB_IMAGE BOOT_OPTIONS STICK_OPTIONS DRIVERS PACKS UPDATE_KEY WIFI BLUETOOTH
FORWARDED = $(foreach v,$(FORWARD),$(if $(filter command line,$(origin $(v))),$(v)="$($(v))"))

all kernel init session initramfs test menuconfig check-config \
installer usb-image update update-key drivers:
	scripts/docker.sh make $@ DEBUG=$(DEBUG) TOOLS=$(TOOLS) APP=$(APP) APPS="$(APPS)" KERNEL_VERSION=$(KERNEL_VERSION) $(FORWARDED)

# No stick is written from the container, which sees none. On a Mac,
# balenaEtcher writes out/saule-linux.img, or dd to the stick's raw device.
usb:
	@echo "On a Mac, write out/saule-linux.img (make usb-image) with balenaEtcher, or:" >&2
	@echo "  diskutil list                      # the stick is /dev/diskN" >&2
	@echo "  diskutil unmountDisk /dev/diskN" >&2
	@echo "  sudo dd if=out/saule-linux.img of=/dev/rdiskN bs=4m   # erases the stick" >&2
	@exit 1

clean distclean:
	scripts/docker.sh make $@
	rm -rf out

else

all: kernel initramfs

# ---------------------------------------------------------------------------
# Kernel
# ---------------------------------------------------------------------------

$(TARBALL):
	mkdir -p $(dir $@)
	curl -fL --progress-bar -o $@.part $(KERNEL_URL)
	mv $@.part $@

$(KSRC)/Makefile: $(TARBALL)
	tar -C $(BUILD) -xf $<
	touch $@

# Everything off, then config/kernel.config on top.
$(KOUT)/.config: $(KCONFIG) $(KSRC)/Makefile
	mkdir -p $(KOUT)
	$(MAKE) -C $(KSRC) O=$(KOUT) KCONFIG_ALLCONFIG=$(KCONFIG) allnoconfig
	@scripts/check-config.sh $(KCONFIG) $@

kernel: $(KOUT)/.config
	$(MAKE) -C $(KSRC) O=$(KOUT) -j$(JOBS) bzImage
	@echo "kernel: $(BZIMAGE) ($$(du -h $(BZIMAGE) | cut -f1))"

menuconfig: $(KOUT)/.config
	$(MAKE) -C $(KSRC) O=$(KOUT) menuconfig

check-config: $(KOUT)/.config
	@scripts/check-config.sh $(KCONFIG) $<

# ---------------------------------------------------------------------------
# Init and the initramfs
# ---------------------------------------------------------------------------

init:
	cd init && cargo build --release
	cd system && cargo build --release --target x86_64-unknown-linux-musl
	$(if $(WIFI_DIR),scripts/fetch-wifi.sh $(WIFI_DIR))
	$(if $(BLUETOOTH_DIR),scripts/fetch-bluetooth.sh $(BLUETOOTH_DIR))
	scripts/fetch-graphics-firmware.sh $(GRAPHICS_FIRMWARE)
	$(if $(FONTS),scripts/common-fonts.sh $(FONTS_COMMON) $(FONTS))

test:
	cd init && cargo test
	cd install && cargo test
	cd system && cargo test
	cd password && cargo test

$(GEN_CPIO): $(KSRC)/Makefile
	mkdir -p $(dir $@)
	$(CC) -O2 -o $@ $(KSRC)/usr/gen_init_cpio.c

# The session as one executable: bytecode, native packages and bundled files
# packed into a copy of the saule runtime. Each app in APPS is built the same
# way into /bin/apps, named after its project's `name:` (what the launcher
# lists) rather than its folder: an app called "settings" is
# /bin/apps/settings whatever its folder is called.
session:
	@test -n "$(SAULE)" || { echo "saule not found: install it or pass SAULE=/path/to/saule"; exit 1; }
	$(SAULE) build $(APP) -o $(SESSION)

# The graphics driver pack DRIVERS names, into build/drivers/<pack>: Mesa and
# libdrm, built from source (needs meson, ninja and python3-mako). Once is
# enough; the image takes it from there.
drivers:
	scripts/build-drivers.sh $(DRIVERS)

# The apps alone, into build/apps: also what the window manager, nested on a
# desktop, opens with `SAULE_APPS=../saule-linux/build/apps saule run .`.
apps:
	@test -n "$(SAULE)" || { echo "saule not found: install it or pass SAULE=/path/to/saule"; exit 1; }
	rm -rf $(APPS_OUT)
	mkdir -p $(APPS_OUT)
	for app in $(APPS); do \
		name=$$(sed -n 's/^name: *"\(.*\)".*/\1/p' $$app/saule.config); \
		$(SAULE) build $$app -o $(APPS_OUT)/$${name:-$$(basename $$app)} || exit 1; \
	done

initramfs: init session apps $(GEN_CPIO)
	@test "$(DEBUG)" != 1 -o -n "$(BUSYBOX)" || { echo "DEBUG=1 needs busybox (apt install busybox-static) or BUSYBOX=/path"; exit 1; }
	@test "$(TOOLS)" != 1 -o -n "$(BUSYBOX)" || { echo "TOOLS=1 needs busybox (apt install busybox-static) or BUSYBOX=/path; TOOLS=0 builds without"; exit 1; }
	STAGE=$(BUILD)/rootfs GEN_INIT_CPIO=$(GEN_CPIO) INIT=$(INIT) SESSION=$(SESSION) \
	ROOTFS=rootfs BUSYBOX=$(INITRAMFS_BUSYBOX) TOOLS_BUSYBOX=$(INITRAMFS_TOOLS) APPS_DIR=$(APPS_OUT) \
	NATIVE_LIBS="$(NATIVE_LIBS)" FONTS="$(FONTS)" FONTS_COMMON="$(FONTS_COMMON)" DRIVERS_DIR="$(DRIVERS_DIR)" \
	FS_TOOLS="$(FS_TOOLS)" ZONEINFO="$(ZONEINFO)" \
	SYSTEM="$(SYSTEM)" WIFI="$(WIFI_DIR)" ALSA="$(ALSA)" XKB="$(XKB)" \
	GRAPHICS_FIRMWARE="$(GRAPHICS_FIRMWARE)" PCI_IDS="$(PCI_IDS)" \
	BLUETOOTH="$(BLUETOOTH_DIR)" INSTALL_TOOL="$(INSTALL_TOOL)" VERSION="$(SYSTEM_VERSION)" \
	WALLPAPERS="$(wildcard $(WM)/wallpapers)" scripts/mkinitramfs.sh $(INITRAMFS)

# ---------------------------------------------------------------------------
# The installer and the USB stick
# ---------------------------------------------------------------------------

installer:
	@test -n "$(SAULE)" || { echo "saule not found: install it or pass SAULE=/path/to/saule"; exit 1; }
	$(SAULE) build installer -o $(INSTALLER)
	cd install && cargo build --release --target x86_64-unknown-linux-musl
	cd install && cargo build --release

# The stick, as one file. On it, in an ext2 file system GRUB reads:
#
#   vmlinuz, initramfs.cpio.gz   the system, with no graphics driver pack
#   installer.cpio.gz            the installer, unpacked over the system when
#                                the stick starts: an installed system does
#                                not carry it
#   drivers/<pack>.cpio.gz       every pack built, of which a machine takes one
#   drivers.list                 which chip takes which
#   cmdline                      the kernel options an installed system gets
#   grub/                        GRUB for a BIOS, and GRUB's configuration
#                                for the stick
#
# and after it a small FAT partition with GRUB for UEFI, which a PC without
# a BIOS starts from. install/src/layout.rs says where things are on the
# disk; nothing here needs root.
usb-image: kernel installer $(GEN_CPIO)
	rm -rf $(USB)
	mkdir -p $(USB)/root/grub $(USB)/root/drivers
	$(MAKE) initramfs INITRAMFS=$(USB)/root/initramfs.cpio.gz DRIVERS=none
	cp $(BZIMAGE) $(USB)/root/vmlinuz
	scripts/mkgrub.sh $(USB)/root/grub $(USB)/efi.img
	STAGE=$(USB)/stage GEN_INIT_CPIO=$(GEN_CPIO) scripts/mkoverlay.sh $(USB)/root/installer.cpio.gz \
		--program $(INSTALLER)=/bin/installer --program $(INSTALL_TOOL)=/sbin/saule-install
	for pack in $(PACKS); do \
		STAGE=$(USB)/stage GEN_INIT_CPIO=$(GEN_CPIO) scripts/mkoverlay.sh \
			$(USB)/root/drivers/$$pack.cpio.gz --drivers $(BUILD)/drivers/$$pack || exit 1; \
	done
	cp config/drivers.list $(USB)/root/drivers.list
	echo "$(BOOT_OPTIONS)" > $(USB)/root/cmdline
	$(INSTALL_HOST) config "$(BOOT_OPTIONS) $(STICK_OPTIONS) saule.installer=1" /installer.cpio.gz > $(USB)/root/grub/grub.cfg
	$(INSTALL_HOST) image $(USB_IMAGE) $(USB)/root --efi $(USB)/efi.img

# An update file for systems already installed: the same kernel,
# initramfs and driver packs as the stick, signed. Settings finds it on a
# drive that is plugged in or in Downloads, and installs it. Named for
# the version the image inside it says it is.
update: usb-image
	@test -f $(UPDATE_KEY) || { echo "no signing key at $(UPDATE_KEY): make update-key makes one"; exit 1; }
	scripts/mkupdate.sh $(INSTALL_HOST) $(USB)/root $(UPDATE_KEY) $(BUILD)

update-key:
	cd install && cargo build --release
	$(INSTALL_HOST) keygen $(UPDATE_KEY) config/update.pub
	@echo "the private key is $(UPDATE_KEY): keep a copy somewhere safe, it is not in the repository"

# Write the image to a stick. The whole device (/dev/sdb, not /dev/sdb1),
# and as root; WSL does not see USB sticks at all, so from Windows write
# build/saule-linux.img with Rufus (DD mode) or balenaEtcher.
usb: usb-image
	@test -n "$(DEVICE)" || { echo "usage: make usb DEVICE=/dev/sdX   (the whole stick: everything on it is erased)"; exit 1; }
	@test -b "$(DEVICE)" || { echo "$(DEVICE) is not a disk"; exit 1; }
	dd if=$(USB_IMAGE) of=$(DEVICE) bs=4M conv=fsync status=progress

endif  # not Darwin

# ---------------------------------------------------------------------------
# Running
# ---------------------------------------------------------------------------

run: all
	$(QEMU) $(QEMU_FLAGS) -nographic -append "console=ttyS0 $(APPEND)"

# The last console= is the one /dev/console — and so the session — uses.
run-gui: all
	$(QEMU) $(QEMU_FLAGS) $(QEMU_DISPLAY) -vga std -serial stdio -append "console=ttyS0 console=tty1 $(APPEND)"

# The same screen as run-gui, served over VNC on 127.0.0.1:5901 for a viewer
# that scales it smoothly to its window: on a Mac, Screen Sharing, which this
# opens once QEMU is listening. The Mac keeps Cmd+Space there, so the
# launcher is Cmd+D.
run-vnc: all
	@echo "VNC on 127.0.0.1:5901, password: $(VNC_PASSWORD)"
	@$(if $(VNC_OPEN),(sleep 2 && $(VNC_OPEN)) &,)
	$(QEMU) $(QEMU_FLAGS) -vga std -display none \
		-object secret,id=vncpass,data=$(VNC_PASSWORD) \
		-vnc 127.0.0.1:1,password-secret=vncpass \
		-serial stdio -append "console=ttyS0 console=tty1 $(APPEND)"

# QEMU dressed as the HP Mini 210-1001sa (Atom N450, 1 GB, SeaBIOS, 1024x600
# VGA, EHCI, PS/2): see scripts/qemu-netbook.sh. Loads the kernel directly,
# with no bootloader; `make run-usb` starts it from a stick instead.
#
#   SLOW=1     about the netbook's speed (KVM capped at 8% of a core);
#   SLOW=12    the same with another percentage, while calibrating
#   MEMORY=2G  RAM, for a netbook with its memory upgraded
#   GL=1       the virtual GPU with 3D, for drawing on the GPU (the
#              `virtual` driver pack)
#   NET=0      no network card: the netbook's port has a cable in
#              otherwise (QEMU's Intel card, the nearest to its Atheros)
#   STICK=f    a USB stick plugged in, with disk image f as its contents
#
# Its 250 GB hard disk is disks/netbook.qcow2, made empty on the first run
# and kept from then on: what the system saves there outlives a reboot.
SLOW   ?=
MEMORY ?= 1G
GL     ?=
NETBOOK_SLOW := $(if $(filter 1 yes,$(SLOW)),--slow,$(if $(SLOW),--slow=$(SLOW)))
NET    ?= 1
STICK  ?=
NETBOOK_GL   := $(if $(filter 1 yes,$(GL)),--gl) $(if $(filter 1 yes,$(NET)),--net) \
                $(if $(STICK),--stick $(STICK))
run-netbook: all
	QEMU=$(QEMU) scripts/qemu-netbook.sh --kernel $(BZIMAGE) --initrd $(INITRAMFS) \
		--memory $(MEMORY) $(NETBOOK_SLOW) $(NETBOOK_GL) --append "$(APPEND)"

# The QEMU netbook started as the real one will be: from a USB stick,
# through the BIOS and GRUB, into the installer. The stick is one made for
# QEMU, since its kernel options ride on the stick: the serial console on
# this terminal, and the panel's size pinned (see scripts/qemu-netbook.sh).
#
# What the installer installs on is the netbook's own hard disk,
# disks/netbook.qcow2, so its "Erase" erases what was saved there.
# DISK=some/other.qcow2 for a disk to practise on. QEMU can't take the stick
# out, so after installing, shut down and `make run-disk`.
DISK ?= disks/netbook.qcow2
QEMU_USB_IMAGE    := $(BUILD)/saule-linux-qemu.img
# Where QEMU finds it: on a Mac, the copy scripts/docker.sh makes in out/.
QEMU_USB_FILE     := $(if $(filter Darwin,$(HOST_OS)),out/$(notdir $(QEMU_USB_IMAGE)),$(QEMU_USB_IMAGE))
QEMU_BOOT_OPTIONS := console=ttyS0 console=tty1 video=Virtual-1:1024x600 \
                     $(if $(WSL_DISTRO_NAME),saule.env.SAULE_WM_KEY=ctrl+alt) $(APPEND)
EFI ?=
NETBOOK_EFI := $(if $(filter 1 yes,$(EFI)),--efi)
run-usb:
	$(MAKE) usb-image USB_IMAGE=$(QEMU_USB_IMAGE) BOOT_OPTIONS="$(strip $(QEMU_BOOT_OPTIONS))"
	QEMU=$(QEMU) scripts/qemu-netbook.sh --usb $(QEMU_USB_FILE) --disk $(DISK) \
		--memory $(MEMORY) $(NETBOOK_SLOW) $(NETBOOK_GL) $(NETBOOK_EFI)

# The QEMU netbook started from its hard disk alone, as it starts once
# Saule Linux is installed on it.
run-disk:
	QEMU=$(QEMU) scripts/qemu-netbook.sh --disk $(DISK) --memory $(MEMORY) $(NETBOOK_SLOW) $(NETBOOK_GL) $(NETBOOK_EFI)

# The netbook with no graphics driver: the screen the BIOS set up, drawn
# through simpledrm. What it would show if i915 failed on its chip.
run-vesa: all
	QEMU=$(QEMU) scripts/qemu-netbook.sh --kernel $(BZIMAGE) --initrd $(INITRAMFS) \
		--memory $(MEMORY) $(NETBOOK_SLOW) --vesa --append "$(APPEND)"

# ---------------------------------------------------------------------------

ifneq ($(HOST_OS),Darwin)
clean:
	rm -rf $(KOUT) $(BUILD)/rootfs $(BUILD)/tools $(INITRAMFS) $(SESSION)
	rm -rf $(USB) $(INSTALLER) $(BUILD)/saule-linux.img $(BUILD)/saule-linux-qemu.img
	cd init && cargo clean
	cd install && cargo clean
	cd system && cargo clean

distclean:
	rm -rf $(BUILD)
	cd init && cargo clean
	cd install && cargo clean
	cd system && cargo clean
endif

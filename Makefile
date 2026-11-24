# ============================================================================
# Saule Linux
# ============================================================================
#
#   make              build the kernel and the initramfs
#   make run          boot it in QEMU, console on this terminal (Ctrl-A X quits)
#   make run-gui      boot it in a QEMU window (the screen the display server
#                     will draw on)
#   make run-netbook  boot it in QEMU dressed as the HP Mini 210-1001sa
#                     (SLOW=1: at about its speed)
#   make DEBUG=1 run  include BusyBox: a shell when the session ends
#
#   make menuconfig   change kernel options (copy keepers to config/kernel.config)
#   make check-config list options config/kernel.config asked for but didn't get
#   make clean        remove what was built, keep the kernel source
#   make distclean    remove build/ entirely
#
# Host packages (Ubuntu): build-essential flex bison bc libelf-dev libssl-dev
# cpio qemu-system-x86, plus `rustup target add x86_64-unknown-linux-musl`.
# DEBUG=1 also needs busybox-static.

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
# Further Saule projects to build into the image as /bin/apps/<name>, for the
# session to start — the apps under a display server.
APPS    ?=

BUILD     := build
KSRC      := $(BUILD)/linux-$(KERNEL_VERSION)
KOUT      := $(abspath $(BUILD)/kernel)
KCONFIG   := $(abspath config/kernel.config)
BZIMAGE   := $(KOUT)/arch/x86/boot/bzImage
INIT      := init/target/x86_64-unknown-linux-musl/release/saule-init
GEN_CPIO  := $(BUILD)/tools/gen_init_cpio
INITRAMFS := $(BUILD)/initramfs.cpio.gz
SESSION   := $(BUILD)/session
APPS_OUT  := $(BUILD)/apps
TARBALL   := $(BUILD)/downloads/linux-$(KERNEL_VERSION).tar.xz
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

# Native Saule packages the session may bundle (Shine, …): the image needs
# the shared libraries they load. And the fonts Shine looks for.
SAULE_HOME  ?= $(HOME)/.saule
NATIVE_LIBS ?= $(wildcard $(SAULE_HOME)/native_packages/*.so)
FONTS       ?= $(wildcard /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf \
                          /usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf)

# KVM when this user may use it (add yourself to the `kvm` group), plain
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

.PHONY: all kernel init session initramfs run run-gui run-vnc run-netbook menuconfig check-config clean distclean

ifeq ($(HOST_OS),Darwin)

# APP must be inside this repo or ../saule-wm: only those, ../saule and
# ../saule-shine reach the container, and the apps in APPS beside the repo
# (`../saule-settings`), which scripts/docker.sh mounts.
all kernel init session initramfs menuconfig check-config:
	scripts/docker.sh make $@ DEBUG=$(DEBUG) TOOLS=$(TOOLS) APP=$(APP) APPS="$(APPS)" KERNEL_VERSION=$(KERNEL_VERSION)

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

$(GEN_CPIO): $(KSRC)/Makefile
	mkdir -p $(dir $@)
	$(CC) -O2 -o $@ $(KSRC)/usr/gen_init_cpio.c

# The session as one executable: bytecode, native packages and bundled files
# packed into a copy of the saule runtime. Each app in APPS is built the same
# way into /bin/apps, named after its project's `name:` (what the launcher
# lists) rather than its folder: ../saule-settings is "settings".
session:
	@test -n "$(SAULE)" || { echo "saule not found: install it or pass SAULE=/path/to/saule"; exit 1; }
	$(SAULE) build $(APP) -o $(SESSION)
	rm -rf $(APPS_OUT)
	mkdir -p $(APPS_OUT)
	for app in $(APPS); do \
		name=$$(sed -n 's/^name: *"\(.*\)".*/\1/p' $$app/saule.config); \
		$(SAULE) build $$app -o $(APPS_OUT)/$${name:-$$(basename $$app)} || exit 1; \
	done

initramfs: init session $(GEN_CPIO)
	@test "$(DEBUG)" != 1 -o -n "$(BUSYBOX)" || { echo "DEBUG=1 needs busybox (apt install busybox-static) or BUSYBOX=/path"; exit 1; }
	@test "$(TOOLS)" != 1 -o -n "$(BUSYBOX)" || { echo "TOOLS=1 needs busybox (apt install busybox-static) or BUSYBOX=/path; TOOLS=0 builds without"; exit 1; }
	STAGE=$(BUILD)/rootfs GEN_INIT_CPIO=$(GEN_CPIO) INIT=$(INIT) SESSION=$(SESSION) \
	ROOTFS=rootfs BUSYBOX=$(INITRAMFS_BUSYBOX) TOOLS_BUSYBOX=$(INITRAMFS_TOOLS) APPS_DIR=$(APPS_OUT) \
	NATIVE_LIBS="$(NATIVE_LIBS)" FONTS="$(FONTS)" \
	scripts/mkinitramfs.sh $(INITRAMFS)

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
# VGA, EHCI, PS/2): see scripts/qemu-netbook.sh. Loads the kernel directly
# until there is a USB stick image; then `scripts/qemu-netbook.sh --usb IMAGE`.
#
#   SLOW=1   about the netbook's speed (KVM capped at 8% of a core);
#   SLOW=12  the same with another percentage, while calibrating
#   MEMORY=2G  RAM, for a netbook with its memory upgraded
SLOW   ?=
MEMORY ?= 1G
NETBOOK_SLOW := $(if $(filter 1 yes,$(SLOW)),--slow,$(if $(SLOW),--slow=$(SLOW)))
run-netbook: all
	QEMU=$(QEMU) scripts/qemu-netbook.sh --kernel $(BZIMAGE) --initrd $(INITRAMFS) \
		--memory $(MEMORY) $(NETBOOK_SLOW) --append "$(APPEND)"

# ---------------------------------------------------------------------------

ifneq ($(HOST_OS),Darwin)
clean:
	rm -rf $(KOUT) $(BUILD)/rootfs $(BUILD)/tools $(INITRAMFS) $(SESSION)
	cd init && cargo clean

distclean:
	rm -rf $(BUILD)
	cd init && cargo clean
endif

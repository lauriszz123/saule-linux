# ============================================================================
# Saule Linux
# ============================================================================
#
#   make              build the kernel and the initramfs
#   make run          boot it in QEMU, console on this terminal (Ctrl-A X quits)
#   make run-gui      boot it in a QEMU window (the screen the display server
#                     will draw on)
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
JOBS           ?= $(shell nproc)
DEBUG          ?= 0

SAULE   ?= $(shell command -v saule)
BUSYBOX ?= $(shell command -v busybox)
QEMU    ?= qemu-system-x86_64
APP     ?= session

BUILD     := build
KSRC      := $(BUILD)/linux-$(KERNEL_VERSION)
KOUT      := $(abspath $(BUILD)/kernel)
KCONFIG   := $(abspath config/kernel.config)
BZIMAGE   := $(KOUT)/arch/x86/boot/bzImage
INIT      := init/target/x86_64-unknown-linux-musl/release/saule-init
GEN_CPIO  := $(BUILD)/tools/gen_init_cpio
INITRAMFS := $(BUILD)/initramfs.cpio.gz
SESSION   := $(BUILD)/session
TARBALL   := $(BUILD)/downloads/linux-$(KERNEL_VERSION).tar.xz
KERNEL_URL := https://cdn.kernel.org/pub/linux/kernel/v$(firstword $(subst ., ,$(KERNEL_VERSION))).x/linux-$(KERNEL_VERSION).tar.xz

ifeq ($(DEBUG),1)
  INITRAMFS_BUSYBOX := $(BUSYBOX)
endif

# KVM when this user may use it (add yourself to the `kvm` group), plain
# emulation otherwise.
QEMU_ACCEL := $(shell [ -w /dev/kvm ] && echo "-enable-kvm -cpu host")
QEMU_FLAGS ?= -m 2G -smp 2 -no-reboot $(QEMU_ACCEL) \
              -kernel $(BZIMAGE) -initrd $(INITRAMFS)
APPEND     ?=

.PHONY: all kernel init session initramfs run run-gui menuconfig check-config clean distclean

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
# packed into a copy of the saule runtime.
session:
	@test -n "$(SAULE)" || { echo "saule not found: install it or pass SAULE=/path/to/saule"; exit 1; }
	$(SAULE) build $(APP) -o $(SESSION)

initramfs: init session $(GEN_CPIO)
	@test "$(DEBUG)" != 1 -o -n "$(BUSYBOX)" || { echo "DEBUG=1 needs busybox (apt install busybox-static) or BUSYBOX=/path"; exit 1; }
	STAGE=$(BUILD)/rootfs GEN_INIT_CPIO=$(GEN_CPIO) INIT=$(INIT) SESSION=$(SESSION) \
	ROOTFS=rootfs BUSYBOX=$(INITRAMFS_BUSYBOX) \
	scripts/mkinitramfs.sh $(INITRAMFS)

# ---------------------------------------------------------------------------
# Running
# ---------------------------------------------------------------------------

run: all
	$(QEMU) $(QEMU_FLAGS) -nographic -append "console=ttyS0 $(APPEND)"

# The last console= is the one /dev/console — and so the session — uses.
run-gui: all
	$(QEMU) $(QEMU_FLAGS) -vga std -serial stdio -append "console=ttyS0 console=tty1 $(APPEND)"

# ---------------------------------------------------------------------------

clean:
	rm -rf $(KOUT) $(BUILD)/rootfs $(BUILD)/tools $(INITRAMFS) $(SESSION)
	cd init && cargo clean

distclean:
	rm -rf $(BUILD)
	cd init && cargo clean

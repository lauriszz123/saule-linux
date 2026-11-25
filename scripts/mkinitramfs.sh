#!/usr/bin/env bash
# ============================================================================
# mkinitramfs.sh — assemble the root file system and pack it as an initramfs
# ============================================================================
#
# Usage: mkinitramfs.sh OUTPUT.cpio.gz
#
# Settings come from the environment (the Makefile passes them):
#
#   STAGE          directory to assemble the tree in (wiped first)
#   GEN_INIT_CPIO  the kernel's cpio packer (usr/gen_init_cpio)
#   INIT           the init binary — becomes /init
#   SESSION        the session, built with `saule build` — becomes
#                  /bin/session
#   ROOTFS         files copied over the tree as they are (etc/, …)
#   BUSYBOX        optional: a BusyBox binary — adds /bin/sh and the usual
#                  tools (debug images)
#   TOOLS_BUSYBOX  optional: a BusyBox binary — the usual tools in /usr/bin,
#                  sh included, for programs to run (a terminal's shell).
#                  Unlike BUSYBOX it adds no /bin/sh, so init's fallback
#                  stays as it is
#   NATIVE_LIBS    optional: native Saule packages (.so) the session may
#                  load. They travel inside the session; what goes in the
#                  tree is the shared libraries *they* need.
#   FS_TOOLS       optional: mke2fs and e2fsck (e2fsprogs), installed in
#                  /sbin: what init makes and checks the data disk with
#   APPS_DIR       optional: a directory of apps built with `saule build`,
#                  installed as /bin/apps/<name>
#   DRIVERS_DIR    optional: a graphics driver pack (scripts/build-drivers.sh,
#                  build/drivers/<pack>): Mesa and libdrm, installed over any
#                  copies of the same libraries brought in for programs.
#                  A stick carries the packs beside the system instead
#                  (mkoverlay.sh), and the installer takes the machine's
#   FONTS          optional: font files, installed at the same path as on
#                  the build host (where Shine looks for them)
#   WALLPAPERS     optional: a directory of wallpaper photographs (the
#                  window manager's wallpapers/), installed in
#                  /usr/share/saule/wallpapers
#
# Dynamically linked binaries bring the libraries `ldd` names for them.
# The tree is packed with root as the owner of every file (stage.sh).

set -euo pipefail

output=${1:?usage: mkinitramfs.sh OUTPUT.cpio.gz}

: "${STAGE:?}" "${GEN_INIT_CPIO:?}" "${INIT:?}" "${SESSION:?}" "${ROOTFS:?}"
BUSYBOX=${BUSYBOX:-}
TOOLS_BUSYBOX=${TOOLS_BUSYBOX:-}
NATIVE_LIBS=${NATIVE_LIBS:-}
FS_TOOLS=${FS_TOOLS:-}
FONTS=${FONTS:-}
DRIVERS_DIR=${DRIVERS_DIR:-}
APPS_DIR=${APPS_DIR:-}

# Absolute paths: the packing step below runs from inside the tree.
mkdir -p "$STAGE" "$(dirname "$output")"
output=$(realpath "$output")
STAGE=$(realpath "$STAGE")
GEN_INIT_CPIO=$(realpath "$GEN_INIT_CPIO")

rm -rf "$STAGE"
mkdir -p "$STAGE"/{bin,dev,etc,proc,sys,run,tmp,root}

# add_libraries, add_program, add_drivers, pack_tree.
. "$(dirname "$0")/stage.sh"

add_program "$INIT" /init
add_program "$SESSION" /bin/session

if [ -n "$APPS_DIR" ] && [ -d "$APPS_DIR" ]; then
    for app in "$APPS_DIR"/*; do
        if [ -f "$app" ]; then
            add_program "$app" "/bin/apps/$(basename "$app")"
        fi
    done
fi

for library in $NATIVE_LIBS; do
    add_libraries "$library"
done

# What init makes and checks the data disk's file system with.
for tool in $FS_TOOLS; do
    add_program "$tool" "/sbin/$(basename "$tool")"
done

for font in $FONTS; do
    install -D -m 0644 "$font" "$STAGE$font"
done

if [ -n "${WALLPAPERS:-}" ] && [ -d "$WALLPAPERS" ]; then
    for photo in "$WALLPAPERS"/*.jpg; do
        install -D -m 0644 "$photo" "$STAGE/usr/share/saule/wallpapers/$(basename "$photo")"
    done
fi

# The graphics drivers. Last, so the pack's libdrm replaces any the host's
# programs brought in.
if [ -n "$DRIVERS_DIR" ]; then
    add_drivers "$DRIVERS_DIR"
fi

cp -r "$ROOTFS/." "$STAGE/"
find "$STAGE" -name .keep -delete

if [ -n "$BUSYBOX" ]; then
    add_program "$BUSYBOX" /bin/busybox

    for applet in $("$BUSYBOX" --list); do
        case $applet in
            init | linuxrc | busybox) continue ;;
        esac
        [ -e "$STAGE/bin/$applet" ] || ln -s busybox "$STAGE/bin/$applet"
    done
fi

if [ -n "$TOOLS_BUSYBOX" ]; then
    add_program "$TOOLS_BUSYBOX" /usr/bin/busybox

    for applet in $("$TOOLS_BUSYBOX" --list); do
        case $applet in
            init | linuxrc | busybox) continue ;;
        esac
        [ -e "$STAGE/usr/bin/$applet" ] || ln -s busybox "$STAGE/usr/bin/$applet"
    done
fi

# The console must exist before devtmpfs is mounted: the kernel opens it
# for init's stdin/stdout/stderr.
pack_tree "$output" "nod /dev/console 0600 0 0 c 5 1"

echo "initramfs: $output ($(du -h "$output" | cut -f1))"

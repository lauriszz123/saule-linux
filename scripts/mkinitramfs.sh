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
#   SYSTEM         optional: saule-system (../system), installed as
#                  /sbin/saule-system, which init keeps running, with its
#                  DHCP script as /usr/share/saule/dhcp
#   WIFI           optional: a directory holding wpa_supplicant and the
#                  radios' firmware (scripts/fetch-wifi.sh), laid out as
#                  they are installed
#   ALSA           optional: ALSA's own configuration (/usr/share/alsa),
#                  which the sound library reads before it plays anything
#   ZONEINFO       optional: the time zone database (tzdata's
#                  /usr/share/zoneinfo), installed at the same path: what
#                  init tells a zone's time by, and the list of zones the
#                  first-start setup offers
#   BLUETOOTH      optional: a directory holding BlueZ, a D-Bus daemon
#                  for it and Intel's radios' firmware
#                  (scripts/fetch-bluetooth.sh), laid out as they are
#                  installed
#   XKB            optional: XKB's descriptions of keyboard layouts
#                  (xkeyboard-config's /usr/share/X11/xkb), installed at
#                  the same path with the library that reads them
#                  (libxkbcommon): what Shine types by on a keyboard that
#                  isn't a US one, and the list of layouts to choose from
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

# What the session asks the machine for: see ../system.
if [ -n "${SYSTEM:-}" ] && [ -f "$SYSTEM" ]; then
    add_program "$SYSTEM" /sbin/saule-system
    install -D -m 0755 "$(dirname "$0")/../system/dhcp" "$STAGE/usr/share/saule/dhcp"
fi

# Wi-Fi: wpa_supplicant with the libraries it loads, and the firmware files.
if [ -n "${WIFI:-}" ] && [ -d "$WIFI" ]; then
    if [ -f "$WIFI/sbin/wpa_supplicant" ]; then
        add_program "$WIFI/sbin/wpa_supplicant" /sbin/wpa_supplicant
    fi
    for library in "$WIFI"/lib/extra/*; do
        [ -f "$library" ] && install -D -m 0755 "$library" \
            "$STAGE/usr/lib/x86_64-linux-gnu/$(basename "$library")"
    done
    if [ -d "$WIFI/lib/firmware" ]; then
        mkdir -p "$STAGE/lib/firmware"
        cp -a "$WIFI/lib/firmware/." "$STAGE/lib/firmware/"
    fi
fi

# The sound library's configuration: which device "default" is, and how
# each kind of card is set up. Not the files for sound cards' own DSPs.
if [ -n "${ALSA:-}" ] && [ -d "$ALSA" ]; then
    mkdir -p "$STAGE/usr/share/alsa"
    cp -a "$ALSA/." "$STAGE/usr/share/alsa/"
    rm -rf "$STAGE/usr/share/alsa/topology" "$STAGE/usr/share/alsa/ucm" "$STAGE/usr/share/alsa/ucm2"
fi

# Every zone's file and the list of them (zone1970.tab); not the copies
# under other names for old programs, nor the tables nothing here reads.
if [ -n "${ZONEINFO:-}" ] && [ -d "$ZONEINFO" ]; then
    zones=$STAGE/usr/share/zoneinfo
    mkdir -p "$zones"
    cp -a "$ZONEINFO/." "$zones/"
    rm -rf "$zones/posix" "$zones/right" "$zones/localtime" "$zones/posixrules" \
        "$zones/Factory" "$zones/leap-seconds.list" "$zones/leapseconds" \
        "$zones/tzdata.zi" "$zones/iso3166.tab" "$zones/zone.tab" "$zones/zonenow.tab"
fi

# Bluetooth: BlueZ's daemon and its command-line client, the bus they
# talk over and its configuration, and the radios' firmware.
if [ -n "${BLUETOOTH:-}" ] && [ -d "$BLUETOOTH/bin" ]; then
    add_program "$BLUETOOTH/bin/bluetoothd" /usr/sbin/bluetoothd
    add_program "$BLUETOOTH/bin/bluetoothctl" /usr/bin/bluetoothctl
    add_program "$BLUETOOTH/bin/dbus-daemon" /usr/bin/dbus-daemon
    for library in "$BLUETOOTH"/lib/extra/*; do
        [ -f "$library" ] && install -D -m 0755 "$library" \
            "$STAGE/usr/lib/x86_64-linux-gnu/$(basename "$library")"
    done
    install -D -m 0644 "$BLUETOOTH/etc/system.conf" "$STAGE/usr/share/dbus-1/system.conf"
    if [ -d "$BLUETOOTH/lib/firmware" ]; then
        mkdir -p "$STAGE/lib/firmware"
        cp -a "$BLUETOOTH/lib/firmware/." "$STAGE/lib/firmware/"
    fi
fi

# Keyboard layouts, and the library Shine looks for when one is asked for.
# Not the drawings of keyboards nor the lists for other programs' menus.
# Without the library the descriptions are no use, and are left out: the
# system then types as a US keyboard and offers no other.
if [ -n "${XKB:-}" ] && [ -d "$XKB" ]; then
    # All of ldconfig's list is read: awk leaving at the first match would
    # end ldconfig with a broken pipe, and this script with it.
    xkbcommon=$(ldconfig -p | awk '$1 == "libxkbcommon.so.0" && /x86-64/ && !found { print $NF; found = 1 }')
    if [ -n "$xkbcommon" ]; then
        install -D -m 0755 "$(readlink -f "$xkbcommon")" "$STAGE$xkbcommon"
        add_libraries "$xkbcommon"
        layouts=$STAGE/usr/share/X11/xkb
        mkdir -p "$layouts"
        cp -aL "$XKB/." "$layouts/"
        rm -rf "$layouts/geometry" "$layouts"/rules/*.xml "$layouts"/rules/*.dtd
        # Newer libraries look under this name first.
        ln -sfn X11/xkb "$STAGE/usr/share/xkeyboard-config-2"
    else
        echo "mkinitramfs: no libxkbcommon.so.0 here: the image types as a US keyboard only" >&2
    fi
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

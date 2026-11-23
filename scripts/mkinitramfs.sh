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
#   NATIVE_LIBS    optional: native Saule packages (.so) the session may
#                  load. They travel inside the session; what goes in the
#                  tree is the shared libraries *they* need.
#   APPS_DIR       optional: a directory of apps built with `saule build`,
#                  installed as /bin/apps/<name>
#   FONTS          optional: font files, installed at the same path as on
#                  the build host (where Shine looks for them)
#
# Dynamically linked binaries bring the libraries `ldd` names for them.
# The tree is packed with root as the owner of every file.

set -euo pipefail

output=${1:?usage: mkinitramfs.sh OUTPUT.cpio.gz}

: "${STAGE:?}" "${GEN_INIT_CPIO:?}" "${INIT:?}" "${SESSION:?}" "${ROOTFS:?}"
BUSYBOX=${BUSYBOX:-}
NATIVE_LIBS=${NATIVE_LIBS:-}
FONTS=${FONTS:-}
APPS_DIR=${APPS_DIR:-}

# Absolute paths: the packing step below runs from inside the tree.
mkdir -p "$STAGE" "$(dirname "$output")"
output=$(realpath "$output")
STAGE=$(realpath "$STAGE")
GEN_INIT_CPIO=$(realpath "$GEN_INIT_CPIO")

rm -rf "$STAGE"
mkdir -p "$STAGE"/{bin,dev,etc,proc,sys,run,tmp,root}

# Copy every shared library a program or library loads into the tree.
add_libraries() {
    # Static binaries print "not a dynamic executable" and have no libraries.
    ldd "$1" 2>/dev/null | grep -oE '/[^ ]+' | while read -r library; do
        [ -e "$library" ] || continue
        install -D -m 0755 "$(readlink -f "$library")" "$STAGE$library"
    done || true
}

# Copy a program, and every shared library it loads, into the tree.
add_program() {
    local source=$1 target=$2

    install -D -m 0755 "$source" "$STAGE$target"
    add_libraries "$source"
}

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

for font in $FONTS; do
    install -D -m 0644 "$font" "$STAGE$font"
done

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

# gen_init_cpio reads a list of entries; owners are always root (0 0).
list=$(mktemp)
trap 'rm -f "$list"' EXIT

{
    # The console must exist before devtmpfs is mounted: the kernel opens it
    # for init's stdin/stdout/stderr.
    echo "nod /dev/console 0600 0 0 c 5 1"

    cd "$STAGE"
    find . -mindepth 1 | sort | while read -r path; do
        entry=${path#.}
        mode=$(stat -c '%a' "$path")

        if [ -L "$path" ]; then
            echo "slink $entry $(readlink "$path") $mode 0 0"
        elif [ -d "$path" ]; then
            echo "dir $entry $mode 0 0"
        else
            echo "file $entry $STAGE$entry $mode 0 0"
        fi
    done
} >"$list"

"$GEN_INIT_CPIO" "$list" | gzip -9 >"$output"

echo "initramfs: $output ($(du -h "$output" | cut -f1))"

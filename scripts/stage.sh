#!/usr/bin/env bash
# ============================================================================
# stage.sh — putting files in a tree and packing the tree as a cpio archive
# ============================================================================
#
# Sourced by mkinitramfs.sh (the system) and mkoverlay.sh (what is unpacked
# over it at boot: a graphics driver pack, the installer). Both set:
#
#   STAGE          the directory the tree is assembled in
#   GEN_INIT_CPIO  the kernel's cpio packer (usr/gen_init_cpio)
#
# Dynamically linked binaries bring the libraries `ldd` names for them.

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

# The graphics drivers of the pack in directory $1: every library of the
# pack, symlinks as they are, and what they load from the host (libstdc++,
# zlib). Stripped: Mesa's symbols are most of its size, and nothing here
# reads them.
add_drivers() {
    local pack=$1 lib=usr/lib/x86_64-linux-gnu library

    for library in "$pack/$lib"/*.so.* "$pack/$lib"/libgallium*.so \
        "$pack/$lib"/gbm/*.so; do
        [ -L "$library" ] || add_libraries "$library"
    done
    mkdir -p "$STAGE/$lib/gbm"
    cp -a "$pack/$lib"/*.so.* "$pack/$lib"/libgallium*.so "$STAGE/$lib/"
    cp -a "$pack/$lib/gbm/." "$STAGE/$lib/gbm/"
    find "$STAGE/$lib" -maxdepth 2 -type f \( -name 'libgallium*' -o -name 'libEGL*' \
        -o -name 'libGLESv2*' -o -name 'libgbm*' -o -name 'libdrm*' -o -name 'dri_gbm*' \
        -o -name 'libpciaccess*' \) -exec strip --strip-unneeded {} +
}

# Pack the tree as $1, gzipped, with root as the owner of every file. Any
# further arguments are entries for gen_init_cpio put before the tree's
# own: the device nodes a tree on disk can't hold.
pack_tree() {
    local output=$1 list
    shift

    # gen_init_cpio reads a list of entries; owners are always root (0 0).
    list=$(mktemp)
    {
        for entry in "$@"; do
            echo "$entry"
        done

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
    rm -f "$list"
}

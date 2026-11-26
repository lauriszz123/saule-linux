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

# Copy every shared library a program or library loads into the tree. OWN,
# when given, is a folder of libraries it came with, which this machine has
# none of. They are looked for there too, so that what *they* load is seen
# and copied as well: `ldd` can't say what a library it didn't find needs.
# Those go where the system looks for libraries; the rest, where they are.
add_libraries() {
    local file=$1 own=${2:-} library
    # The whole path, as `ldd` will print it; a folder that isn't there is
    # no folder of libraries.
    if [ -n "$own" ]; then
        own=$(cd "$own" 2>/dev/null && pwd) || own=
    fi
    # Static binaries print "not a dynamic executable" and have no libraries.
    if [ -n "$own" ]; then
        LD_LIBRARY_PATH=$own${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH} ldd "$file" 2>/dev/null
    else
        ldd "$file" 2>/dev/null
    fi | grep -oE '/[^ ]+' | while read -r library; do
        [ -e "$library" ] || continue
        case ${own:+$library} in
            "$own"/*)
                install -D -m 0755 "$(readlink -f "$library")" \
                    "$STAGE/usr/lib/x86_64-linux-gnu/$(basename "$library")" ;;
            *)
                install -D -m 0755 "$(readlink -f "$library")" "$STAGE$library" ;;
        esac
    done || true
}

# Copy a program, and every shared library it loads, into the tree. OWN,
# when given, is a folder of libraries the program came with, which this
# machine has none of: they are found there, and go where the system looks
# for libraries.
add_program() {
    local source=$1 target=$2 own=${3:-}

    install -D -m 0755 "$source" "$STAGE$target"
    add_libraries "$source" "$own"
}

# The graphics drivers of the pack in directory $1: every library of the
# pack, symlinks as they are, and what they load from the host (libstdc++,
# zlib). Stripped: Mesa's symbols are most of its size, and nothing here
# reads them.
#
# Where the drivers themselves are depends on the Mesa the pack was built
# from: one libgallium and a loader for it in gbm/, or (before Mesa 25) one
# file in dri/ under each driver's name.
add_drivers() {
    local pack=$1 lib=usr/lib/x86_64-linux-gnu library folder

    for library in "$pack/$lib"/*.so.* "$pack/$lib"/libgallium*.so \
        "$pack/$lib"/gbm/*.so "$pack/$lib"/dri/*.so; do
        [ -e "$library" ] || continue
        [ -L "$library" ] || add_libraries "$library"
    done

    mkdir -p "$STAGE/$lib"
    cp -a "$pack/$lib"/*.so.* "$STAGE/$lib/"
    for library in "$pack/$lib"/libgallium*.so; do
        [ -e "$library" ] && cp -a "$library" "$STAGE/$lib/"
    done
    for folder in gbm dri; do
        [ -d "$pack/$lib/$folder" ] || continue
        mkdir -p "$STAGE/$lib/$folder"
        cp -a "$pack/$lib/$folder/." "$STAGE/$lib/$folder/"
    done

    # dri/'s names are hard links to one file, which a cpio archive made
    # from a list would hold once per name: one of them stays the file and
    # the others become links to it.
    if [ -d "$STAGE/$lib/dri" ]; then
        local -A first=()
        local inode
        for library in "$STAGE/$lib/dri"/*.so; do
            [ -L "$library" ] && continue
            inode=$(stat -c '%i' "$library")
            if [ -n "${first[$inode]:-}" ]; then
                ln -sf "${first[$inode]}" "$library"
            else
                first[$inode]=$(basename "$library")
            fi
        done
    fi

    find "$STAGE/$lib" -maxdepth 2 -type f \( -name 'libgallium*' -o -name 'libEGL*' \
        -o -name 'libGLESv2*' -o -name 'libgbm*' -o -name 'libdrm*' -o -name 'dri_gbm*' \
        -o -name '*_dri.so' -o -name 'libglapi*' -o -name 'libpciaccess*' \) \
        -exec strip --strip-unneeded {} +
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

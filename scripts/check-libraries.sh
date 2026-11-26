#!/usr/bin/env bash
# ============================================================================
# check-libraries.sh — every program in a tree finds its libraries there
# ============================================================================
#
# Usage: check-libraries.sh ROOT
#
# Reads every program and shared library under ROOT, and says which shared
# libraries they name that ROOT doesn't hold, where the system started from
# ROOT would look for them. Fails when there are any.
#
# A program is copied into an image with the libraries `ldd` finds for it
# on the build machine. One that comes with libraries of its own, which the
# build machine has none of, is where that goes wrong: `ldd` can't follow a
# library it didn't find, so what that library needs in turn is never seen,
# and the program is left unable to start. bluetoothd was: it loads libdw,
# fetched with it, and libdw loads libelf, which nothing copied. Nothing
# said so until a machine with a radio tried to start it. This says so as
# the image is made.
#
# Only what a file names outright (its NEEDED entries) is checked, not what
# a program opens by name as it runs.

set -euo pipefail

root=${1:?usage: check-libraries.sh ROOT}
command -v readelf >/dev/null || {
    echo "check-libraries.sh: no readelf (binutils); the image's libraries are not checked" >&2
    exit 0
}

# Where the loader looks in a system with no list of its own
# (/etc/ld.so.conf): these, and beside the file that asks, which is where
# a driver's own parts are.
places="lib lib64 usr/lib usr/lib64 lib/x86_64-linux-gnu usr/lib/x86_64-linux-gnu"

missing=0
while IFS= read -r -d '' file; do
    # Programs and libraries only: an ELF file starts with these four bytes.
    [ "$(head -c 4 "$file" 2>/dev/null | od -An -c | tr -d ' ')" = '177ELF' ] || continue
    needed=$(readelf -d "$file" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
    for library in $needed; do
        found=0
        for place in $places; do
            [ -e "$root/$place/$library" ] && found=1 && break
        done
        [ "$found" = 1 ] || [ -e "$(dirname "$file")/$library" ] || {
            echo "check-libraries.sh: ${file#"$root"} needs $library, which the image doesn't have" >&2
            missing=1
        }
    done
done < <(find "$root" -type f \( -perm -u+x -o -name '*.so' -o -name '*.so.*' \) -print0)

[ "$missing" = 0 ] || exit 1

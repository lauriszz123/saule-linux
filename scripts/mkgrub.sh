#!/usr/bin/env bash
# ============================================================================
# mkgrub.sh — GRUB for a BIOS, as the two files a disk boots with
# ============================================================================
#
# Usage: mkgrub.sh OUT_DIR
#
# Writes OUT_DIR/boot.img (the code that goes in a disk's first sector) and
# OUT_DIR/core.img (GRUB itself, for the gap before the first partition).
# `saule-install` puts both on a disk: install/src/grub.rs says how.
#
# core.img holds only what booting Saule Linux takes: reading a disk through
# the BIOS, an MBR, an ext file system, and starting Linux. It looks for its
# configuration at /grub/grub.cfg on the first partition of the disk it was
# started from, so the same file boots a stick and the disk installed from it.
#
# Needs GRUB's tools and its BIOS modules: `apt install grub-pc-bin
# grub2-common`. Without them, on a machine with apt, the two packages are
# fetched and unpacked under TOOLS (default build/tools/grub) — nothing is
# installed, and no root is needed.

set -euo pipefail

out=${1:?usage: mkgrub.sh OUT_DIR}
tools=${TOOLS:-build/tools/grub}

mkimage=$(command -v grub-mkimage || true)
modules=/usr/lib/grub/i386-pc

if [ -z "$mkimage" ] || [ ! -f "$modules/boot.img" ]; then
    if [ ! -x "$tools/usr/bin/grub-mkimage" ] || [ ! -f "$tools/usr/lib/grub/i386-pc/boot.img" ]; then
        command -v apt-get >/dev/null && command -v dpkg-deb >/dev/null || {
            echo "mkgrub.sh: GRUB not found (apt install grub-pc-bin grub2-common)" >&2
            exit 1
        }
        echo "mkgrub.sh: no GRUB on this machine; fetching its packages into $tools"
        mkdir -p "$tools/packages"
        (cd "$tools/packages" && apt-get download -q grub-pc-bin grub2-common >/dev/null)
        for package in "$tools"/packages/*.deb; do
            dpkg-deb -x "$package" "$tools"
        done
    fi
    mkimage=$tools/usr/bin/grub-mkimage
    modules=$tools/usr/lib/grub/i386-pc
fi

mkdir -p "$out"
cp "$modules/boot.img" "$out/boot.img"

# `(,msdos1)`: the first partition of whichever disk this was loaded from.
"$mkimage" --format=i386-pc --directory="$modules" --output="$out/core.img" \
    --prefix='(,msdos1)/grub' \
    biosdisk part_msdos ext2 normal configfile linux

echo "grub: $out/core.img ($(du -h "$out/core.img" | cut -f1))"

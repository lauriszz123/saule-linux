#!/usr/bin/env bash
# ============================================================================
# mkgrub.sh — GRUB, as the files a disk boots with
# ============================================================================
#
# Usage: mkgrub.sh OUT_DIR EFI_IMAGE
#
# For a PC with a BIOS (the netbook), into OUT_DIR:
#
#   boot.img   the code that goes in a disk's first sector
#   core.img   GRUB itself, for the gap before the first partition
#
# For a PC with UEFI and no BIOS (most made since about 2012), EFI_IMAGE: a
# small FAT file system holding GRUB as EFI/BOOT/BOOTX64.EFI, the one place
# UEFI firmware looks on a disk nobody has told it about. It is written
# whole to a partition of its own (the "EFI system partition").
#
# `saule-install` puts all three on a disk: install/src/grub.rs says how.
#
# Both GRUBs hold only what booting Saule Linux takes: reading an MBR and an
# ext file system, starting Linux, and setting a screen mode for a kernel
# with no driver for the graphics chip. Both look for their configuration at
# /grub/grub.cfg on the first partition of the disk they were started from,
# so the same files boot a stick and the disk installed from it.
#
# UEFI firmware with Secure Boot on refuses this GRUB, which nobody has
# signed: Secure Boot has to be turned off in the firmware's settings.
#
# Needs GRUB's tools and modules (grub-common, grub-pc-bin,
# grub-efi-amd64-bin) and, for the FAT file system, dosfstools and mtools.
# Whatever is missing is fetched by fetch-debs.sh, with the packages it
# depends on, and unpacked under TOOLS (default build/tools/grub): nothing
# is installed, no root is needed.

set -euo pipefail

out=${1:?usage: mkgrub.sh OUT_DIR EFI_IMAGE}
efi_image=${2:?usage: mkgrub.sh OUT_DIR EFI_IMAGE}
tools=${TOOLS:-build/tools/grub}

# Megabytes of the EFI system partition: the least FAT32 comes in, which is
# what the UEFI specification asks of one. `EFI_SECTORS` in
# install/src/layout.rs is the same number.
efi_megabytes=34

# The path of FILE (relative to /), on this machine or among the packages
# fetched; the packages named are fetched when it is in neither.
find_file() {
    local file=$1
    shift
    if [ ! -e "/$file" ] && [ ! -e "$tools/$file" ]; then
        echo "mkgrub.sh: no /$file on this machine; fetching $* into $tools" >&2
        "$(dirname "$0")/fetch-debs.sh" "$tools/packages" "$tools" "$@"
    fi
    if [ -e "/$file" ]; then
        echo "/$file"
    else
        echo "$tools/$file"
    fi
}

mkimage=$(find_file usr/bin/grub-mkimage grub-common)
bios=$(dirname "$(find_file usr/lib/grub/i386-pc/boot.img grub-pc-bin)")
efi=$(dirname "$(find_file usr/lib/grub/x86_64-efi/normal.mod grub-efi-amd64-bin)")
mkfs_fat=$(find_file usr/sbin/mkfs.fat dosfstools)
mcopy=$(find_file usr/bin/mcopy mtools)
mmd=$(find_file usr/bin/mmd mtools)

# A program fetched into TOOLS loads the libraries fetched with it (the
# device mapper's, for grub-mkimage). Only what this machine lacks was
# fetched, so its own programs find nothing there to load instead of theirs.
export LD_LIBRARY_PATH="$tools/usr/lib/x86_64-linux-gnu:$tools/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

# `(,msdos1)`: the first partition of whichever disk GRUB was loaded from.
prefix='(,msdos1)/grub'
modules="part_msdos ext2 normal configfile linux all_video"

mkdir -p "$out" "$(dirname "$efi_image")"
cp "$bios/boot.img" "$out/boot.img"
"$mkimage" --format=i386-pc --directory="$bios" --output="$out/core.img" \
    --prefix="$prefix" biosdisk $modules

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
"$mkimage" --format=x86_64-efi --directory="$efi" --output="$work/BOOTX64.EFI" \
    --prefix="$prefix" $modules

rm -f "$efi_image"
truncate -s "${efi_megabytes}M" "$efi_image"
"$mkfs_fat" -F 32 -s 1 -n SAULE-EFI "$efi_image" >/dev/null
"$mmd" -i "$efi_image" ::/EFI ::/EFI/BOOT
"$mcopy" -i "$efi_image" "$work/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI

echo "grub: $out/core.img ($(du -h "$out/core.img" | cut -f1)), $efi_image ($(du -h "$work/BOOTX64.EFI" | cut -f1) of GRUB)"

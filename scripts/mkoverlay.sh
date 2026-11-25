#!/usr/bin/env bash
# ============================================================================
# mkoverlay.sh — an archive unpacked over the system when it boots
# ============================================================================
#
# Usage: mkoverlay.sh OUTPUT.cpio.gz [--drivers PACK_DIR] [--program FILE=PATH]...
#
# The kernel takes more than one initramfs: GRUB hands it the system's and
# then any others (`initrd /initramfs.cpio.gz /drivers/gen3.cpio.gz`), and
# each is unpacked over what came before. That is how one system image
# serves every machine: what only some need travels beside it.
#
#   --drivers PACK_DIR    a graphics driver pack (scripts/build-drivers.sh,
#                         build/drivers/<pack>): the one of several a machine
#                         takes for its chip
#   --program FILE=PATH   FILE installed as PATH, with the libraries it
#                         loads: the installer, which only the stick has
#
# Settings from the environment, as mkinitramfs.sh:
#
#   STAGE          directory to assemble the tree in (wiped first)
#   GEN_INIT_CPIO  the kernel's cpio packer (usr/gen_init_cpio)

set -euo pipefail

output=${1:?usage: mkoverlay.sh OUTPUT.cpio.gz [--drivers DIR] [--program FILE=PATH]...}
shift

: "${STAGE:?}" "${GEN_INIT_CPIO:?}"

mkdir -p "$STAGE" "$(dirname "$output")"
output=$(realpath "$output")
STAGE=$(realpath "$STAGE")
GEN_INIT_CPIO=$(realpath "$GEN_INIT_CPIO")

rm -rf "$STAGE"
mkdir -p "$STAGE"

. "$(dirname "$0")/stage.sh"

while [ $# -gt 0 ]; do
    case $1 in
        --drivers)
            add_drivers "${2:?--drivers needs the directory of a pack}"
            shift 2 ;;
        --program)
            spec=${2:?--program needs FILE=PATH}
            add_program "${spec%%=*}" "${spec#*=}"
            shift 2 ;;
        *)
            echo "mkoverlay.sh: unknown option $1" >&2
            exit 2 ;;
    esac
done

pack_tree "$output"

echo "overlay: $output ($(du -h "$output" | cut -f1))"

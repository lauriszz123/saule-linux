#!/usr/bin/env bash
# ============================================================================
# mkupdate.sh — an update file for systems already installed
# ============================================================================
#
# Usage: mkupdate.sh SAULE_INSTALL ROOT KEY OUT_DIR
#
# ROOT is a stick's folder as `make usb-image` stages it: the kernel, the
# initramfs and the driver packs. They are put in one file, signed with
# the private key KEY (`saule-install bundle`; see install/src/update.rs),
# and named for the version the initramfs says it is, so the name on a
# USB stick says what it will install.

set -euo pipefail

tool=${1:?usage: mkupdate.sh SAULE_INSTALL ROOT KEY OUT_DIR}
root=${2:?}
key=${3:?}
out_dir=${4:?}

# All of the archive is read: `cpio` leaving at the first match would end
# `zcat` with a broken pipe, and this script with it.
version=$(zcat "$root/initramfs.cpio.gz" | cpio -i --quiet --to-stdout etc/saule-release 2>/dev/null | head -n 1 || true)
if [ -z "$version" ]; then
    echo "mkupdate.sh: the initramfs in $root says no version (/etc/saule-release)" >&2
    exit 1
fi

out=$out_dir/saule-linux-$version.update
"$tool" bundle "$out" "$root" --key "$key" --version "$version"
echo "update: $out ($(du -h "$out" | cut -f1))"

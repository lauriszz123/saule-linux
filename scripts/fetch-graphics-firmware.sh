#!/usr/bin/env bash
# ============================================================================
# fetch-graphics-firmware.sh — what Intel's graphics chips load when they start
# ============================================================================
#
# Usage: fetch-graphics-firmware.sh OUT_DIR
#
# Intel's chips from Skylake (2015) on have a small program of their own
# for their display's power states (the "DMC"), which the kernel's driver
# loads into them from /lib/firmware/i915. Without it the chip works and
# the driver says it is missing; with it a laptop's screen and chip sleep
# as deeply as they can between frames, which is battery.
#
# Gathered into OUT_DIR/lib/firmware/i915, as mkinitramfs.sh installs it:
# the files for Skylake to Alder Lake, a few kilobytes each, pinned by
# checksum as fetch-wifi.sh's are.

set -euo pipefail

out=${1:?usage: fetch-graphics-firmware.sh OUT_DIR}
firmware_url=https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain

firmware="
i915/skl_dmc_ver1_27.bin   d3b6dc1a39bb2aeb37a1179f2b4e8145c24986da78c533571a5712dc91ec3f61
i915/kbl_dmc_ver1_04.bin   2cde41c3e5ad181423bcc3e98ff9c49f743c88f18646af4d0b3c3a9664b831a1
i915/bxt_dmc_ver1_07.bin   b4036e7698130c7a59637e543f6c614495c7076666104a2079d011f0923455f5
i915/glk_dmc_ver1_04.bin   412d3a3ed68908b983ff16cfe496e969c9fd31f94283e0872d7e5413c7bee504
i915/icl_dmc_ver1_09.bin   aafad1967679baa36e971e5331da73eea31c68239416432df83e71332755dc01
i915/tgl_dmc_ver2_12.bin   3c013ef0ad96ba73aee8e5bd04a8e27cc9b1c6e9183b1a83ce124485f325afca
i915/rkl_dmc_ver2_03.bin   139ce0eb873a13679a4685463cdacf6d247073ce286c8a5bca443ef1d17ee2ac
i915/adlp_dmc.bin          3516de2e134ddcf3b319c75d2e437779fecbd58cbb77234bd6f297c544e92ccb
i915/adlp_dmc_ver2_16.bin  2da482ea46a40e54c9ca3b54185959177f393eff98ece21acdac7eb6cacb0fcb
"

mkdir -p "$out/dl" "$out/lib/firmware/i915"

echo "$firmware" | while read -r path sum; do
    [ -n "$path" ] || continue
    name=$(basename "$path")
    if [ ! -f "$out/dl/$name" ]; then
        curl -fsSL "$firmware_url/$path" -o "$out/dl/$name.part"
        mv -f "$out/dl/$name.part" "$out/dl/$name"
    fi
    echo "$sum  $out/dl/$name" | sha256sum -c --quiet - || {
        echo "fetch-graphics-firmware.sh: $out/dl/$name is not the file expected" >&2
        exit 1
    }
    install -m 0644 "$out/dl/$name" "$out/lib/firmware/i915/$name"
done

echo "graphics firmware: $out ($(du -sh "$out/lib/firmware" | cut -f1))"

#!/usr/bin/env bash
# ============================================================================
# fetch-wifi.sh — what Wi-Fi needs in the image that is not built here
# ============================================================================
#
# Usage: fetch-wifi.sh OUT_DIR
#
# Into OUT_DIR, laid out as mkinitramfs.sh installs it (its WIFI):
#
#   sbin/wpa_supplicant    the program that joins a network: scanning, the
#                          handshake with the password, the keys. The
#                          machine's own, or fetched as a package
#   lib/extra/             a library it loads that this machine may lack
#   lib/firmware/          the files Intel's radios load before they work,
#                          from the linux-firmware repository, with the
#                          licence they may be passed on under
#
# The firmware is for the cards of about 2013 to 2017 (the Inspiron 7559's
# 3165 among them); the kernel names the file a card wants in its log when
# it is missing, and newer cards are a line each below. Atheros's 802.11n
# cards need no file.
#
# Whatever is not on this machine is fetched by fetch-debs.sh, with the
# packages it depends on, and unpacked under OUT_DIR: nothing is
# installed, no root is needed.

set -euo pipefail

out=${1:?usage: fetch-wifi.sh OUT_DIR}
firmware_url=https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain

# Each firmware file, where it is in the repository, and its hash.
firmware="
intel/iwlwifi/iwlwifi-7260-17.ucode   5d81a6003df0228a497ad27f916ba2c979614b4c439b0f45a5f2873dc0607fe8
intel/iwlwifi/iwlwifi-3160-17.ucode   0e893c1ed74c3186047508e93442b2d71a07dd5fac00d9dbdb68f34407d7be14
intel/iwlwifi/iwlwifi-7265-17.ucode   91f699308f102a9ed90217d813bfdb355464ece958a4f8ac96bba5e9d77fe870
intel/iwlwifi/iwlwifi-7265D-29.ucode  c52bcaa0595f64d5fb32b7db3dd4b832ea7bf78f1037bcafc19d9c0f5c5c617e
intel/iwlwifi/iwlwifi-3168-29.ucode   3afae7bf4bb5fbb83a895ec7ad097b9fd20b7d8794f405c2823cbea5284e65c3
intel/iwlwifi/iwlwifi-8000C-36.ucode  479931721f5e168d69d67c297c11738acd75da390adb94f96030a1055f4cf57a
intel/iwlwifi/iwlwifi-8265-36.ucode   1336afcd028ed094d1fe33893c84c273bb5711be52970040344a75a12f276d56
LICENSES/LICENCE.iwlwifi_firmware     16d5040c7cf851fc693b7542e20870935b99802533ea1bfd231c377a2305e5c5
brcm/bcm43xx-0.fw                     24668c702e723d2b1caebda047ee975ce16d7edf23788bc447dc12bcaf246b0f
brcm/bcm43xx_hdr-0.fw                 4aaa2ba62983d64b65f351908c59062a8a7c99100d190904c0e780d7c30f0dd0
"

mkdir -p "$out/dl" "$out/lib/firmware" "$out/lib/extra" "$out/sbin"

# Each file once, checked every time; a file that isn't the one expected is
# an error, not a file put in the image.
echo "$firmware" | while read -r path sum; do
    [ -n "$path" ] || continue
    name=$(basename "$path")
    if [ ! -f "$out/dl/$name" ]; then
        curl -fsSL "$firmware_url/$path" -o "$out/dl/$name.part"
        mv -f "$out/dl/$name.part" "$out/dl/$name"
    fi
    echo "$sum  $out/dl/$name" | sha256sum -c --quiet - || {
        echo "fetch-wifi.sh: $out/dl/$name is not the file expected" >&2
        exit 1
    }
    # Broadcom's driver looks for its files in a folder of their own.
    case $path in
        brcm/*) install -D -m 0644 "$out/dl/$name" "$out/lib/firmware/brcm/$name" ;;
        *) install -m 0644 "$out/dl/$name" "$out/lib/firmware/$name" ;;
    esac
done

# wpa_supplicant: this machine's own, else its package and whatever that
# needs which this machine doesn't have.
here=$(cd "$(dirname "$0")" && pwd)
packages=$out/dl/packages

supplicant=$(command -v wpa_supplicant || true)
if [ -z "$supplicant" ]; then
    if [ ! -x "$packages/root/usr/sbin/wpa_supplicant" ]; then
        echo "fetch-wifi.sh: no wpa_supplicant on this machine; fetching its package into $out"
        "$here/fetch-debs.sh" "$packages" "$packages/root" wpasupplicant
    fi
    supplicant=$packages/root/usr/sbin/wpa_supplicant
fi
install -m 0755 "$supplicant" "$out/sbin/wpa_supplicant"

# The libraries it loads that this machine has none of, and what those load
# in turn, from the packages: a round for each step down.
for round in 1 2 3; do
    for binary in "$out/sbin/wpa_supplicant" "$out"/lib/extra/*; do
        [ -f "$binary" ] || continue
        LD_LIBRARY_PATH="$out/lib/extra" ldd "$binary" 2>/dev/null |
            awk '/not found/ { print $1 }' | while read -r library; do
            # Often a link to the file with the version in its name.
            found=$(find "$packages/root" -name "$library" 2>/dev/null | head -1)
            [ -n "$found" ] || {
                echo "fetch-wifi.sh: $(basename "$binary") needs $library, which was not found" >&2
                exit 1
            }
            install -m 0755 "$(readlink -f "$found")" "$out/lib/extra/$library"
        done
    done
done

# Broadcom's cards before the BCM4313 (the BCM4312 of the HP Mini 210, and
# the 43xx of a decade of laptops): the kernel's b43 driver loads firmware
# that is in no repository, because Broadcom never allowed it to be passed
# on. It is cut out of Broadcom's own driver instead, as distributions'
# installers do on the machine they install to: the driver's archive is
# fetched, and b43-fwcutter takes the files out of it. An image with them
# in is for the machine it was built for, not for handing out. `B43=0`
# leaves them out.
b43_archive=broadcom-wl-5.100.138.tar.bz2
b43_url=https://sources.openwrt.org/$b43_archive
b43_sum=f1e7067aac5b62b67b8b6e4c517990277804339ac16065eb13c731ff909ae46f
if [ "${B43:-1}" != 0 ] && [ ! -f "$out/lib/firmware/b43/ucode15.fw" ]; then
    cutter=$(command -v b43-fwcutter || true)
    if [ -z "$cutter" ]; then
        "$here/fetch-debs.sh" "$out/packages" "$out/tools" b43-fwcutter
        cutter=$out/tools/usr/bin/b43-fwcutter
    fi
    if [ ! -f "$out/dl/$b43_archive" ]; then
        curl -fsSL "$b43_url" -o "$out/dl/$b43_archive.part"
        mv -f "$out/dl/$b43_archive.part" "$out/dl/$b43_archive"
    fi
    echo "$b43_sum  $out/dl/$b43_archive" | sha256sum -c --quiet - || {
        echo "fetch-wifi.sh: $out/dl/$b43_archive is not the file expected" >&2
        exit 1
    }
    rm -rf "$out/b43-src"
    mkdir -p "$out/b43-src"
    tar -xjf "$out/dl/$b43_archive" -C "$out/b43-src"
    "$cutter" -w "$out/lib/firmware" "$out/b43-src/broadcom-wl-5.100.138/linux/wl_apsta.o" >/dev/null
    rm -rf "$out/b43-src"
fi

# Which channels and how much power each country allows: the kernel reads
# it (and its signature) as a radio starts, and without it keeps to what
# is allowed everywhere, which leaves out channels 12 and 13 that Europe's
# networks use. From the wireless-regdb repository, at a tag.
regdb_url=https://git.kernel.org/pub/scm/linux/kernel/git/wens/wireless-regdb.git/plain
regdb_tag=master-2026-09-03
regdb="
regulatory.db      7e236caecd939c8ec98be4870bf30422f28ffef2565a38aaaa2d9ddabd0c2641
regulatory.db.p7s  50332f0db09b8bcc719235ec4985c27377f2cc2f42abb7b5dcf951866c5a888a
"
echo "$regdb" | while read -r name sum; do
    [ -n "$name" ] || continue
    if [ ! -f "$out/dl/$name" ]; then
        curl -fsSL "$regdb_url/$name?h=$regdb_tag" -o "$out/dl/$name.part"
        mv -f "$out/dl/$name.part" "$out/dl/$name"
    fi
    echo "$sum  $out/dl/$name" | sha256sum -c --quiet - || {
        echo "fetch-wifi.sh: $out/dl/$name is not the file expected" >&2
        exit 1
    }
    install -m 0644 "$out/dl/$name" "$out/lib/firmware/$name"
done

echo "wifi: $out ($(du -sh "$out/lib/firmware" | cut -f1) of firmware)"

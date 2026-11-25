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
# Whatever is not on this machine is fetched with `apt-get download` and
# unpacked under OUT_DIR: nothing is installed, no root is needed.

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
    install -m 0644 "$out/dl/$name" "$out/lib/firmware/$name"
done

# wpa_supplicant, and the one library of its that a machine without it
# installed tends not to have.
packages=$out/dl/packages
unpack() {
    mkdir -p "$packages"
    (cd "$packages" && apt-get download -q "$@" >/dev/null)
    local package
    for package in "$packages"/*.deb; do
        dpkg-deb -x "$package" "$packages/root"
    done
}

supplicant=$(command -v wpa_supplicant || true)
if [ -z "$supplicant" ]; then
    if [ ! -x "$packages/root/usr/sbin/wpa_supplicant" ]; then
        command -v apt-get >/dev/null || {
            echo "fetch-wifi.sh: wpa_supplicant not found (apt install wpasupplicant)" >&2
            exit 1
        }
        echo "fetch-wifi.sh: no wpa_supplicant on this machine; fetching its package into $out"
        unpack wpasupplicant libnl-genl-3-200
    fi
    supplicant=$packages/root/usr/sbin/wpa_supplicant
fi
install -m 0755 "$supplicant" "$out/sbin/wpa_supplicant"

# Libraries it names that this machine can't find: from the packages.
ldd "$supplicant" 2>/dev/null | awk '/not found/ { print $1 }' | while read -r library; do
    found=$(find "$packages/root" -name "$library*" -type f 2>/dev/null | head -1)
    [ -n "$found" ] || {
        echo "fetch-wifi.sh: wpa_supplicant needs $library, which was not found" >&2
        exit 1
    }
    install -m 0755 "$found" "$out/lib/extra/$library"
done

echo "wifi: $out ($(du -sh "$out/lib/firmware" | cut -f1) of firmware)"

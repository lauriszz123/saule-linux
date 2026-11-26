#!/usr/bin/env bash
# ============================================================================
# fetch-bluetooth.sh — what Bluetooth needs beside the kernel's half of it
# ============================================================================
#
# Usage: fetch-bluetooth.sh OUT_DIR
#
# BlueZ is Linux's Bluetooth: its daemon (bluetoothd) finds, pairs and
# connects, and is spoken to over D-Bus, so the image carries a D-Bus
# daemon for the two of them and nobody else. `saule-system` starts both
# and asks through BlueZ's own command-line client (system/src/bluetooth.rs).
#
# Gathered into OUT_DIR, laid out as mkinitramfs.sh installs it:
#
#   bin/            bluetoothd, bluetoothctl, dbus-daemon: this machine's
#                   own where it has them, else its distribution's packages
#   lib/extra/      the libraries they load that this machine doesn't have
#   lib/firmware/   what Intel's radios are loaded with when they start:
#                   the ones beside the Wi-Fi cards fetch-wifi.sh covers
#                   (7260, 3160, 7265, 3168, 8260, 8265)
#   etc/            the bus's configuration: root alone may use it
#
# The firmware is pinned by checksum, as fetch-wifi.sh's is.

set -euo pipefail

out=${1:?usage: fetch-bluetooth.sh OUT_DIR}
firmware_url=https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain

firmware="
intel/ibt-hw-37.7.10-fw-1.80.2.3.d.bseq   566f46e1531eac3aacde0213a0800f95952eee059d4af1354a3ab2d14df000d6
intel/ibt-hw-37.8.10-fw-1.10.3.11.e.bseq  25c925605ebb06751719a17543f4421f7dc26c5f4074abce0ec6fbe67feeb8e1
intel/ibt-11-5.sfi                         62e0ffb26d8f04adb5b9a2838ec7c60840bdabb38c024a98b905b991ed08e000
intel/ibt-11-5.ddc                         9e57302abc15ac991b71350705c4b5060f69c7111813bc5034704f6edea30872
intel/ibt-12-16.sfi                        75b8b8cc10abd616a96066bce9ce62be80ebc293d6a4e9a0a11c3ecc5d3ca373
intel/ibt-12-16.ddc                        3ca252ad20e8f76d6d906536bf9e5abe6654c657183584a48e90a0dcf40ac4d5
"

mkdir -p "$out/dl" "$out/lib/firmware/intel" "$out/lib/extra" "$out/bin" "$out/etc"

echo "$firmware" | while read -r path sum; do
    [ -n "$path" ] || continue
    name=$(basename "$path")
    if [ ! -f "$out/dl/$name" ]; then
        curl -fsSL "$firmware_url/$path" -o "$out/dl/$name.part"
        mv -f "$out/dl/$name.part" "$out/dl/$name"
    fi
    echo "$sum  $out/dl/$name" | sha256sum -c --quiet - || {
        echo "fetch-bluetooth.sh: $out/dl/$name is not the file expected" >&2
        exit 1
    }
    install -m 0644 "$out/dl/$name" "$out/lib/firmware/intel/$name"
done

# The programs: this machine's own, else from its distribution's packages,
# unpacked beside the downloads and never installed.
packages=$out/dl/packages
have_all=1
for program in bluetoothctl dbus-daemon; do
    command -v "$program" >/dev/null || have_all=0
done
[ -x /usr/sbin/bluetoothd ] || [ -x /usr/libexec/bluetooth/bluetoothd ] || have_all=0

if [ "$have_all" = 0 ] && [ ! -x "$packages/root/usr/bin/bluetoothctl" ]; then
    echo "fetch-bluetooth.sh: fetching BlueZ's and D-Bus's packages into $out"
    # With whatever they need that this machine doesn't have: GLib,
    # AppArmor's library, elfutils', D-Bus's own.
    "$(dirname "$0")/fetch-debs.sh" "$packages" "$packages/root" bluez dbus-daemon
fi

# Where a program is: on this machine, or in the packages.
where() {
    local name=$1 path
    for path in "$(command -v "$name" 2>/dev/null || true)" "/usr/sbin/$name" \
        "/usr/libexec/bluetooth/$name" "$packages/root/usr/bin/$name" \
        "$packages/root/usr/sbin/$name" "$packages/root/usr/libexec/bluetooth/$name"; do
        if [ -n "$path" ] && [ -x "$path" ] && [ ! -L "$path" ]; then
            echo "$path"
            return
        fi
    done
    echo "fetch-bluetooth.sh: $name was not found" >&2
    exit 1
}

for program in bluetoothd bluetoothctl dbus-daemon; do
    install -m 0755 "$(where "$program")" "$out/bin/$program"
done

# The libraries they load that this machine has none of, and what those
# load in turn, from the packages.
for round in 1 2 3; do
    for binary in "$out"/bin/* "$out"/lib/extra/*; do
        [ -f "$binary" ] || continue
        LD_LIBRARY_PATH="$out/lib/extra" ldd "$binary" 2>/dev/null |
            awk '/not found/ { print $1 }' | while read -r library; do
            # Often a link to the file with the version in its name.
            found=$(find "$packages/root" -name "$library" 2>/dev/null | head -1)
            [ -n "$found" ] || {
                echo "fetch-bluetooth.sh: $(basename "$binary") needs $library, which was not found" >&2
                exit 1
            }
            install -m 0755 "$(readlink -f "$found")" "$out/lib/extra/$library"
        done
    done
done

# The bus: a system bus that root alone may connect to and say anything
# on. Its two users are bluetoothd and bluetoothctl, both run by
# saule-system; the session is not one.
cat >"$out/etc/system.conf" <<'CONF'
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>system</type>
  <listen>unix:path=/run/dbus/system_bus_socket</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <deny user="*"/>
    <allow user="root"/>
  </policy>
  <policy user="root">
    <allow own="*"/>
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
  </policy>
</busconfig>
CONF

echo "bluetooth: $out ($(du -sh "$out/lib/firmware" | cut -f1) of firmware)"

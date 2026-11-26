#!/usr/bin/env bash
# ============================================================================
# fetch-debs.sh — Debian packages, unpacked where a build can use them
# ============================================================================
#
# Usage: fetch-debs.sh DOWNLOADS ROOT PACKAGE...
#
# Fetches each PACKAGE into DOWNLOADS, with every package it depends on, all
# the way down, that this machine doesn't have installed, and unpacks them
# all over ROOT. Nothing is installed and no root is needed. What was
# fetched before is not fetched again.
#
# The package a program comes in rarely holds everything it loads:
# wpa_supplicant needs libnl's three libraries, bluetoothctl GLib, D-Bus
# AppArmor's, grub-mkimage the device mapper's. Naming them by hand missed
# whichever ones the machine the list was made on happened to have.
# Following the packages' own dependencies takes exactly what this machine
# lacks, and leaves out what it has, so nothing fetched stands in for one of
# its own libraries.
#
# Needs apt's package lists. A machine that has none (a Docker image, which
# removes them to be smaller) is told to run `apt-get update`.

set -euo pipefail

usage="usage: fetch-debs.sh DOWNLOADS ROOT PACKAGE..."
downloads=${1:?$usage}
root=${2:?$usage}
shift 2
[ $# -gt 0 ] || { echo "$usage" >&2; exit 2; }

fail() {
    echo "fetch-debs.sh: $1" >&2
    exit 1
}

for tool in apt-get apt-cache dpkg-deb dpkg-query; do
    command -v "$tool" >/dev/null || fail "no $tool: install $* by hand"
done

# A package apt can't name means its lists are missing or out of date.
for package in "$@"; do
    apt-cache show "$package" >/dev/null 2>&1 ||
        fail "apt knows no package $package (its lists may be missing: apt-get update)"
done

installed() {
    dpkg-query -W -f='${db:Status-Abbrev}' "$1" 2>/dev/null | grep -q '^ii'
}

# The packages and everything under them: real packages only (a virtual
# one is shown as <name>, and what it stands for is listed beside it),
# each once.
needed=()
while read -r package; do
    installed "$package" || needed+=("$package")
done < <(
    apt-cache depends --recurse --no-recommends --no-suggests --no-conflicts \
        --no-breaks --no-replaces --no-enhances "$@" | grep -v '^[ <]' | sort -u
)
[ ${#needed[@]} -gt 0 ] || exit 0

mkdir -p "$downloads" "$root"
missing=()
for package in "${needed[@]}"; do
    # A package's file is NAME_VERSION_ARCH.deb, without any `:arch`.
    compgen -G "$downloads/${package%%:*}_*.deb" >/dev/null || missing+=("$package")
done
if [ ${#missing[@]} -gt 0 ]; then
    (cd "$downloads" && apt-get download -q "${missing[@]}" >/dev/null)
fi

for package in "${needed[@]}"; do
    for deb in "$downloads/${package%%:*}"_*.deb; do
        dpkg-deb -x "$deb" "$root"
    done
done

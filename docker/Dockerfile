# The build host for Saule Linux on a machine that isn't x86_64 Linux (a Mac).
# scripts/docker.sh builds and runs it; nothing here needs to be run by hand.
#
# x86_64 on purpose: `saule build` packs the session into a copy of the saule
# binary that runs it, so saule has to be an x86_64 Linux program.

FROM --platform=linux/amd64 ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential flex bison bc libelf-dev libssl-dev cpio \
        busybox-static \
        git curl ca-certificates xz-utils python3 libncurses-dev \
        pkg-config libasound2-dev fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

# What the image's fetch scripts take from the build machine when it has
# it, and would otherwise fetch from apt: Wi-Fi's wpa_supplicant and
# Broadcom's firmware cutter (fetch-wifi.sh), BlueZ and D-Bus
# (fetch-bluetooth.sh), GRUB and a FAT file system's tools (mkgrub.sh), and
# fontTools (common-fonts.sh). Fetching needs apt's lists, which the line
# above removes, so they are installed here instead, and are found as a
# Linux build machine's own would be: with the libraries they load, which
# the image takes from here. And what the image copies from the build
# machine as it is: the keyboard layouts and the library that reads them
# (the Makefile's XKB), and the names of PCI devices (PCI_IDS), without
# which an image made here has no layout but US and reports say no device's
# name.
RUN apt-get update && apt-get install -y --no-install-recommends \
        wpasupplicant b43-fwcutter bluez dbus-daemon \
        grub-common grub-pc-bin grub-efi-amd64-bin dosfstools mtools \
        python3-fonttools \
        xkb-data libxkbcommon0 pci.ids \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path \
    && rustup target add x86_64-unknown-linux-musl

# The repos are bind-mounted from the Mac and owned by another uid.
RUN git config --system --add safe.directory '*'

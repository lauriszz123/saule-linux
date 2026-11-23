#!/usr/bin/env bash
# ============================================================================
# qemu-netbook.sh — boot in QEMU dressed as the HP Mini 210-1000
# ============================================================================
#
# Usage:
#   qemu-netbook.sh --usb IMAGE            boot a USB stick image through the
#                                          BIOS, the way the netbook will
#   qemu-netbook.sh --kernel BZIMAGE --initrd INITRAMFS [--append "..."]
#                                          load the kernel directly (no
#                                          bootloader)
#   qemu-netbook.sh                        no boot medium: just the BIOS
#
#   --nographic      no window; the serial port and the QEMU monitor share
#                    this terminal (Ctrl-A X quits)
#   --bios-log FILE  write SeaBIOS's debug output to FILE
#   --               everything after this goes to QEMU as it is
#
# The serial port is on this terminal in both modes (console=ttyS0).
# QEMU=/path/to/qemu-system-x86_64 overrides the binary.
#
# How close it gets to the real machine:
#
#   real hardware                   here
#   ------------------------------  -------------------------------------------
#   Atom N450, 1.66 GHz, 1 core,    n270 (same family: Bonnell) with long mode,
#     2 threads, 64-bit               SYSCALL and NX, 1 core x 2 threads
#   2 GB DDR2                       2 GB
#   legacy BIOS, no EFI             SeaBIOS
#   NM10 chipset (ICH7-based)       q35 (ICH9): the closest chipset QEMU has
#   GMA 3150, 1024x600 panel        standard VGA (bochs-display/simpledrm/vesa
#                                     in Linux) preferring 1024x600 — there is
#                                     no i915 emulation, so only the "keep the
#                                     BIOS mode" framebuffer path is testable
#   USB 2.0 (EHCI), USB stick       EHCI controller + usb-storage, bootable
#   PS/2 keyboard, Synaptics pad    PS/2 keyboard + PS/2 mouse (the touchpad
#                                     shows up as a plain mouse)
#   HD Audio                        intel-hda + hda-duplex (sound discarded)
#   Atheros LAN, Broadcom Wi-Fi     not emulated: no network
#
# There's no KVM on a Mac and the guest is x86 anyway, so this is plain
# emulation (TCG): slower than the real thing, but the boot path is the same.

set -euo pipefail

QEMU=${QEMU:-qemu-system-x86_64}

usb= kernel= initrd= append= nographic=0 bios_log=
while [ $# -gt 0 ]; do
    case $1 in
        --usb)       usb=${2:?--usb needs an image};           shift 2 ;;
        --kernel)    kernel=${2:?--kernel needs a bzImage};    shift 2 ;;
        --initrd)    initrd=${2:?--initrd needs an initramfs}; shift 2 ;;
        --append)    append=${2?--append needs a string};      shift 2 ;;
        --nographic) nographic=1;                              shift ;;
        --bios-log)  bios_log=${2:?--bios-log needs a file};   shift 2 ;;
        --)          shift; break ;;
        -h|--help)   sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)           echo "qemu-netbook.sh: unknown option $1 (try --help)" >&2; exit 2 ;;
    esac
done

if [ -n "$usb" ] && [ -n "$kernel" ]; then
    echo "qemu-netbook.sh: --usb and --kernel boot differently; pick one" >&2
    exit 2
fi
if [ -n "$kernel" ] && [ -z "$initrd" ]; then
    echo "qemu-netbook.sh: --kernel needs --initrd" >&2
    exit 2
fi

args=(
    -name "HP Mini 210-1000"
    -machine q35
    -cpu "n270,+lm,+syscall,+nx,+lahf-lm,model-id=Intel(R) Atom(TM) CPU N450   @ 1.66GHz"
    -smp 2,sockets=1,cores=1,threads=2
    -m 2G
    -no-reboot

    # Screen: a plain VGA whose EDID prefers the panel's 1024x600.
    -vga none
    -device VGA,xres=1024,yres=600

    # USB 2.0 controller; the stick (if any) hangs off it.
    -device usb-ehci,id=ehci

    # HD Audio, with the sound thrown away.
    -audiodev none,id=snd
    -device intel-hda
    -device hda-duplex,audiodev=snd

    -nic none
)

if [ -n "$usb" ]; then
    [ -r "$usb" ] || { echo "qemu-netbook.sh: can't read $usb" >&2; exit 1; }
    args+=(
        -drive "if=none,id=stick,format=raw,file=$usb"
        -device usb-storage,bus=ehci.0,drive=stick,removable=on,bootindex=0
    )
elif [ -n "$kernel" ]; then
    args+=(-kernel "$kernel" -initrd "$initrd")
    # The last console= is the one /dev/console — and so the session — uses.
    if [ "$nographic" = 1 ]; then
        args+=(-append "console=tty1 console=ttyS0 $append")
    else
        args+=(-append "console=ttyS0 console=tty1 $append")
    fi
fi

if [ -n "$bios_log" ]; then
    args+=(-chardev "file,id=seabios,path=$bios_log"
           -device isa-debugcon,iobase=0x402,chardev=seabios)
fi

if [ "$nographic" = 1 ]; then
    args+=(-nographic)
else
    args+=(-serial stdio)
fi

exec "$QEMU" "${args[@]}" "$@"

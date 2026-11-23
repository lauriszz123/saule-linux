#!/usr/bin/env bash
# ============================================================================
# qemu-netbook.sh — boot in QEMU dressed as the HP Mini 210-1001sa
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
#   --no-kvm         emulate the CPU in software even when KVM is available
#   --memory SIZE    RAM, e.g. 2G for a netbook with its memory upgraded
#                    (default 1G, as the 1001sa ships)
#   --slow[=PERCENT] run about as fast as the real netbook: KVM, with the
#                    virtual CPUs capped at PERCENT of one CPU core
#                    (default 8). See "Speed" below.
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
#   1 GB DDR2 (one slot, max 2 GB)  1 GB (--memory 2G for an upgraded one)
#   legacy BIOS, no EFI             SeaBIOS
#   NM10 chipset (ICH7-based)       q35 (ICH9): the closest chipset QEMU has
#   GMA 3150, 1024x600 panel        virtio-vga (virtio-gpu in Linux, VGA for
#                                     the BIOS) preferring 1024x600 — there is
#                                     no i915 emulation; the real machine's
#                                     driver is only testable on it
#   USB 2.0 (EHCI), USB stick       EHCI controller + usb-storage, bootable
#   PS/2 keyboard, Synaptics pad    PS/2 keyboard + PS/2 mouse (the touchpad
#                                     shows up as a plain mouse), and a USB
#                                     tablet as the pointer on the desktop
#   HD Audio                        intel-hda + hda-duplex (sound discarded)
#   Atheros LAN, Broadcom Wi-Fi     not emulated: no network
#
# Speed: with KVM (Linux, /dev/kvm writable — be in the `kvm` group) the
# guest runs on the real CPU, much faster than the netbook. Without it (a
# Mac, or no access) every instruction is emulated in software (TCG), at a
# speed that depends on this machine, not the netbook's.
#
# --slow gets close to the netbook: KVM, with the VM's virtual CPUs limited
# to a share of one core by the kernel's CPU accounting (cgroups, in a
# systemd user scope, so no root needed). Only the virtual CPUs: QEMU's own
# threads, the one drawing this window above all, run free. Capping the
# whole process made the guest pay for showing its screen on this desktop,
# which a real netbook doesn't, and a busy screen ran at a third of the
# speed. The default 8% is an *estimate*: an Atom N450 against a Ryzen 5
# 7600, from published single-thread benchmark scores, and on that machine
# `examples/bench` runs about 16 times slower than natively at 8%. Calibrate
# it by running `examples/bench` on the real netbook and in here, and
# adjusting the percentage until the two agree. It's an approximation of
# the CPU only: memory and graphics stay this machine's.

set -euo pipefail

QEMU=${QEMU:-qemu-system-x86_64}

usb= kernel= initrd= append= nographic=0 bios_log= kvm=1 slow= memory=1G
while [ $# -gt 0 ]; do
    case $1 in
        --usb)       usb=${2:?--usb needs an image};           shift 2 ;;
        --kernel)    kernel=${2:?--kernel needs a bzImage};    shift 2 ;;
        --initrd)    initrd=${2:?--initrd needs an initramfs}; shift 2 ;;
        --append)    append=${2?--append needs a string};      shift 2 ;;
        --nographic) nographic=1;                              shift ;;
        --bios-log)  bios_log=${2:?--bios-log needs a file};   shift 2 ;;
        --no-kvm)    kvm=0;                                    shift ;;
        --slow)      slow=8;                                   shift ;;
        --memory)    memory=${2:?--memory needs a size, e.g. 2G};  shift 2 ;;
        --slow=*)    slow=${1#--slow=};                        shift ;;
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

if [ -n "$slow" ]; then
    case $slow in
        '' | *[!0-9]* | 0)
            echo "qemu-netbook.sh: --slow takes a percentage of one core, e.g. --slow=8" >&2
            exit 2 ;;
    esac
    # The cap is a share of real CPU time; on top of software emulation it
    # would mean nothing anyone could calibrate.
    if [ "$kvm" = 0 ] || [ ! -w /dev/kvm ]; then
        echo "qemu-netbook.sh: --slow needs KVM (be in the \`kvm\` group)" >&2
        exit 1
    fi
    case $(systemctl --user is-system-running 2>/dev/null) in
        running | degraded) ;;
        *)
            echo "qemu-netbook.sh: --slow needs systemd's user manager (systemd-run --user)" >&2
            exit 1 ;;
    esac
    # Starved of CPU in bursts, the kernel's boot-time check of the timer
    # interrupt can time out and panic ("IO-APIC + timer doesn't work") —
    # an artifact of the cap, not of a slow machine.
    append="no_timer_check $append"
fi

# KVM when it's there and allowed: the same CPU model, run by the real CPU.
cpu="n270,+lm,+syscall,+nx,+lahf-lm,model-id=Intel(R) Atom(TM) CPU N450   @ 1.66GHz"
smp="2,sockets=1,cores=1,threads=2"
accel=tcg
if [ "$kvm" = 1 ] && [ -w /dev/kvm ]; then
    accel=kvm
    # Two things a real CPU may not do that emulation can, each of which made
    # QEMU warn on every start: the N450's self-snoop flag, and two threads
    # on one core without AMD's topology extension. Two cores instead.
    cpu="$cpu,-ss"
    smp="2,sockets=1,cores=2,threads=1"
elif [ "$kvm" = 1 ] && [ -e /dev/kvm ]; then
    echo "qemu-netbook.sh: no access to /dev/kvm, emulating the CPU in software" \
         "(slow; \`sudo usermod -aG kvm \$USER\` and log in again for KVM)" >&2
fi

args=(
    # Named threads: how --slow finds the virtual CPUs ("CPU 0/KVM").
    -name "HP Mini 210-1001sa",debug-threads=on
    -machine q35,accel=$accel
    -cpu "$cpu"
    -smp "$smp"
    -m "$memory"
    -no-reboot

    # Screen: QEMU's paravirtual GPU (with a VGA side for the BIOS), whose
    # display prefers the panel's 1024x600. Not the netbook's GMA 3150,
    # which QEMU can't emulate. A plain emulated VGA would do, but QEMU
    # spots what changed on one by write-protecting its memory, and every
    # frame then costs the guest's CPU time no real chip charges; at
    # --slow, that made a busy screen run at a third of its speed.
    -vga none
    -device virtio-vga,xres=1024,yres=600

    # USB 2.0 controller; the stick (if any) hangs off it.
    -device usb-ehci,id=ehci

    # The pointer, as a USB tablet: absolute positions, so the mouse moves
    # in and out of the QEMU window with the desktop's cursor, no grab
    # needed (grabbing barely works under WSLg). On its own controller,
    # since a tablet is a USB 1.1 device and EHCI only speaks 2.0.
    -device qemu-xhci,id=xhci
    -device usb-tablet,bus=xhci.0

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
    # The panel's size, pinned: the virtual GPU reports the size of the
    # window it's shown in as the screen's preferred mode, so without this
    # the guest would shrink to fit a fresh 640x480 window rather than the
    # window growing to the netbook's screen. Shine takes a mode given here
    # over the preferred one.
    append="video=Virtual-1:1024x600 $append"
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
    # An SDL window, sized to the screen. QEMU's GTK window doesn't follow
    # the virtual GPU's screen size (it expects the guest to follow the
    # window instead), so it stayed at 640x480 and cut the netbook's screen
    # off. Unless a display was asked for after `--`.
    case " $* " in
        *" -display "* | *" -nographic "*) ;;
        *) args+=(-display sdl) ;;
    esac
fi

if [ -n "$slow" ]; then
    # QEMU runs as this very process (systemd-run execs it in place), in a
    # scope of its own that this user may divide up. Once its virtual CPUs
    # exist, move them into a sub-group with the cap; the rest stays free.
    # A 20 ms period keeps it smooth: the guest runs a little every 20 ms
    # rather than in long bursts with long freezes between.
    me=$$
    (
        # Looking before the threads exist fails, which is expected here.
        set +e +o pipefail
        for _ in $(seq 1 500); do
            vcpus=$(grep -l '^CPU [0-9]*/KVM' /proc/$me/task/*/comm 2>/dev/null | cut -d/ -f5)
            [ -n "$vcpus" ] && break
            sleep 0.02
        done
        cg=/sys/fs/cgroup$(cut -d: -f3 "/proc/$me/cgroup" 2>/dev/null)
        case $cg in
            */run-*.scope) ;;
            *) echo "qemu-netbook.sh: --slow: QEMU isn't in a scope of its own; not capped" >&2
               exit 1 ;;
        esac
        if [ -z "$vcpus" ] \
            || ! mkdir "$cg/vcpus" \
            || ! echo threaded >"$cg/vcpus/cgroup.type" \
            || ! echo +cpu >"$cg/cgroup.subtree_control"; then
            echo "qemu-netbook.sh: --slow: could not set up the cap; running at full speed" >&2
            exit 1
        fi
        for t in $vcpus; do
            echo "$t" >"$cg/vcpus/cgroup.threads"
        done
        echo "$((slow * 200)) 20000" >"$cg/vcpus/cpu.max"
        echo "qemu-netbook.sh: virtual CPUs capped at $slow% of one core, about the netbook's speed" >&2
    ) &
    exec systemd-run --user --scope --quiet -p Delegate=yes -- "$QEMU" "${args[@]}" "$@"
fi

exec "$QEMU" "${args[@]}" "$@"

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
#   --gl             the virtual GPU with 3D: the guest's OpenGL (Mesa's virgl,
#                    from the `virtual` driver pack) runs on this machine's
#                    GPU, the nearest QEMU comes to the netbook's GMA 3150
#   --vesa           no graphics driver: the screen the BIOS set up (VESA,
#                    1024x768), as the netbook would show if i915 failed
#                    on its GMA 3150, or any PC whose chip has no driver
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
repo=$(cd "$(dirname "$0")/.." && pwd)

# What the options set. `append` grows as the machine is put together:
# several parts of it need a kernel option of their own.
usb= kernel= initrd= append= nographic=0 bios_log= kvm=1 slow= memory=1G vesa=0 gl=0
extra=()     # everything after `--`, for QEMU as it is
args=()      # QEMU's command line
mac_window=0

fail() {
    echo "qemu-netbook.sh: $1" >&2
    exit "${2:-1}"
}

# ---------------------------------------------------------------------------
# Options
# ---------------------------------------------------------------------------

parse_options() {
    while [ $# -gt 0 ]; do
        case $1 in
            --usb)       usb=${2:?--usb needs an image};           shift 2 ;;
            --kernel)    kernel=${2:?--kernel needs a bzImage};    shift 2 ;;
            --initrd)    initrd=${2:?--initrd needs an initramfs}; shift 2 ;;
            --append)    append=${2?--append needs a string};      shift 2 ;;
            --nographic) nographic=1;                              shift ;;
            --bios-log)  bios_log=${2:?--bios-log needs a file};   shift 2 ;;
            --no-kvm)    kvm=0;                                    shift ;;
            --vesa)      vesa=1;                                   shift ;;
            --gl)        gl=1;                                     shift ;;
            --slow)      slow=8;                                   shift ;;
            --slow=*)    slow=${1#--slow=};                        shift ;;
            --memory)    memory=${2:?--memory needs a size, e.g. 2G}; shift 2 ;;
            --)          shift; extra=("$@"); return ;;
            -h|--help)   sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
            *)           fail "unknown option $1 (try --help)" 2 ;;
        esac
    done
}

check_options() {
    if [ -n "$usb" ] && [ -n "$kernel" ]; then
        fail "--usb and --kernel boot differently; pick one" 2
    fi
    if [ -n "$kernel" ] && [ -z "$initrd" ]; then
        fail "--kernel needs --initrd" 2
    fi
    if [ -n "$slow" ]; then
        check_slow
    fi
}

# --slow needs a percentage, KVM, and systemd's user manager to cap it with.
check_slow() {
    case $slow in
        '' | *[!0-9]* | 0)
            fail "--slow takes a percentage of one core, e.g. --slow=8" 2 ;;
    esac
    # The cap is a share of real CPU time; on top of software emulation it
    # would mean nothing anyone could calibrate.
    if [ "$kvm" = 0 ] || [ ! -w /dev/kvm ]; then
        fail "--slow needs KVM (be in the \`kvm\` group)"
    fi
    case $(systemctl --user is-system-running 2>/dev/null) in
        running | degraded) ;;
        *) fail "--slow needs systemd's user manager (systemd-run --user)" ;;
    esac
    # Starved of CPU in bursts, the kernel's boot-time check of the timer
    # interrupt can time out and panic ("IO-APIC + timer doesn't work") —
    # an artifact of the cap, not of a slow machine.
    append="no_timer_check $append"
}

# ---------------------------------------------------------------------------
# The machine
# ---------------------------------------------------------------------------

# The processor, memory, chipset, and what is soldered to the board.
add_machine() {
    # KVM when it's there and allowed: the same CPU model, run by the real CPU.
    local cpu="n270,+lm,+syscall,+nx,+lahf-lm,model-id=Intel(R) Atom(TM) CPU N450   @ 1.66GHz"
    local smp="2,sockets=1,cores=1,threads=2"
    local accel=tcg
    if [ "$kvm" = 1 ] && [ -w /dev/kvm ]; then
        accel=kvm
        # Two things a real CPU may not do that emulation can, each of which
        # made QEMU warn on every start: the N450's self-snoop flag, and two
        # threads on one core without AMD's topology extension. Two cores
        # instead.
        cpu="$cpu,-ss"
        smp="2,sockets=1,cores=2,threads=1"
    elif [ "$kvm" = 1 ] && [ -e /dev/kvm ]; then
        echo "qemu-netbook.sh: no access to /dev/kvm, emulating the CPU in software" \
             "(slow; \`sudo usermod -aG kvm \$USER\` and log in again for KVM)" >&2
    fi

    args+=(
        # Named threads: how --slow finds the virtual CPUs ("CPU 0/KVM").
        -name "HP Mini 210-1001sa",debug-threads=on
        -machine q35,accel=$accel
        -cpu "$cpu"
        -smp "$smp"
        -m "$memory"
        # No -no-reboot: Restart boots it again, as it does the netbook.
        # Shut Down still ends QEMU.

        # USB 2.0 controller; the stick (if any) hangs off it.
        -device usb-ehci,id=ehci

        # The pointer, as a USB tablet: absolute positions, so the mouse
        # moves in and out of the QEMU window with the desktop's cursor, no
        # grab needed (grabbing barely works under WSLg). On its own
        # controller, since a tablet is a USB 1.1 device and EHCI only
        # speaks 2.0.
        -device qemu-xhci,id=xhci
        -device usb-tablet,bus=xhci.0

        # HD Audio, with the sound thrown away.
        -audiodev none,id=snd
        -device intel-hda
        -device hda-duplex,audiodev=snd

        -nic none
    )
}

# The screen: QEMU's paravirtual GPU (with a VGA side for the BIOS), whose
# display prefers the panel's 1024x600. Not the netbook's GMA 3150, which
# QEMU can't emulate. A plain emulated VGA would do, but QEMU spots what
# changed on one by write-protecting its memory, and every frame then costs
# the guest's CPU time no real chip charges; at --slow, that made a busy
# screen run at a third of its speed.
#
# With --vesa, a plain VGA card whose driver is kept from starting, so the
# kernel falls back to the framebuffer the BIOS set up (simpledrm), as it
# would on a chip it has no driver for. vga=0x318 asks the BIOS for 1024x768
# at 24 bits; VESA has no 1024x600 mode.
add_screen() {
    if [ "$vesa" = 1 ]; then
        args+=(-vga std)
        append="vga=0x318 initcall_blacklist=bochs_pci_driver_init $append"
    elif [ "$gl" = 1 ]; then
        args+=(-vga none -device virtio-vga-gl,xres=1024,yres=600)
    else
        args+=(-vga none -device virtio-vga,xres=1024,yres=600)
    fi
}

# What it boots from: a USB stick through the BIOS, the kernel loaded
# directly, or nothing (just the BIOS).
add_boot_medium() {
    # Under WSL the Super key never reaches a Linux window: Windows keeps it
    # (Start, Win+D, Win+Space). So the window manager is told to use
    # Ctrl+Alt, as it does nested on Windows, unless the command line says.
    if [ -n "${WSL_DISTRO_NAME:-}" ] && [[ "$append" != *SAULE_WM_KEY* ]]; then
        append="saule.env.SAULE_WM_KEY=ctrl+alt $append"
    fi

    if [ -n "$usb" ]; then
        [ -r "$usb" ] || fail "can't read $usb"
        args+=(
            -drive "if=none,id=stick,format=raw,file=$usb"
            -device usb-storage,bus=ehci.0,drive=stick,removable=on,bootindex=0
        )
    elif [ -n "$kernel" ]; then
        args+=(-kernel "$kernel" -initrd "$initrd")
        # The panel's size, pinned: the virtual GPU reports the size of the
        # window it's shown in as the screen's preferred mode, so without
        # this the guest would shrink to fit a fresh 640x480 window rather
        # than the window growing to the netbook's screen. Shine takes a
        # mode given here over the preferred one.
        if [ "$vesa" = 0 ]; then
            append="video=Virtual-1:1024x600 $append"
        fi
        # The last console= is the one /dev/console — and so the session —
        # uses.
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
}

# ---------------------------------------------------------------------------
# The window
# ---------------------------------------------------------------------------

# Where the screen is shown: nowhere (--nographic), or a window sized to
# it, unless a display was asked for after `--`.
add_window() {
    # The window's own OpenGL, which a 3D virtual GPU draws through. Under
    # WSL that is Windows' GPU through Mesa's D3D12 driver, which Mesa
    # doesn't pick by itself there (it would draw on this machine's CPU
    # instead).
    local gl_display=
    if [ "$gl" = 1 ]; then
        gl_display=",gl=on"
        if [ -e /dev/dxg ] && [ -z "${GALLIUM_DRIVER:-}" ]; then
            export GALLIUM_DRIVER=d3d12
        fi
    fi

    if [ "$nographic" = 1 ]; then
        args+=(-nographic)
        return
    fi
    args+=(-serial stdio)

    case " ${extra[*]:-} " in
        *" -display "* | *" -nographic "*)
            return ;;
    esac

    # On a Mac, QEMU's own window (see use_mac_window); elsewhere SDL:
    # QEMU's GTK window doesn't follow the virtual GPU's screen size (it
    # expects the guest to follow the window instead), so it stayed at
    # 640x480 and cut the netbook's screen off.
    if [ "$(uname -s)" = Darwin ]; then
        mac_window=1
    elif [ -n "${WSL_DISTRO_NAME:-}" ]; then
        # Under WSL: QEMU's own keys (the screen's console on Ctrl+Alt+1,
        # 2, …, full screen on Ctrl+Alt+F) on the right Ctrl alone, so the
        # left Ctrl+Alt reaches the window manager, whose key it is here
        # (see SAULE_WM_KEY above).
        args+=(-display "sdl,grab-mod=rctrl$gl_display")
    else
        args+=(-display "sdl$gl_display")
    fi
}

# On a Mac: the netbook's screen at its own size, one netbook pixel to one
# point. QEMU draws a guest pixel to each *physical* pixel of a Retina
# screen, so 1024x600 showed at a quarter of the netbook's size, and its
# zoom-to-fit scaling keeps rescaling the picture, which shakes. An app
# that says it can't do Retina is drawn by macOS at 1x and every pixel
# doubled: so QEMU runs from a copy of itself in a small .app that says so
# (build/qemu-netbook.app, made once and again when QEMU is upgraded).
# The copy finds its firmware by -L, as it no longer sits beside it.
# full-grab: Cmd+Space and the other shortcuts macOS takes reach the
# window manager inside.
use_mac_window() {
    local real app copy
    real=$(command -v "$QEMU") && real=$(readlink -f "$real") \
        || fail "$QEMU not found"
    app=$repo/build/qemu-netbook.app
    copy=$app/Contents/MacOS/$(basename "$real")
    if [ ! -x "$copy" ] || [ "$real" -nt "$copy" ]; then
        mkdir -p "$app/Contents/MacOS"
        cp "$real" "$copy"
        cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>$(basename "$real")</string>
  <key>CFBundleIdentifier</key><string>org.saule.qemu-netbook</string>
  <key>CFBundleName</key><string>HP Mini 210</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><false/>
</dict>
</plist>
PLIST
        # A copied binary needs signing again before macOS runs it.
        codesign --force --sign - "$copy" >/dev/null 2>&1
    fi
    QEMU=$copy
    args+=(-L "$(dirname "$real")/../share/qemu" -display cocoa,full-grab=on)
}

# ---------------------------------------------------------------------------
# Running
# ---------------------------------------------------------------------------

# --slow: once process `pid` (QEMU) has its virtual CPUs, move them into a
# cgroup of their own with the cap; the rest of QEMU stays free. A 20 ms
# period keeps it smooth: the guest runs a little every 20 ms rather than
# in long bursts with long freezes between.
cap_virtual_cpus() {
    local pid=$1 vcpus= cg t
    # Looking before the threads exist fails, which is expected here.
    set +e +o pipefail
    for _ in $(seq 1 500); do
        vcpus=$(grep -l '^CPU [0-9]*/KVM' /proc/$pid/task/*/comm 2>/dev/null | cut -d/ -f5)
        [ -n "$vcpus" ] && break
        sleep 0.02
    done
    cg=/sys/fs/cgroup$(cut -d: -f3 "/proc/$pid/cgroup" 2>/dev/null)
    case $cg in
        */run-*.scope) ;;
        *) fail "--slow: QEMU isn't in a scope of its own; not capped" ;;
    esac
    if [ -z "$vcpus" ] \
        || ! mkdir "$cg/vcpus" \
        || ! echo threaded >"$cg/vcpus/cgroup.type" \
        || ! echo +cpu >"$cg/cgroup.subtree_control"; then
        fail "--slow: could not set up the cap; running at full speed"
    fi
    for t in $vcpus; do
        echo "$t" >"$cg/vcpus/cgroup.threads"
    done
    echo "$((slow * 200)) 20000" >"$cg/vcpus/cpu.max"
    echo "qemu-netbook.sh: virtual CPUs capped at $slow% of one core, about the netbook's speed" >&2
}

# Become QEMU. With --slow, in a systemd scope of its own that this user
# may divide up (systemd-run execs it in place, so it keeps this process's
# id, which the capping waits on in the background).
run() {
    # An empty array is an error to the bash 3.2 a Mac has, under `set -u`.
    args+=(${extra[@]+"${extra[@]}"})
    if [ -n "$slow" ]; then
        (cap_virtual_cpus $$) &
        exec systemd-run --user --scope --quiet -p Delegate=yes -- "$QEMU" "${args[@]}"
    fi
    exec "$QEMU" "${args[@]}"
}

parse_options "$@"
check_options
add_machine
add_screen
add_boot_medium
add_window
if [ "$mac_window" = 1 ]; then
    use_mac_window
fi
run

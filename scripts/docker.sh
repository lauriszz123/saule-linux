#!/usr/bin/env bash
# ============================================================================
# docker.sh — build Saule Linux in an x86_64 Ubuntu container
# ============================================================================
#
# For hosts that can't build it themselves (a Mac). The Makefile calls this
# on macOS, so `make`, `make run` and the rest work as usual there.
#
# Usage:
#   docker.sh make [TARGET...] [VAR=value...]
#       build saule (from SAULE_SRC) and Shine (from SHINE_SRC), then run
#       make inside the container; the kernel, the initramfs, the USB sticks
#       and update files it made are copied to out/ for QEMU and the host
#   docker.sh shell
#       a shell in the container, in the repo
#   docker.sh run COMMAND...
#       run one command in the container, in the repo
#
#   SAULE_SRC   the saule repo to build `saule` from (default: ../saule)
#   WM_SRC      the window manager (default: ../saule-wm; skipped if
#               missing), built as the session when it's there
#   SHINE_SRC   the Shine repo (default: ../saule-shine; skipped if missing).
#               It is built from a copy, against SAULE_SRC's saule-sdk, so
#               its native ABI matches the saule it runs in and its
#               Cargo.lock is left alone.
#
# An app in APPS= that lives beside this repo rather than in it
# (`../saule-linux-apps/settings`) has the repository it is in mounted
# read-only at the same place relative to this one, so the path means the
# same in the container, and so does any path dependency it has: on another
# folder of its repository (`../lib/fileskit`) or on a sibling repository
# mounted anyway (`../saule-wm/api`).
#
# What lives where:
#   the repo (and SAULE_SRC)   bind-mounted from the host: edits are shared
#   build/                     the volume saule-linux-build — the kernel
#                              source needs a case-sensitive file system
#   init/target, saule target  the volume saule-linux-target, which also
#                              holds the Saule home (/target/saule-home)
#                              that `saule build` finds Shine in
#   cargo downloads            the volume saule-linux-cargo
#   out/                       in the repo: bzImage and initramfs for QEMU
#
# `docker volume rm saule-linux-build saule-linux-target saule-linux-cargo`
# forgets all of it.

set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
SAULE_SRC=$(cd "${SAULE_SRC:-$repo/../saule}" && pwd)
SHINE_SRC=${SHINE_SRC:-$repo/../saule-shine}
shine=()
if [ -d "$SHINE_SRC" ]; then
    shine=(-v "$(cd "$SHINE_SRC" && pwd):/src/saule-shine:ro")
fi
# The window manager, at the same ../saule-wm the Makefile looks for.
WM_SRC=${WM_SRC:-$repo/../saule-wm}
wm=()
if [ -d "$WM_SRC" ]; then
    wm=(-v "$(cd "$WM_SRC" && pwd):/src/saule-wm:ro")
fi

# The repositories of the apps outside this one, from the APPS= among the
# arguments: each once, and none already mounted above. Mounting an app's
# folder alone would hide what it shares with the rest of its repository.
apps=()
mounted=" saule "
[ ${#shine[@]} -gt 0 ] && mounted="${mounted}saule-shine "
[ ${#wm[@]} -gt 0 ] && mounted="${mounted}saule-wm "
for arg in "$@"; do
    case $arg in
        APPS=*)
            for app in ${arg#APPS=}; do
                case $app in
                    ../*)
                        top=${app#../}
                        top=${top%%/*}
                        case $mounted in *" $top "*) continue ;; esac
                        host=$(cd "$repo/../$top" && pwd) || exit 1
                        apps+=(-v "$host:/src/$top:ro")
                        mounted="$mounted$top "
                        ;;
                esac
            done
            ;;
    esac
done

docker info >/dev/null 2>&1 || {
    echo "docker.sh: no Docker engine running (on a Mac: \`colima start\` or open Docker Desktop)" >&2
    exit 1
}

# The image is named after its Dockerfile, so editing it rebuilds it.
tag=$(shasum "$repo/docker/Dockerfile" | cut -c1-12)
image=saule-linux-build:$tag
if ! docker image inspect "$image" >/dev/null 2>&1; then
    echo "docker.sh: building the build image ($image)"
    docker build --platform linux/amd64 -t "$image" "$repo/docker"
fi

tty=()
[ -t 0 ] && [ -t 1 ] && tty=(-it)

run() {
    docker run --rm ${tty[@]+"${tty[@]}"} --platform linux/amd64 \
        -v "$repo:/src/saule-linux" \
        -v "$SAULE_SRC:/src/saule" \
        -v saule-linux-build:/src/saule-linux/build \
        -v saule-linux-target:/target \
        ${shine[@]+"${shine[@]}"} \
        ${wm[@]+"${wm[@]}"} \
        ${apps[@]+"${apps[@]}"} \
        -v saule-linux-cargo:/opt/cargo/registry \
        -e CARGO_TARGET_DIR=/target \
        -e SAULE_HOME=/target/saule-home \
        -w /src/saule-linux \
        "$image" "$@"
}

cmd=${1:-}
[ $# -gt 0 ] && shift
case $cmd in
    make)
        # Cargo builds into /target (CARGO_TARGET_DIR), so INIT points there.
        run bash -c '
            set -euo pipefail
            cargo build --release --locked --manifest-path /src/saule/Cargo.toml -p saule-cli

            # `saule build` refuses a package with no receipt in
            # $SAULE_HOME/installed, and a source package (uikit) has to be
            # there in full. So install the window manager'"'"'s dependencies
            # the way `saule install` does, at the versions its saule.lock
            # pins, from a copy: the checkout is mounted read-only. Shine is
            # among them, and the build below then replaces its library.
            if [ -d /src/saule-wm ]; then
                rm -rf /tmp/wm-deps
                cp -r /src/saule-wm /tmp/wm-deps
                (cd /tmp/wm-deps && /target/release/saule install)
            fi

            # Shine, from a copy so the patch below cannot touch its Cargo.lock.
            if [ -d /src/saule-shine ]; then
                mkdir -p /target/shine-src
                tar -C /src/saule-shine --exclude=./target --exclude=./.git -cf - . \
                    | tar -C /target/shine-src -xf -
                cargo build --release --manifest-path /target/shine-src/Cargo.toml \
                    --config "patch.\"https://github.com/lauriszz123/saule\".saule-sdk.path=\"/src/saule/crates/saule-sdk\"" \
                    --config "patch.\"https://github.com/lauriszz123/saule\".saule-ipc.path=\"/src/saule/crates/saule-ipc\""
                mkdir -p "$SAULE_HOME/native_packages"
                cp /target/release/libsaule_shine.so "$SAULE_HOME/native_packages/"
            fi
            make SAULE=/target/release/saule INIT=/target/x86_64-unknown-linux-musl/release/saule-init "$@"
            # What the Mac runs or hands on: the kernel and initramfs for
            # QEMU, the sticks (usb-image, run-usb) and update files. Each
            # only when newer than the copy there, a stick being large.
            mkdir -p out
            for f in build/kernel/arch/x86/boot/bzImage build/initramfs.cpio.gz \
                build/saule-linux.img build/saule-linux-qemu.img build/saule-linux-*.update; do
                if [ -f "$f" ] && [ "$f" -nt "out/${f##*/}" ]; then
                    cp -p "$f" out/
                fi
            done
            true
        ' bash "$@"
        ;;
    shell)
        run bash -l
        ;;
    run)
        run "$@"
        ;;
    *)
        sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
        exit 2
        ;;
esac

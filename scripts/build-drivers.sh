#!/usr/bin/env bash
# ============================================================================
# build-drivers.sh — graphics driver packs for Saule Linux: Mesa and libdrm
# ============================================================================
#
# What Shine draws on the GPU through on the bare screen: EGL on GBM (no
# X11, no Wayland) and OpenGL ES. Built as one pack per kind of chip, so a
# machine gets only the driver it needs: the system looks at its graphics
# chip and installs that pack (see config/drivers.list for which chip is which).
#
#   gen3       i915: Intel Gen3, the netbook's GMA 3150 (GLES 2 only)
#   intel      crocus, iris: Intel Gen4 to now
#   amd        r300, r600, radeonsi: ATI/AMD Radeon, radeonsi with ACO
#              rather than LLVM, which alone would add ~100 MB
#   nvidia     nouveau: NVIDIA, the open driver
#   virtual    virgl: QEMU's virtio GPU with 3D on, which hands the drawing
#              to the host's GPU
#   software   softpipe alone: no usable GPU, drawn on the CPU
#
# Every pack has softpipe too, so a chip its driver turns down still draws.
# Each is a whole Mesa (libEGL, libGLESv2, libgbm, libgallium) in
# build/drivers/<pack>, prefix /usr with Debian's multiarch libdir, so the
# paths compiled in are the system's. Versions are pinned, and the tarballs
# checked against the hashes below.
#
# Needs meson, ninja and Python's mako and yaml on the build host:
#   apt install meson ninja-build python3-mako python3-yaml
#
# Usage: build-drivers.sh [PACK...]     (default: every pack)
#   OUT=dir   where the packs go (default: build/drivers)

set -euo pipefail

LIBDRM_VERSION=2.4.131
LIBDRM_SHA256=45ba9983b51c896406a3d654de81d313b953b76e6391e2797073d543c5f617d5
MESA_VERSION=26.0.8
MESA_SHA256=caf1c0061a68e88dfa74967a7e780c0e85d65b6c4e334cd69095a5dc54ad78bc

# Each pack's gallium drivers.
declare -A PACKS=(
    [gen3]=i915,softpipe
    [intel]=crocus,iris,softpipe
    [amd]=r300,r600,radeonsi,softpipe
    [nvidia]=nouveau,softpipe
    [virtual]=virgl,softpipe
    [software]=softpipe
)

wanted=("$@")
[ ${#wanted[@]} -gt 0 ] || wanted=(gen3 intel amd nvidia virtual software)
for pack in "${wanted[@]}"; do
    [ -n "${PACKS[$pack]:-}" ] || { echo "build-drivers.sh: no pack called $pack" >&2; exit 2; }
done

out=$(realpath -m "${OUT:-build/drivers}")
src=$out/src
libdrm_root=$out/libdrm
libdir=lib/x86_64-linux-gnu

for tool in meson ninja; do
    command -v "$tool" >/dev/null || {
        echo "build-drivers.sh: $tool not found (apt install meson ninja-build python3-mako python3-yaml)" >&2
        exit 1
    }
done
python3 -c 'import mako, yaml' 2>/dev/null || {
    echo "build-drivers.sh: Python's mako and yaml are missing (apt install python3-mako python3-yaml)" >&2
    exit 1
}

mkdir -p "$src"

# Download `url` into the source folder once, and check it every time.
fetch() {
    local url=$1 sum=$2 file
    file=$src/$(basename "$url")
    if [ ! -f "$file" ]; then
        curl -fsSL "$url" -o "$file.part"
        mv -f "$file.part" "$file"
    fi
    echo "$sum  $file" | sha256sum -c --quiet - || {
        echo "build-drivers.sh: $file is not the tarball expected" >&2
        exit 1
    }
    tar -C "$src" -xf "$file"
}

fetch "https://dri.freedesktop.org/libdrm/libdrm-$LIBDRM_VERSION.tar.xz" "$LIBDRM_SHA256"
fetch "https://archive.mesa3d.org/mesa-$MESA_VERSION.tar.xz" "$MESA_SHA256"

# libdrm: the kernel's graphics interface, with the helpers the drivers
# use (Intel's, AMD's, NVIDIA's). Built once; every pack carries a copy.
rm -rf "$libdrm_root"
meson setup --wipe "$out/libdrm-build" "$src/libdrm-$LIBDRM_VERSION" \
    --prefix=/usr --libdir="$libdir" --buildtype=release \
    -Dintel=auto -Dradeon=enabled -Damdgpu=enabled -Dnouveau=enabled \
    -Dvmwgfx=disabled -Dtests=false -Dman-pages=disabled \
    -Dvalgrind=disabled -Dcairo-tests=disabled
ninja -C "$out/libdrm-build"
DESTDIR="$libdrm_root" ninja -C "$out/libdrm-build" install

# Mesa finds that libdrm through copies of its pkg-config files that point
# into the tree, so it compiles against it while the paths it bakes in stay
# the system's /usr.
pc=$out/pkgconfig
rm -rf "$pc" && mkdir -p "$pc"
for f in "$libdrm_root/usr/$libdir/pkgconfig/"*.pc; do
    sed "s|^prefix=.*|prefix=$libdrm_root/usr|" "$f" > "$pc/$(basename "$f")"
done

for pack in "${wanted[@]}"; do
    root=$out/$pack
    rm -rf "$root"
    cp -a "$libdrm_root" "$root"

    PKG_CONFIG_PATH=$pc meson setup --wipe "$out/mesa-build-$pack" "$src/mesa-$MESA_VERSION" \
        --prefix=/usr --libdir="$libdir" --buildtype=release -Db_ndebug=true \
        -Dplatforms= -Degl-native-platform=drm \
        -Degl=enabled -Dgbm=enabled -Dgles1=disabled -Dgles2=enabled \
        -Dopengl=true -Dglx=disabled -Dglvnd=disabled \
        -Dgallium-drivers="${PACKS[$pack]}" -Dvulkan-drivers= \
        -Dllvm=disabled -Damd-use-llvm=false \
        -Dgallium-va=disabled -Dvideo-codecs= \
        -Dxmlconfig=disabled -Dexpat=disabled \
        -Dlmsensors=disabled -Dlibunwind=disabled -Dvalgrind=disabled \
        -Dzstd=disabled -Dbuild-tests=false -Dtools=
    ninja -C "$out/mesa-build-$pack"
    DESTDIR="$root" ninja -C "$out/mesa-build-$pack" install
    rm -rf "$root/usr/share/man" "$root/usr/share/doc"
    echo "drivers: $pack in $root ($(du -sh "$root/usr/$libdir" | cut -f1))"
done

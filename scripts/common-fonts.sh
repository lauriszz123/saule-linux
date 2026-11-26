#!/bin/sh
# The common part of each font: a copy with only the characters most text
# is made of, to go beside the whole file in the image.
#
#   scripts/common-fonts.sh OUT FONT...
#
# Shine reads every outline of a font file before it draws the first
# letter, and every program reads its fonts as it starts: six thousand
# outlines of DejaVu Sans are a third of a second on a netbook, each
# launch. Where `Name.common.ttf` lies beside `Name.ttf`, Shine starts with
# that one and reads the whole file only when a character outside it turns
# up (see "Text" in Shine's README). What is drawn is the same: the
# outlines, widths and kerning are copied, not remade.
#
# Kept: Latin with its accented letters, punctuation, currency, the signs
# and arrows keys and menus are labelled with, box drawing and shapes:
# some seven hundred outlines of the six thousand. Greek, Cyrillic,
# mathematics and the rest stay in the whole file.
#
# Needs fontTools (Debian's python3-fonttools). Without it, it is fetched
# into build/tools, unpacked and used from there; nothing is installed.
# Without that either, no common parts are made and the image works as it
# would with whole fonts only.

set -eu

OUT=$1
shift
TOOLS=$(cd "$(dirname "$0")/.." && pwd)/build/tools/fonttools
PACKAGES=$TOOLS/usr/lib/python3/dist-packages
RANGES="U+0020-007E,U+00A0-017F,U+0218-021B,U+02C6-02DD,U+2010-2027"
RANGES="$RANGES,U+2030-203A,U+2044,U+20AC,U+2116,U+2122,U+2190-2199"
RANGES="$RANGES,U+21B5,U+21E5,U+21E7,U+21EA,U+2212,U+221E,U+2248,U+2260"
RANGES="$RANGES,U+2264,U+2265,U+2303,U+2318,U+2325,U+2326,U+232B,U+238B"
RANGES="$RANGES,U+23CE,U+23CF,U+2500-259F,U+25A0-25FF,U+2605,U+2606"
RANGES="$RANGES,U+2713-2717,U+FFFD"

has_fonttools() {
    PYTHONPATH="$PACKAGES${PYTHONPATH:+:$PYTHONPATH}" \
        python3 -c 'import fontTools.subset' 2>/dev/null
}

if ! has_fonttools; then
    mkdir -p "$TOOLS"
    (cd "$TOOLS" && apt-get download -q python3-fonttools >/dev/null 2>&1 &&
        for deb in *.deb; do dpkg-deb -x "$deb" .; done) || true
fi
if ! has_fonttools; then
    echo "common-fonts: no fontTools (python3-fonttools); whole fonts only" >&2
    exit 0
fi

mkdir -p "$OUT"
for font in "$@"; do
    [ -f "$font" ] || continue
    name=$(basename "$font")
    part="$OUT/${name%.*}.common.${name##*.}"
    if [ "$part" -nt "$font" ] && [ "$part" -nt "$0" ]; then
        continue
    fi
    PYTHONPATH="$PACKAGES${PYTHONPATH:+:$PYTHONPATH}" python3 -m fontTools.subset \
        "$font" --unicodes="$RANGES" --output-file="$part" \
        --layout-features='' --no-hinting --notdef-outline \
        --name-IDs='*' --legacy-kern 2>/dev/null
    echo "common-fonts: $(basename "$part"), $(wc -c <"$part") of $(wc -c <"$font") bytes"
done

#!/usr/bin/env bash
# ============================================================================
# check-config.sh — did every option in config/kernel.config make it?
# ============================================================================
#
# Usage: check-config.sh FRAGMENT KERNEL_DOT_CONFIG
#
# Kconfig quietly drops an option whose dependencies aren't met, so a line in
# the fragment can do nothing. This lists every such line.

set -euo pipefail

fragment=${1:?usage: check-config.sh FRAGMENT .config}
config=${2:?usage: check-config.sh FRAGMENT .config}
missing=0

while IFS= read -r line; do
    case $line in
        CONFIG_*=*) ;;
        *) continue ;;
    esac

    if ! grep -qxF "$line" "$config"; then
        name=${line%%=*}
        actual=$(grep -E "^($name=|# $name is not set)" "$config" || echo "(not offered)")
        echo "  wanted $line, got $actual"
        missing=1
    fi
done <"$fragment"

if [ $missing -eq 0 ]; then
    echo "kernel config: every option in $fragment is set"
else
    echo "kernel config: some options were dropped (missing dependencies?)"
fi

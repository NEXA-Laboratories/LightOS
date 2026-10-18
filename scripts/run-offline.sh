#!/bin/sh
# Boot LightOS without a server (the package manager uses its built-in repository).
IMG="$(dirname "$0")/../kernel/target/x86_64-lightos/release/bootimage-lightos.bin"
[ -f "$IMG" ] || { echo "run scripts/build.sh first"; exit 1; }
exec qemu-system-x86_64 -drive format=raw,file="$IMG" -serial stdio -m 128M "$@"

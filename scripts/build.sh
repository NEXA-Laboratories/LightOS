#!/bin/sh
# Build the kernel image and the data disk.
set -e
cd "$(dirname "$0")/.."
command -v bootimage >/dev/null || cargo install bootimage
(cd kernel && cargo bootimage --release)
python3 tools/mkdisk.py data.img --root disk-root

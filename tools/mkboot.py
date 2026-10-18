#!/usr/bin/env python3
"""Prepare the LightOS boot image for VirtualBox and check that the installer can use it.

  python tools/mkboot.py kernel/target/x86_64-lightos/release/bootimage-lightos.bin build/live.img

* pads the image to a whole number of MiB (VirtualBox convertfromraw is happier with that)
* checks the boot signature (0x55AA)
* checks that the partition-table area of sector 0 (bytes 446..509) is free: the installer
  stores the partition table there. If this prints WARNING, send me the output.
* checks the image fits in the 8 MiB system area copied by `install`.
Exit status: 0 ok, 1 error, 3 usable but the installer will refuse (table area occupied).
"""
import os, sys

SYSTEM_AREA = 16384 * 512
MIB = 1024 * 1024


def prepare(src, dst):
    data = open(src, "rb").read()
    if len(data) < 512 or data[510:512] != b"\x55\xaa":
        print("ERROR: missing 0x55AA boot signature - is this a bootimage-*.bin file?")
        return 1
    if len(data) > SYSTEM_AREA:
        print(f"ERROR: image is {len(data)} bytes; the installer copies at most {SYSTEM_AREA}")
        return 1
    status = 0
    if any(data[446:510]):
        print("WARNING: bytes 446..509 of the boot sector are not zero; the bootloader uses the")
        print("         partition-table area, so 'install' will refuse. Please report this.")
        status = 3
    else:
        print("OK: partition-table area of the boot sector is free")
    padded = data + b"\0" * (-len(data) % MIB or 0)
    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    open(dst, "wb").write(padded)
    print(f"wrote {dst}: {len(padded)} bytes ({len(padded)//MIB} MiB), system area use {len(data)*100//SYSTEM_AREA}%")
    return status


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(1)
    sys.exit(prepare(sys.argv[1], sys.argv[2]))

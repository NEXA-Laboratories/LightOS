#!/usr/bin/env python3
"""Create a FAT32 disk image for LightOS (optionally pre-filled from a host directory).

  python3 tools/mkdisk.py data.img [--size 64] [--root disk-root]

Attach it to QEMU as the primary-slave IDE drive:
  -drive file=data.img,format=raw,if=ide,index=1
File and directory names must be 8.3 (they are upper-cased).
"""
import os, struct, sys

BPS = 512
SPC = 1                 # 512-byte clusters: 64 MiB gives >65525 clusters, as FAT32 requires
RESERVED = 32
NFATS = 2
EOC = 0x0FFFFFFF
VALID = set("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_~!#$%&(){}@^")


def to_83(name):
    up = name.upper()
    base, _, ext = up.rpartition(".") if "." in up else (up, "", "")
    if "." in up and not base:
        raise SystemExit(f"bad name {name!r}")
    if not base or len(base) > 8 or len(ext) > 3 or not set(base + ext) <= VALID:
        raise SystemExit(f"{name!r} is not a valid 8.3 name")
    return (base.ljust(8) + ext.ljust(3)).encode("ascii")


class Image:
    def __init__(self, path, size_mib):
        self.path = path
        self.total = size_mib * 1024 * 1024 // BPS
        fat = 1
        while True:  # smallest FAT that covers all clusters
            clusters = (self.total - RESERVED - NFATS * fat) // SPC
            need = -(-(clusters + 2) * 4 // BPS)
            if need <= fat:
                break
            fat = need
        self.fat_size = fat
        self.clusters = (self.total - RESERVED - NFATS * fat) // SPC
        if self.clusters < 65525:
            raise SystemExit("image too small for FAT32 (use --size 64 or more)")
        self.data_start = RESERVED + NFATS * fat
        self.fat = [0] * (self.clusters + 2)
        self.fat[0], self.fat[1], self.fat[2] = 0x0FFFFFF8, EOC, EOC
        self.next = 3
        self.f = open(path, "wb")
        self.f.truncate(self.total * BPS)

    def write_sector(self, lba, data):
        assert len(data) <= BPS
        self.f.seek(lba * BPS)
        self.f.write(data.ljust(BPS, b"\0"))

    def alloc_chain(self, n, first=None):
        chain = [first] if first is not None else []
        while len(chain) < n:
            if self.next >= self.clusters + 2:
                raise SystemExit("image full")
            chain.append(self.next)
            self.next += 1
        for a, b in zip(chain, chain[1:]):
            self.fat[a] = b
        self.fat[chain[-1]] = EOC
        return chain

    def write_chain(self, chain, data):
        for i, c in enumerate(chain):
            self.write_sector(self.data_start + (c - 2) * SPC, data[i * BPS:(i + 1) * BPS])

    @staticmethod
    def entry(name11, attr, cluster, size):
        date = ((2026 - 1980) << 9) | (10 << 5) | 17
        e = bytearray(32)
        e[0:11] = name11
        e[11] = attr
        for off in (16, 18, 24):
            struct.pack_into("<H", e, off, date)
        struct.pack_into("<H", e, 20, cluster >> 16)
        struct.pack_into("<H", e, 26, cluster & 0xFFFF)
        struct.pack_into("<I", e, 28, size)
        return bytes(e)

    def put_dir(self, host_dir, first_cluster, parent_cluster):
        items = sorted(os.listdir(host_dir)) if host_dir else []
        nclusters = max(1, -(-(2 + len(items)) * 32 // BPS))
        chain = self.alloc_chain(nclusters, first_cluster)
        entries = []
        if parent_cluster is not None:  # not the root
            entries.append(self.entry(b".          ", 0x10, chain[0], 0))
            entries.append(self.entry(b"..         ", 0x10, 0 if parent_cluster == 2 else parent_cluster, 0))
        for name in items:
            p = os.path.join(host_dir, name)
            n83 = to_83(name)
            if os.path.isdir(p):
                sub_first = self.next
                self.next += 1
                self.put_dir(p, sub_first, chain[0])
                entries.append(self.entry(n83, 0x10, sub_first, 0))
            else:
                data = open(p, "rb").read()
                if data:
                    c = self.alloc_chain(-(-len(data) // BPS))
                    self.write_chain(c, data)
                    entries.append(self.entry(n83, 0x20, c[0], len(data)))
                else:
                    entries.append(self.entry(n83, 0x20, 0, 0))
        self.write_chain(chain, b"".join(entries))

    def finish(self):
        bs = bytearray(BPS)
        bs[0:3] = b"\xEB\x58\x90"
        bs[3:11] = b"LIGHTOS "
        struct.pack_into("<HBHBHHBHHHII", bs, 11, BPS, SPC, RESERVED, NFATS, 0, 0, 0xF8, 0, 63, 255, 0, self.total)
        struct.pack_into("<IHHIHH", bs, 36, self.fat_size, 0, 0, 2, 1, 6)
        bs[64] = 0x80
        bs[66] = 0x29
        struct.pack_into("<I", bs, 67, 0x4C4F5330)
        bs[71:82] = b"LIGHTOS    "
        bs[82:90] = b"FAT32   "
        bs[510:512] = b"\x55\xAA"
        fsinfo = bytearray(BPS)
        struct.pack_into("<I", fsinfo, 0, 0x41615252)
        struct.pack_into("<I", fsinfo, 484, 0x61417272)
        struct.pack_into("<II", fsinfo, 488, 0xFFFFFFFF, 0xFFFFFFFF)
        struct.pack_into("<I", fsinfo, 508, 0xAA550000)
        for base in (0, 6):
            self.write_sector(base, bytes(bs))
            self.write_sector(base + 1, bytes(fsinfo))
        raw = b"".join(struct.pack("<I", v) for v in self.fat)
        for k in range(NFATS):
            for s in range(self.fat_size):
                self.write_sector(RESERVED + k * self.fat_size + s, raw[s * BPS:(s + 1) * BPS])
        self.f.close()


def main(argv):
    if not argv or argv[0].startswith("-"):
        print(__doc__)
        return 1
    path, size, root = argv[0], 64, None
    args = argv[1:]
    while args:
        k = args.pop(0)
        if k == "--size":
            size = int(args.pop(0))
        elif k == "--root":
            root = args.pop(0)
        else:
            print("unknown option", k)
            return 1
    img = Image(path, size)
    img.put_dir(root, 2, None)
    img.finish()
    print(f"{path}: {size} MiB FAT32, {img.clusters} clusters, {img.next - 2} used")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

"""Independent FAT32 reader (written from the spec) to validate mkdisk.py output."""
import os, struct, subprocess, sys, tempfile, unittest
HERE = os.path.dirname(os.path.abspath(__file__))

class Reader:
    def __init__(self, path):
        self.d = open(path, "rb").read()
        b = self.d
        self.bps, self.spc, self.res, self.nf = struct.unpack_from("<HBHB", b, 11)
        self.tot = struct.unpack_from("<I", b, 32)[0]
        self.fatsz, _, _, self.root = struct.unpack_from("<IHHI", b, 36)
        assert b[510:512] == b"\x55\xaa" and b[82:90] == b"FAT32   "
        assert self.d[6 * 512:7 * 512] == self.d[0:512]      # backup boot sector
        self.ds = self.res + self.nf * self.fatsz
        self.nclusters = (self.tot - self.ds) // self.spc
        assert self.nclusters >= 65525
    def fat(self, c, k=0):
        return struct.unpack_from("<I", self.d, (self.res + k * self.fatsz) * 512 + c * 4)[0] & 0x0FFFFFFF
    def chain(self, c):
        out = []
        while c and c < 0x0FFFFFF8:
            out.append(c); c = self.fat(c)
        return out
    def data(self, c):
        return b"".join(self.d[(self.ds + (x - 2) * self.spc) * 512:(self.ds + (x - 1) * self.spc) * 512] for x in self.chain(c))
    def entries(self, c):
        raw = self.data(c)
        for i in range(0, len(raw), 32):
            e = raw[i:i + 32]
            if e[0] == 0: break
            if e[0] == 0xE5 or e[11] == 0x0F: continue
            nm = e[:8].decode().strip(); ex = e[8:11].decode().strip()
            cl = (struct.unpack_from("<H", e, 20)[0] << 16) | struct.unpack_from("<H", e, 26)[0]
            yield (nm + ("." + ex if ex else ""), e[11], cl, struct.unpack_from("<I", e, 28)[0])
    def walk(self, c=None, prefix=""):
        c = c or self.root
        for name, attr, cl, size in self.entries(c):
            if name.startswith("."): continue
            if attr & 0x10:
                yield from self.walk(cl, prefix + name + "/")
            else:
                yield prefix + name, self.data(cl)[:size] if size else b""

class T(unittest.TestCase):
    def test_roundtrip(self):
        with tempfile.TemporaryDirectory() as t:
            root = os.path.join(t, "r"); os.makedirs(os.path.join(root, "bin")); os.makedirs(os.path.join(root, "bin", "deep"))
            files = {"readme.txt": b"hello\n" * 200, "bin/hello.elf": os.urandom(5000), "bin/deep/a.b": b"x", "empty.txt": b""}
            for i in range(40):  # force a multi-cluster root directory
                files[f"f{i}.dat"] = bytes([i]) * (i * 100)
            for p, d in files.items():
                open(os.path.join(root, p), "wb").write(d)
            img = os.path.join(t, "d.img")
            subprocess.check_call([sys.executable, os.path.join(HERE, "mkdisk.py"), img, "--root", root], stdout=subprocess.DEVNULL)
            r = Reader(img)
            got = dict(r.walk())
            self.assertEqual({k.upper(): v for k, v in files.items()}, got)
            for c in range(2, 400):                       # both FATs identical
                self.assertEqual(r.fat(c, 0), r.fat(c, 1))
    def test_bad_name(self):
        with tempfile.TemporaryDirectory() as t:
            os.makedirs(os.path.join(t, "r")); open(os.path.join(t, "r", "toolongname.txt"), "w").write("x")
            p = subprocess.run([sys.executable, os.path.join(HERE, "mkdisk.py"), os.path.join(t, "d.img"), "--root", os.path.join(t, "r")], capture_output=True)
            self.assertNotEqual(p.returncode, 0)

if __name__ == "__main__":
    unittest.main()

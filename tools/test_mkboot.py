import os, tempfile, unittest, mkboot

class T(unittest.TestCase):
    def run_one(self, data):
        with tempfile.TemporaryDirectory() as t:
            src, dst = os.path.join(t, "a.bin"), os.path.join(t, "o", "b.img")
            open(src, "wb").write(data)
            rc = mkboot.prepare(src, dst)
            return rc, (os.path.getsize(dst) if os.path.exists(dst) else None)
    def img(self, n, table=False):
        b = bytearray(n); b[510:512] = b"\x55\xaa"
        if table: b[446] = 0x80
        return bytes(b)
    def test_ok_and_padding(self):
        rc, size = self.run_one(self.img(700_000))
        self.assertEqual((rc, size), (0, 1024 * 1024))
    def test_occupied(self):
        self.assertEqual(self.run_one(self.img(4096, True))[0], 3)
    def test_bad_signature_and_too_big(self):
        self.assertEqual(self.run_one(b"\0" * 1024)[0], 1)
        self.assertEqual(self.run_one(self.img(9 * 1024 * 1024))[0], 1)

if __name__ == "__main__":
    unittest.main()

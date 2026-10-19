import json, os, shutil, socket, tempfile, threading, unittest, urllib.request, urllib.error
import los_server as S

REPO = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "repo")

def mkman(**kw):
    d = dict(name="my-app", version="1.0.0", description="My app", author="Me", license_="MIT", verified=False, deps=[("los-core", "0.1.0")])
    d.update(kw)
    return S.Manifest(**d)

class Core(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        shutil.copytree(REPO, os.path.join(self.tmp, "r"))
        self.repo = S.Repo(os.path.join(self.tmp, "r"))
    def tearDown(self):
        shutil.rmtree(self.tmp)

    def test_hash_vectors(self):
        self.assertEqual(S.fnv1a64(b""), 0xcbf29ce484222325)
        self.assertEqual(S.fnv1a64(b"a"), 0xaf63dc4c8601ec8c)

    def test_seed_repo(self):
        names = [p.manifest.name for p in self.repo.all()]
        self.assertEqual(names, ["crash-elf", "demo-app", "hello-elf", "los-core", "los-desktop", "los-edit", "los-net", "los-shell", "priv-elf", "spin-elf"])
        self.assertFalse(self.repo.get("demo-app").manifest.verified)
        self.assertTrue(self.repo.get("los-core").manifest.verified)

    def test_submit_ok_starts_unverified_and_persists(self):
        ok, e, w = self.repo.submit(mkman(verified=True), "no_std app payload\n")
        self.assertTrue(ok, e)
        self.assertFalse(self.repo.get("my-app").manifest.verified)
        again = S.Repo(self.repo.path)
        self.assertEqual(again.get("my-app").hash, self.repo.get("my-app").hash)

    def test_security_rejections(self):
        for kw, payload in [(dict(license_="GPL"), "x"), (dict(name="Bad Name"), "x"), (dict(deps=[("nope", "1")]), "x"),
                            (dict(description="a|b"), "x"), (dict(version="1.0"), "x"), ({}, ""), ({}, "caf\u00e9")]:
            ok, e, _ = self.repo.submit(mkman(**kw), payload)
            self.assertFalse(ok, (kw, payload))
            self.assertTrue(e)

    def test_version_must_increase(self):
        self.assertTrue(self.repo.submit(mkman(), "p")[0])
        self.assertFalse(self.repo.submit(mkman(), "p2")[0])
        self.assertTrue(self.repo.submit(mkman(version="1.0.1"), "p2")[0])

    def test_cycle_detected(self):
        self.repo.submit(mkman(name="aa", deps=[]), "p")
        self.repo.submit(mkman(name="bb", deps=[("aa", "1")]), "p")
        ok, e, _ = self.repo.submit(mkman(name="aa", version="1.0.1", deps=[("bb", "1")]), "p")
        self.assertFalse(ok)
        self.assertIn("cycle", " ".join(e))

    def test_verify_and_remove_rules(self):
        self.repo.set_verified("demo-app", True)
        self.assertTrue(S.Repo(self.repo.path).get("demo-app").manifest.verified)
        with self.assertRaises(ValueError):
            self.repo.remove("los-core")
        self.repo.remove("demo-app")
        self.assertIsNone(self.repo.get("demo-app"))

    def test_search(self):
        self.assertEqual([p.manifest.name for p in self.repo.search("editor")], ["los-edit"])

    def test_bridge_protocol(self):
        lines = S.bridge_reply(self.repo, "LIST")
        self.assertEqual(lines[-1], "END")
        self.assertEqual(len(lines), 11)
        f = lines[0].split("|")
        self.assertEqual((f[0], len(f)), ("PKG", 9))
        get = S.bridge_reply(self.repo, "GET los-edit")[0]
        self.assertTrue(get.startswith("DATA|los-edit|los-edit 0.1.0\\ncore") is False)
        self.assertIn("\\n", get)
        self.assertTrue(S.bridge_reply(self.repo, "GET ../etc")[0].startswith("ERR|"))
        self.assertEqual(S.bridge_reply(self.repo, "PING"), ["PONG"])

    def test_kernel_side_hash_matches(self):
        # The kernel hashes the unescaped payload; make sure escape/unescape round-trips.
        p = self.repo.get("los-core")
        esc = S.escape_payload(p.payload)
        back = esc.replace("\\n", "\n").replace("\\\\", "\\")
        self.assertEqual(S.fnv1a64(back.encode()), p.hash)

class Live(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp()
        shutil.copytree(REPO, os.path.join(cls.tmp, "r"))
        cls.repo = S.Repo(os.path.join(cls.tmp, "r"))
        cls.httpd = S.ThreadingHTTPServer(("127.0.0.1", 0), S.make_http_handler(cls.repo, "tok"))
        cls.bridge = S.ThreadingTCP(("127.0.0.1", 0), S.make_bridge_handler(cls.repo))
        for s in (cls.httpd, cls.bridge):
            threading.Thread(target=s.serve_forever, daemon=True).start()
        cls.base = f"http://127.0.0.1:{cls.httpd.server_address[1]}"
    @classmethod
    def tearDownClass(cls):
        cls.httpd.shutdown(); cls.bridge.shutdown(); shutil.rmtree(cls.tmp)

    def req(self, method, path, body=None, headers=None):
        r = urllib.request.Request(self.base + path, data=body.encode() if body else None, method=method, headers=headers or {})
        try:
            with urllib.request.urlopen(r) as f:
                return f.status, f.read().decode()
        except urllib.error.HTTPError as e:
            return e.code, e.read().decode()

    def test_http_flow(self):
        self.assertEqual(self.req("GET", "/health")[0], 200)
        c, b = self.req("GET", "/api/v1/packages")
        self.assertEqual(len(json.loads(b)), 10)
        self.assertEqual(self.req("GET", "/api/v1/packages/los-core/payload")[1], self.repo.get("los-core").payload)
        c, txt = self.req("GET", "/api/v1/index.txt")
        self.assertEqual(c, 200)
        self.assertEqual(len(txt.strip().split("\n")), 10)
        self.assertTrue(all(l.startswith("PKG|") and len(l.split("|")) == 9 for l in txt.strip().split("\n")))
        self.assertEqual(self.req("GET", "/api/v1/packages/zzz")[0], 404)
        self.assertEqual(json.loads(self.req("GET", "/api/v1/search?q=net")[1])[0]["name"], "los-net")
        sub = open(os.path.join(REPO, "..", "examples", "hello-app", "package.toml")).read() + "\n---payload---\nno_std hello\n"
        c, b = self.req("POST", "/api/v1/submit", sub)
        self.assertEqual(c, 201, b)
        self.assertEqual(self.req("POST", "/api/v1/verify/my-app")[0], 403)
        self.assertEqual(self.req("POST", "/api/v1/verify/my-app", "", {"X-Admin-Token": "tok"})[0], 200)
        self.assertTrue(self.repo.get("my-app").manifest.verified)
        self.assertEqual(self.req("DELETE", "/api/v1/packages/los-core", None, {"X-Admin-Token": "tok"})[0], 409)
        self.assertEqual(self.req("POST", "/api/v1/submit", "garbage")[0], 400)

    def test_bridge_over_tcp(self):
        s = socket.create_connection(("127.0.0.1", self.bridge.server_address[1]), timeout=5)
        f = s.makefile("rwb")
        f.write(b"[OK] GDT initialized\nLOS/1 LIST\n"); f.flush()
        got = []
        while True:
            line = f.readline().decode().strip()
            got.append(line)
            if line == "END":
                break
        self.assertEqual(len(got), 11)
        s.close()

if __name__ == "__main__":
    unittest.main()

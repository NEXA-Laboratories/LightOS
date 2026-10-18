#!/usr/bin/env python3
"""
los-server 0.1.0 - the LightOS package repository server.

Zero dependencies (Python 3.8+ standard library only).

  python3 los_server.py serve [--repo ../repo] [--http 127.0.0.1:8080]
                              [--bridge 127.0.0.1:7878] [--token SECRET]
  python3 los_server.py submit <dir>      # dir has package.toml + payload.txt
  python3 los_server.py verify <name>     # maintainer: mark reviewed/verified
  python3 los_server.py unverify <name>
  python3 los_server.py remove <name>
  python3 los_server.py list

Two front doors share one repository:
  * HTTP JSON API (browsers, curl, tooling)
  * Serial bridge (TCP): QEMU's COM1 (-serial tcp:127.0.0.1:7878) speaks the
    line protocol used by `los update` / `los install` inside the kernel.
"""
import json
import os
import re
import shutil
import socketserver
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse

VERSION = "0.4.0"
MAX_PAYLOAD = 32 * 1024
MAX_BODY = 256 * 1024
NAME_RE = re.compile(r"^[a-z0-9][a-z0-9-]{1,47}$")
VERSION_RE = re.compile(r"^\d+\.\d+\.\d+$")
LICENSES = {"MIT", "Apache-2.0", "MIT OR Apache-2.0"}
PAYLOAD_SEP = "---payload---"


# ----------------------------------------------------------------------- hashing
def fnv1a64(data: bytes) -> int:
    h = 0xCBF29CE484222325
    for b in data:
        h ^= b
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


# ---------------------------------------------------------------------- manifest
class Manifest:
    def __init__(self, name, version, description, author, license_, verified, deps):
        self.name, self.version, self.description = name, version, description
        self.author, self.license, self.verified, self.deps = author, license_, verified, deps

    def to_toml(self) -> str:
        out = ["[package]", f'name = "{self.name}"', f'version = "{self.version}"',
               f'description = "{self.description}"', f'author = "{self.author}"',
               f'license = "{self.license}"', f'verified = {"true" if self.verified else "false"}',
               "", "[dependencies]"]
        out += [f'{k} = "{v}"' for k, v in self.deps]
        return "\n".join(out) + "\n"


def _strip_comment(line: str) -> str:
    in_q = False
    for i, ch in enumerate(line):
        if ch == '"':
            in_q = not in_q
        elif ch == "#" and not in_q:
            return line[:i]
    return line


def parse_manifest(text: str) -> Manifest:
    section, pkg, deps = "", {}, []
    for n, raw in enumerate(text.splitlines(), 1):
        line = _strip_comment(raw).strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip()
            continue
        if "=" not in line:
            raise ValueError(f"package.toml line {n}: expected key = value")
        k, v = (s.strip() for s in line.split("=", 1))
        if len(v) >= 2 and v[0] == v[-1] == '"':
            v = v[1:-1]
        if section == "package":
            pkg[k] = v
        elif section == "dependencies":
            deps.append((k, v))
    for req in ("name", "version"):
        if not pkg.get(req):
            raise ValueError(f"package.toml: missing '{req}'")
    return Manifest(pkg["name"], pkg["version"], pkg.get("description", ""), pkg.get("author", ""),
                    pkg.get("license", ""), pkg.get("verified", "false") == "true", deps)


def vtuple(v: str):
    return tuple(int(x) for x in v.split("."))


# -------------------------------------------------------------- automatic review
def security_check(m: Manifest, payload: str, known: set):
    """Automatic checks run on every submission. Returns (errors, warnings)."""
    errors, warnings = [], []
    if not NAME_RE.match(m.name):
        errors.append("name must be 2-48 chars of [a-z0-9-] and start with a letter/digit")
    if not VERSION_RE.match(m.version):
        errors.append("version must look like MAJOR.MINOR.PATCH")
    if m.license not in LICENSES:
        errors.append("license must be one of: " + ", ".join(sorted(LICENSES)))
    if not m.description or len(m.description) > 120:
        errors.append("description is required and must be <= 120 chars")
    if not m.author or len(m.author) > 60:
        errors.append("author is required and must be <= 60 chars")
    for field in (m.description, m.author):
        if "|" in field or "\n" in field or '"' in field:
            errors.append('description/author may not contain |, " or newlines')
            break
    for dep, _ in m.deps:
        if dep == m.name:
            errors.append("package depends on itself")
        elif dep not in known:
            errors.append(f"dependency '{dep}' is not in the repository")
    if not payload.strip():
        errors.append("payload is empty")
    if len(payload.encode("utf-8", "replace")) > MAX_PAYLOAD:
        errors.append(f"payload exceeds {MAX_PAYLOAD} bytes")
    if any(ord(c) > 126 or (ord(c) < 32 and c not in "\n\t") for c in payload):
        errors.append("payload must be plain ASCII text (no control or non-ASCII characters)")
    if re.search(r"\bunsafe\b", payload):
        warnings.append("payload contains 'unsafe' - reviewer will look closely")
    if "no_std" not in payload and "#![no_std]" not in payload:
        warnings.append("payload does not mention no_std - make sure the package works in a no_std environment")
    return errors, warnings


# ------------------------------------------------------------------- repository
class Package:
    def __init__(self, manifest: Manifest, payload: str):
        self.manifest, self.payload = manifest, payload
        self.hash = fnv1a64(payload.encode("ascii", "replace"))

    def to_dict(self):
        m = self.manifest
        return {"name": m.name, "version": m.version, "description": m.description,
                "author": m.author, "license": m.license, "verified": m.verified,
                "dependencies": [d for d, _ in m.deps], "hash": f"{self.hash:016x}",
                "size": len(self.payload)}

    def index_line(self) -> str:
        m = self.manifest
        return "PKG|" + "|".join([m.name, m.version, m.author, m.description,
                                  "1" if m.verified else "0", ",".join(d for d, _ in m.deps),
                                  f"{self.hash:016x}", str(len(self.payload))])


def escape_payload(s: str) -> str:
    return s.replace("\\", "\\\\").replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")


class Repo:
    def __init__(self, path):
        self.path = os.path.abspath(path)
        self.lock = threading.RLock()
        self.packages = {}
        os.makedirs(self.path, exist_ok=True)
        for name in sorted(os.listdir(self.path)):
            d = os.path.join(self.path, name)
            tp, pp = os.path.join(d, "package.toml"), os.path.join(d, "payload.txt")
            if os.path.isfile(tp) and os.path.isfile(pp):
                with open(tp, encoding="utf-8") as f, open(pp, encoding="utf-8", newline="") as g:
                    self.packages[name] = Package(parse_manifest(f.read()), g.read())

    def _write(self, p: Package):
        d = os.path.join(self.path, p.manifest.name)
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "package.toml"), "w", encoding="utf-8") as f:
            f.write(p.manifest.to_toml())
        with open(os.path.join(d, "payload.txt"), "w", encoding="utf-8", newline="") as f:
            f.write(p.payload)

    def _reaches(self, start, target, deps_override):
        seen, stack = set(), [start]
        while stack:
            cur = stack.pop()
            if cur == target:
                return True
            if cur in seen:
                continue
            seen.add(cur)
            deps = deps_override.get(cur)
            if deps is None and cur in self.packages:
                deps = [d for d, _ in self.packages[cur].manifest.deps]
            stack.extend(deps or [])
        return False

    def submit(self, manifest: Manifest, payload: str):
        """Returns (ok, errors, warnings). New packages always start unverified."""
        with self.lock:
            errors, warnings = security_check(manifest, payload, set(self.packages))
            old = self.packages.get(manifest.name)
            if old and not errors and vtuple(manifest.version) <= vtuple(old.manifest.version):
                errors.append(f"version {manifest.version} must be greater than published {old.manifest.version}")
            if not errors:
                override = {manifest.name: [d for d, _ in manifest.deps]}
                if any(self._reaches(d, manifest.name, override) for d, _ in manifest.deps):
                    errors.append("dependency cycle detected")
            if errors:
                return False, errors, warnings
            manifest.verified = False
            pkg = Package(manifest, payload)
            self._write(pkg)
            self.packages[manifest.name] = pkg
            return True, [], warnings

    def set_verified(self, name: str, value: bool):
        with self.lock:
            p = self.packages.get(name)
            if not p:
                raise KeyError(name)
            p.manifest.verified = value
            self._write(p)

    def remove(self, name: str):
        with self.lock:
            if name not in self.packages:
                raise KeyError(name)
            users = [n for n, p in self.packages.items() if n != name and any(d == name for d, _ in p.manifest.deps)]
            if users:
                raise ValueError("required by: " + ", ".join(sorted(users)))
            shutil.rmtree(os.path.join(self.path, name), ignore_errors=True)
            del self.packages[name]

    def search(self, q: str):
        q = q.lower()
        with self.lock:
            return [p for p in self.packages.values()
                    if q in p.manifest.name.lower() or q in p.manifest.description.lower()]

    def all(self):
        with self.lock:
            return sorted(self.packages.values(), key=lambda p: p.manifest.name)

    def get(self, name):
        with self.lock:
            return self.packages.get(name)


def split_submission(body: str):
    if PAYLOAD_SEP not in body:
        raise ValueError(f"body must be package.toml, a line '{PAYLOAD_SEP}', then the payload")
    head, payload = body.split(PAYLOAD_SEP, 1)
    return head, payload[1:] if payload.startswith("\n") else payload


# --------------------------------------------------------------------------- HTTP
def make_http_handler(repo: Repo, token: str):
    class Handler(BaseHTTPRequestHandler):
        server_version = f"los-server/{VERSION}"

        def log_message(self, fmt, *args):
            sys.stderr.write("[http] " + (fmt % args) + "\n")

        def _send(self, code, body, ctype="application/json"):
            data = body if isinstance(body, bytes) else body.encode()
            self.send_response(code)
            self.send_header("Content-Type", ctype + "; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()
            self.wfile.write(data)

        def _json(self, code, obj):
            self._send(code, json.dumps(obj, indent=2) + "\n")

        def _admin(self):
            if self.headers.get("X-Admin-Token", "") != token:
                self._json(403, {"ok": False, "error": "missing or wrong X-Admin-Token"})
                return False
            return True

        def do_GET(self):
            u = urlparse(self.path)
            parts = [unquote(p) for p in u.path.strip("/").split("/") if p]
            if not parts:
                return self._send(200, f"LightOS los-server {VERSION}\nGET /api/v1/packages\n", "text/plain")
            if parts == ["health"]:
                return self._json(200, {"status": "ok", "version": VERSION, "packages": len(repo.all())})
            if parts[:3] == ["api", "v1", "packages"]:
                if len(parts) == 3:
                    return self._json(200, [p.to_dict() for p in repo.all()])
                p = repo.get(parts[3])
                if not p:
                    return self._json(404, {"ok": False, "error": "package not found"})
                if len(parts) == 4:
                    return self._json(200, p.to_dict())
                if len(parts) == 5 and parts[4] == "payload":
                    return self._send(200, p.payload, "text/plain")
            if parts == ["api", "v1", "search"]:
                q = parse_qs(u.query).get("q", [""])[0]
                return self._json(200, [p.to_dict() for p in repo.search(q)])
            self._json(404, {"ok": False, "error": "not found"})

        def do_POST(self):
            parts = [p for p in urlparse(self.path).path.strip("/").split("/") if p]
            length = int(self.headers.get("Content-Length", "0") or 0)
            if length > MAX_BODY:
                return self._json(413, {"ok": False, "error": "body too large"})
            body = self.rfile.read(length).decode("utf-8", "replace")
            if parts == ["api", "v1", "submit"]:
                try:
                    head, payload = split_submission(body)
                    manifest = parse_manifest(head)
                except ValueError as e:
                    return self._json(400, {"ok": False, "errors": [str(e)]})
                ok, errors, warnings = repo.submit(manifest, payload)
                code = 201 if ok else 422
                return self._json(code, {"ok": ok, "errors": errors, "warnings": warnings,
                                         "status": "pending manual review" if ok else "rejected"})
            if len(parts) == 4 and parts[:3] == ["api", "v1", "verify"]:
                if not self._admin():
                    return
                try:
                    repo.set_verified(parts[3], True)
                except KeyError:
                    return self._json(404, {"ok": False, "error": "package not found"})
                return self._json(200, {"ok": True, "verified": parts[3]})
            self._json(404, {"ok": False, "error": "not found"})

        def do_DELETE(self):
            parts = [p for p in urlparse(self.path).path.strip("/").split("/") if p]
            if len(parts) == 4 and parts[:3] == ["api", "v1", "packages"]:
                if not self._admin():
                    return
                try:
                    repo.remove(parts[3])
                except KeyError:
                    return self._json(404, {"ok": False, "error": "package not found"})
                except ValueError as e:
                    return self._json(409, {"ok": False, "error": str(e)})
                return self._json(200, {"ok": True})
            self._json(404, {"ok": False, "error": "not found"})

    return Handler


# ------------------------------------------------------------------- serial bridge
def bridge_reply(repo: Repo, cmd: str):
    """Handle one 'LOS/1 ...' line from the guest; returns list of reply lines."""
    verb, _, arg = cmd.partition(" ")
    arg = arg.strip()
    if verb == "PING":
        return ["PONG"]
    if verb == "LIST":
        return [p.index_line() for p in repo.all()] + ["END"]
    if verb == "SEARCH":
        return [p.index_line() for p in repo.search(arg)] + ["END"]
    if verb == "GET":
        p = repo.get(arg) if NAME_RE.match(arg) else None
        if not p:
            return ["ERR|package not found"]
        return [f"DATA|{p.manifest.name}|{escape_payload(p.payload)}"]
    return ["ERR|unknown command"]


def make_bridge_handler(repo: Repo):
    class Bridge(socketserver.StreamRequestHandler):
        def handle(self):
            sys.stderr.write(f"[bridge] guest connected from {self.client_address[0]}\n")
            for raw in self.rfile:
                line = raw.decode("utf-8", "replace").rstrip("\r\n")
                if line.startswith("LOS/1 "):
                    sys.stderr.write(f"[bridge] <- {line}\n")
                    for out in bridge_reply(repo, line[6:]):
                        self.wfile.write((out + "\n").encode())
                    self.wfile.flush()
                elif line:
                    sys.stderr.write(f"[guest] {line}\n")
            sys.stderr.write("[bridge] guest disconnected\n")

    return Bridge


class ThreadingTCP(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True


# ---------------------------------------------------------------------------- CLI
def hostport(s):
    h, _, p = s.rpartition(":")
    return (h or "127.0.0.1", int(p))


def take(args, flag, default):
    if flag in args:
        i = args.index(flag)
        v = args[i + 1]
        del args[i:i + 2]
        return v
    return default


def main(argv):
    args = list(argv)
    if not args or args[0] in ("-h", "--help", "help"):
        print(__doc__)
        return 0
    cmd = args.pop(0)
    default_repo = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "repo")
    repo = Repo(take(args, "--repo", default_repo))

    if cmd == "serve":
        http_addr = hostport(take(args, "--http", "127.0.0.1:8080"))
        bridge_addr = hostport(take(args, "--bridge", "127.0.0.1:7878"))
        token = take(args, "--token", os.environ.get("LOS_ADMIN_TOKEN", ""))
        if not token:
            token = "changeme"
            print("WARNING: using the default admin token 'changeme'; set LOS_ADMIN_TOKEN", file=sys.stderr)
        bridge = ThreadingTCP(bridge_addr, make_bridge_handler(repo))
        threading.Thread(target=bridge.serve_forever, daemon=True).start()
        httpd = ThreadingHTTPServer(http_addr, make_http_handler(repo, token))
        print(f"los-server {VERSION}: {len(repo.all())} package(s) from {repo.path}")
        print(f"  HTTP   http://{http_addr[0]}:{http_addr[1]}/api/v1/packages")
        print(f"  Serial bridge tcp://{bridge_addr[0]}:{bridge_addr[1]}  (qemu -serial tcp:{bridge_addr[0]}:{bridge_addr[1]})")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nbye")
        return 0

    if cmd == "list":
        for p in repo.all():
            m = p.manifest
            print(f"{'v' if m.verified else '?'} {m.name:<14} {m.version:<8} {m.description}")
        return 0

    if cmd == "submit" and args:
        d = args[0]
        try:
            with open(os.path.join(d, "package.toml"), encoding="utf-8") as f:
                manifest = parse_manifest(f.read())
            with open(os.path.join(d, "payload.txt"), encoding="utf-8", newline="") as f:
                payload = f.read()
        except (OSError, ValueError) as e:
            print(f"submit: {e}", file=sys.stderr)
            return 1
        ok, errors, warnings = repo.submit(manifest, payload)
        for w in warnings:
            print(f"warning: {w}")
        for e in errors:
            print(f"error: {e}", file=sys.stderr)
        if ok:
            print(f"accepted {manifest.name} {manifest.version} - pending manual review (run: verify {manifest.name})")
        return 0 if ok else 1

    if cmd in ("verify", "unverify") and args:
        try:
            repo.set_verified(args[0], cmd == "verify")
        except KeyError:
            print("no such package", file=sys.stderr)
            return 1
        print(f"{args[0]}: {'verified' if cmd == 'verify' else 'unverified'}")
        return 0

    if cmd == "remove" and args:
        try:
            repo.remove(args[0])
        except (KeyError, ValueError) as e:
            print(f"remove: {e}", file=sys.stderr)
            return 1
        print(f"removed {args[0]}")
        return 0

    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

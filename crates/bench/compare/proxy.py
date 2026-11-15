#!/usr/bin/env python3
"""Record/replay HTTPS proxy over dasher's DASHER_RECORD store format.

usage: proxy.py --port P --ca DIR --store DIR [--record]

Serves every request from DIR (the `*.meta` / `*.body` pairs dasher's
`replay` feature writes), matching as dasher's replay does: method and URL,
then method, scheme, host and path. A recorded response that came at the end
of redirects answers its request URL with a 302 to where it ended, and the
end URL with the response.

With --record, a request the store cannot answer is fetched from the
network (following redirects), written to the store in the same format and
served. Without it, a miss closes the connection, which a client sees as a
network error, as dasher's replay refuses one.

TLS is terminated with a certificate for each host signed by the CA in
--ca (ca.pem / ca.key, created if missing); give the client ca.pem.
"""

import argparse
import datetime
import gzip
import hashlib
import http.client
import os
import socket
import socketserver
import ssl
import sys
import threading
import zlib
from urllib.parse import urlsplit, urljoin

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

HOP = {
    "connection", "keep-alive", "proxy-connection", "proxy-authorization",
    "te", "trailer", "transfer-encoding", "upgrade", "content-length",
    "content-encoding",
}


class Store:
    def __init__(self, root, record):
        self.root = root
        self.record = record
        self.lock = threading.Lock()
        self.exact = {}
        self.by_path = {}
        self.final = {}
        os.makedirs(root, exist_ok=True)
        for name in sorted(os.listdir(root)):
            if name.endswith(".meta"):
                self._index(os.path.join(root, name))

    @staticmethod
    def path_key(method, url):
        s = urlsplit(url)
        return f"{method} {s.scheme}://{s.hostname or ''}{s.path or '/'}"

    def _index(self, meta):
        try:
            with open(meta, encoding="utf-8") as f:
                lines = f.read().split("\n")
            method, url, _status, final, redirected = lines[:5]
        except (OSError, ValueError, UnicodeDecodeError):
            return
        url = norm(url)
        self.by_path.setdefault(self.path_key(method, url), meta)
        self.exact[f"{method} {url}"] = meta
        if redirected == "1":
            self.final.setdefault(f"{method} {norm(final)}", meta)

    def find(self, method, url):
        """(meta path, served as the redirect's end) or None."""
        key = f"{method} {url}"
        with self.lock:
            if key in self.exact:
                return self.exact[key], False
            if key in self.final:
                return self.final[key], True
            meta = self.by_path.get(self.path_key(method, url))
        return (meta, False) if meta else None

    def add(self, method, url, status, final, redirected, headers, body):
        stem = "lp" + hashlib.sha1(f"{method} {url}".encode()).hexdigest()[:16]
        meta = os.path.join(self.root, stem + ".meta")
        text = f"{method}\n{url}\n{status}\n{final}\n{1 if redirected else 0}\n"
        for name, value in headers:
            if name.lower() in HOP or "\n" in value or "\r" in value:
                continue
            text += f"{name.lower()}: {value}\n"
        with self.lock:
            with open(os.path.join(self.root, stem + ".body"), "wb") as f:
                f.write(body)
            with open(meta, "w", encoding="utf-8") as f:
                f.write(text)
            self._index(meta)
        return meta


def norm(url):
    """URLs as the url crate prints them: an empty path is '/'."""
    s = urlsplit(url)
    if s.path == "" and s.scheme in ("http", "https"):
        url = f"{s.scheme}://{s.netloc}/" + (f"?{s.query}" if s.query else "")
    return url


def read_meta(meta):
    with open(meta, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    status, final, redirected = int(lines[2]), lines[3], lines[4] == "1"
    headers = []
    for line in lines[5:]:
        if ": " in line:
            n, v = line.split(": ", 1)
            headers.append((n, v))
    with open(meta[:-5] + ".body", "rb") as f:
        body = f.read()
    return status, final, redirected, headers, body


def decode(body, encoding):
    enc = (encoding or "").strip().lower()
    if enc in ("", "identity"):
        return body
    if enc in ("gzip", "x-gzip"):
        return gzip.decompress(body)
    if enc == "deflate":
        try:
            return zlib.decompress(body)
        except zlib.error:
            return zlib.decompress(body, -zlib.MAX_WBITS)
    raise ValueError(f"encoding {enc}")


def fetch_live(method, url, headers, body):
    """Fetch following redirects: (status, final url, redirected, headers, body)."""
    redirected = False
    for _ in range(10):
        s = urlsplit(url)
        port = s.port or (443 if s.scheme == "https" else 80)
        if s.scheme == "https":
            conn = http.client.HTTPSConnection(s.hostname, port, timeout=20,
                                               context=ssl.create_default_context())
        else:
            conn = http.client.HTTPConnection(s.hostname, port, timeout=20)
        path = s.path or "/"
        if s.query:
            path += "?" + s.query
        out = {}
        for n, v in headers:
            if n.lower() in HOP or n.lower() in ("host", "accept-encoding"):
                continue
            out[n] = v
        out["Host"] = s.netloc
        out["Accept-Encoding"] = "gzip, deflate"
        conn.request(method, path, body=body, headers=out)
        r = conn.getresponse()
        data = r.read()
        hdrs = r.getheaders()
        conn.close()
        if r.status in (301, 302, 303, 307, 308) and r.getheader("location"):
            url = norm(urljoin(url, r.getheader("location")))
            redirected = True
            if r.status == 303 or (r.status in (301, 302) and method == "POST"):
                method, body = "GET", None
            continue
        data = decode(data, r.getheader("content-encoding"))
        return r.status, url, redirected, hdrs, data
    raise RuntimeError("too many redirects")


class CA:
    def __init__(self, d):
        os.makedirs(d, exist_ok=True)
        kp, cp = os.path.join(d, "ca.key"), os.path.join(d, "ca.pem")
        if not os.path.exists(cp):
            key = ec.generate_private_key(ec.SECP256R1())
            name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "dasher bench CA")])
            now = datetime.datetime.now(datetime.timezone.utc)
            cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
                    .public_key(key.public_key()).serial_number(x509.random_serial_number())
                    .not_valid_before(now - datetime.timedelta(days=1))
                    .not_valid_after(now + datetime.timedelta(days=3650))
                    .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
                    .add_extension(x509.KeyUsage(digital_signature=True, key_cert_sign=True, crl_sign=True,
                                                 content_commitment=False, key_encipherment=False,
                                                 data_encipherment=False, key_agreement=False,
                                                 encipher_only=False, decipher_only=False), critical=True)
                    .sign(key, hashes.SHA256()))
            with open(kp, "wb") as f:
                f.write(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                          serialization.NoEncryption()))
            with open(cp, "wb") as f:
                f.write(cert.public_bytes(serialization.Encoding.PEM))
        with open(kp, "rb") as f:
            self.key = serialization.load_pem_private_key(f.read(), None)
        with open(cp, "rb") as f:
            self.cert = x509.load_pem_x509_certificate(f.read())
        self.dir = os.path.join(d, "hosts")
        os.makedirs(self.dir, exist_ok=True)
        self.leaf_key = ec.generate_private_key(ec.SECP256R1())
        self.leaf_key_path = os.path.join(self.dir, f"leaf-{os.getpid()}.key")
        with open(self.leaf_key_path, "wb") as f:
            f.write(self.leaf_key.private_bytes(serialization.Encoding.PEM,
                                                serialization.PrivateFormat.PKCS8,
                                                serialization.NoEncryption()))
        self.contexts = {}
        self.lock = threading.Lock()

    def context(self, host):
        with self.lock:
            if host in self.contexts:
                return self.contexts[host]
            now = datetime.datetime.now(datetime.timezone.utc)
            try:
                import ipaddress
                san = x509.IPAddress(ipaddress.ip_address(host))
            except ValueError:
                san = x509.DNSName(host)
            cert = (x509.CertificateBuilder()
                    .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, host[:64])]))
                    .issuer_name(self.cert.subject).public_key(self.leaf_key.public_key())
                    .serial_number(x509.random_serial_number())
                    .not_valid_before(now - datetime.timedelta(days=1))
                    .not_valid_after(now + datetime.timedelta(days=365))
                    .add_extension(x509.SubjectAlternativeName([san]), critical=False)
                    .sign(self.key, hashes.SHA256()))
            path = os.path.join(self.dir, hashlib.sha1(host.encode()).hexdigest() + ".pem")
            with open(path, "wb") as f:
                f.write(cert.public_bytes(serialization.Encoding.PEM))
                f.write(self.cert.public_bytes(serialization.Encoding.PEM))
            ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            ctx.load_cert_chain(path, self.leaf_key_path)
            ctx.set_alpn_protocols(["http/1.1"])
            self.contexts[host] = ctx
            return ctx


class Handler(socketserver.StreamRequestHandler):
    timeout = 120

    def handle(self):
        try:
            line = self.rfile.readline(65536).decode("latin-1").strip()
            if not line:
                return
            method, target, _ = line.split(" ", 2)
            headers = self.read_headers(self.rfile)
            if method == "CONNECT":
                host, _, port = target.rpartition(":")
                self.wfile.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
                self.wfile.flush()
                tls = self.server.ca.context(host).wrap_socket(self.connection, server_side=True)
                rfile = tls.makefile("rb")
                origin = "https://" + host + ("" if port == "443" else ":" + port)
                self.serve(tls, rfile, origin, None)
            else:
                self.serve(self.connection, self.rfile, None, (method, target, headers))
        except (OSError, ssl.SSLError, ValueError):
            pass

    @staticmethod
    def read_headers(rfile):
        headers = []
        while True:
            h = rfile.readline(65536).decode("latin-1")
            if h in ("\r\n", "\n", ""):
                return headers
            n, _, v = h.partition(":")
            headers.append((n.strip(), v.strip()))

    def serve(self, sock, rfile, origin, first):
        while True:
            if first:
                method, target, headers = first
                first = None
            else:
                line = rfile.readline(65536).decode("latin-1").strip()
                if not line:
                    return
                method, target, _ = line.split(" ", 2)
                headers = self.read_headers(rfile)
            hmap = {n.lower(): v for n, v in headers}
            body = None
            if "content-length" in hmap:
                body = rfile.read(int(hmap["content-length"]))
            elif hmap.get("transfer-encoding", "").lower() == "chunked":
                body = b""
                while True:
                    size = int(rfile.readline().strip().split(b";")[0], 16)
                    if size == 0:
                        rfile.readline()
                        break
                    body += rfile.read(size)
                    rfile.readline()
            url = norm(target if origin is None else origin + target)
            if not self.respond(sock, method, url, headers, body):
                return
            if hmap.get("connection", "").lower() == "close":
                return

    def respond(self, sock, method, url, headers, body):
        store = self.server.store
        found = store.find(method, url)
        if found is None and store.record:
            try:
                status, final, redirected, hdrs, data = fetch_live(method, url, headers, body)
                meta = store.add(method, url, status, final, redirected, hdrs, data)
                found = (meta, False)
                log(f"record {method} {url} {status}")
            except Exception as e:  # noqa: BLE001 - any failure is a network error
                log(f"fail {method} {url} {e!r}")
        if found is None:
            log(f"miss {method} {url}")
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            return False
        meta, at_end = found
        status, final, redirected, hdrs, data = read_meta(meta)
        if redirected and not at_end and norm(final) != url:
            out = (f"HTTP/1.1 302 Found\r\nLocation: {final}\r\nContent-Length: 0\r\n\r\n")
            sock.sendall(out.encode("latin-1"))
            log(f"hit {method} {url} -> {final}")
            return True
        reason = http.client.responses.get(status, "OK")
        out = f"HTTP/1.1 {status} {reason}\r\n"
        for n, v in hdrs:
            if n.lower() in HOP:
                continue
            out += f"{n}: {v}\r\n"
        if method == "HEAD" or status in (204, 304) or 100 <= status < 200:
            out += "Content-Length: 0\r\n\r\n"
            sock.sendall(out.encode("latin-1", "replace"))
        else:
            out += f"Content-Length: {len(data)}\r\n\r\n"
            sock.sendall(out.encode("latin-1", "replace") + data)
        log(f"hit {method} {url}")
        return True


def log(msg):
    if LOG:
        LOG.write(msg + "\n")
        LOG.flush()


LOG = None


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 256


def main():
    global LOG
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--ca", required=True)
    p.add_argument("--store", required=True)
    p.add_argument("--record", action="store_true")
    p.add_argument("--log")
    a = p.parse_args()
    if a.log:
        LOG = open(a.log, "a", encoding="utf-8")
    srv = Server(("127.0.0.1", a.port), Handler)
    srv.store = Store(a.store, a.record)
    srv.ca = CA(a.ca)
    print(f"ready {a.port}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Build and sign the DentalCraft update manifest (update.json + update.json.minisig).

    python3 packaging/update-manifest.py --version 0.3.0 --assets dist/ \
        [--date 2026-11-20] [--key ~/.config/dentalcraft-release/minisign.key] \
        [--base-url https://dentalcraft.app/download/] [--notes-url URL]

The key comes from --key or, when that is absent, from the environment variable
DENTALCRAFT_MINISIGN_KEY holding the contents of an unencrypted minisign secret key
(`minisign -G -W`). Run it after every release asset exists and before the SHA256SUMS
file is written, so that the sums also cover update.json and its signature.

Output: <assets>/update.json and <assets>/update.json.minisig. The signature is a
standard prehashed minisign signature (Ed25519 over BLAKE2b-512), verifiable with
`minisign -Vm update.json -P <public key>` and by the app (crate minisign-verify).

Stdlib only: Ed25519 is implemented below (RFC 8032), so no minisign binary is needed.
"""

import argparse
import base64
import datetime
import hashlib
import json
import os
import re
import sys
from pathlib import Path

# ---------------------------------------------------------------- Ed25519 (RFC 8032)

P = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
D = -121665 * pow(121666, P - 2, P) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)


def _recover_x(y, sign):
    x2 = (y * y - 1) * pow(D * y * y + 1, P - 2, P) % P
    if x2 == 0:
        if sign:
            raise ValueError("invalid point")
        return 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P:
        x = x * SQRT_M1 % P
    if (x * x - x2) % P:
        raise ValueError("invalid point")
    if (x & 1) != sign:
        x = P - x
    return x


_BY = 4 * pow(5, P - 2, P) % P
_BX = _recover_x(_BY, 0)
BASE = (_BX, _BY, 1, _BX * _BY % P)  # extended coordinates (X, Y, Z, T)
IDENTITY = (0, 1, 1, 0)


def _add(p, q):
    a = (p[1] - p[0]) * (q[1] - q[0]) % P
    b = (p[1] + p[0]) * (q[1] + q[0]) % P
    c = 2 * p[3] * q[3] * D % P
    d = 2 * p[2] * q[2] % P
    e, f, g, h = b - a, d - c, d + c, b + a
    return (e * f % P, g * h % P, f * g % P, e * h % P)


def _mul(s, p):
    q = IDENTITY
    while s:
        if s & 1:
            q = _add(q, p)
        p = _add(p, p)
        s >>= 1
    return q


def _encode(p):
    zi = pow(p[2], P - 2, P)
    x, y = p[0] * zi % P, p[1] * zi % P
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def _decode(b):
    if len(b) != 32:
        raise ValueError("invalid point length")
    y = int.from_bytes(b, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    if y >= P:
        raise ValueError("invalid point")
    x = _recover_x(y, sign)
    return (x, y, 1, x * y % P)


def _h(*parts):
    return int.from_bytes(hashlib.sha512(b"".join(parts)).digest(), "little")


def ed25519_public(seed):
    a = int.from_bytes(hashlib.sha512(seed).digest()[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return _encode(_mul(a, BASE))


def ed25519_sign(seed, msg):
    digest = hashlib.sha512(seed).digest()
    a = int.from_bytes(digest[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    pub = _encode(_mul(a, BASE))
    r = _h(digest[32:], msg) % L
    big_r = _encode(_mul(r, BASE))
    k = _h(big_r, pub, msg) % L
    s = (r + k * a) % L
    return big_r + s.to_bytes(32, "little")


def ed25519_verify(pub, msg, sig):
    if len(sig) != 64:
        return False
    s = int.from_bytes(sig[32:], "little")
    if s >= L:
        return False
    try:
        a_point = _decode(pub)
        r_point = _decode(sig[:32])
    except ValueError:
        return False
    k = _h(sig[:32], pub, msg) % L
    return _encode(_mul(s, BASE)) == _encode(_add(r_point, _mul(k, a_point)))


# ---------------------------------------------------------------- minisign


def load_secret_key(text):
    """Return (key_id, seed, public_key) from an unencrypted minisign secret key file."""
    lines = [ln for ln in text.splitlines() if ln.strip()]
    if len(lines) < 2 or not lines[0].startswith("untrusted comment:"):
        raise SystemExit("minisign key: not a minisign secret key file")
    raw = base64.b64decode(lines[1])
    if len(raw) != 158 or raw[:2] != b"Ed" or raw[4:6] != b"B2":
        raise SystemExit("minisign key: unsupported format")
    if raw[2:4] != b"\0\0":
        raise SystemExit("minisign key: password-protected keys are not supported, use an unencrypted key (minisign -G -W)")
    key_id, sk, checksum = raw[54:62], raw[62:126], raw[126:158]
    # minisign -G -W leaves the checksum zeroed; the seed/public-key match below still guards the key.
    if checksum != bytes(32) and hashlib.blake2b(raw[:2] + key_id + sk, digest_size=32).digest() != checksum:
        raise SystemExit("minisign key: checksum mismatch (corrupted key)")
    seed, pub = sk[:32], sk[32:]
    if ed25519_public(seed) != pub:
        raise SystemExit("minisign key: public half does not match the seed")
    return key_id, seed, pub


def minisign_sign(data, key_id, seed, pub, trusted_comment):
    prehash = hashlib.blake2b(data, digest_size=64).digest()
    sig = ed25519_sign(seed, prehash)
    global_sig = ed25519_sign(seed, sig + trusted_comment.encode())
    if not (ed25519_verify(pub, prehash, sig) and ed25519_verify(pub, sig + trusted_comment.encode(), global_sig)):
        raise SystemExit("minisign: self-verification of the signature failed")
    return (
        f"untrusted comment: signature from minisign secret key {key_id[::-1].hex().upper()}\n"
        f"{base64.b64encode(b'ED' + key_id + sig).decode()}\n"
        f"trusted comment: {trusted_comment}\n"
        f"{base64.b64encode(global_sig).decode()}\n"
    )


# ---------------------------------------------------------------- manifest

VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$")


def asset_names(v):
    return {
        "linux-standalone": f"dentalcraft_{v}_linux_x86_64.tar.gz",
        "appimage": f"DentalCraft-{v}-x86_64.AppImage",
        "windows-standalone": f"dentalcraft_{v}_windows_x86_64.zip",
        "windows-exe": f"DentalCraft-{v}-setup.exe",
        "windows-msi": f"DentalCraft-{v}-x86_64.msi",
    }


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--version", required=True, help="release version without the leading v, e.g. 0.3.0 or 0.3.0-alpha.1")
    ap.add_argument("--assets", required=True, type=Path, help="directory holding the release assets; output goes there too")
    ap.add_argument("--date", default=datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d"), help="release date, YYYY-MM-DD (default: today, UTC)")
    ap.add_argument("--key", type=Path, help="minisign secret key file (default: env DENTALCRAFT_MINISIGN_KEY)")
    ap.add_argument("--base-url", default="https://dentalcraft.app/download/")
    ap.add_argument("--notes-url", default="https://dentalcraft.app/en/download/")
    args = ap.parse_args()

    version = args.version.removeprefix("v")
    if not VERSION_RE.match(version):
        raise SystemExit(f"bad --version: {args.version!r}")
    if not re.match(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$", args.date):
        raise SystemExit(f"bad --date: {args.date!r}")
    if args.key:
        key_text = args.key.read_text()
    elif os.environ.get("DENTALCRAFT_MINISIGN_KEY", "").strip():
        key_text = os.environ["DENTALCRAFT_MINISIGN_KEY"]
    else:
        raise SystemExit("no signing key: pass --key or set DENTALCRAFT_MINISIGN_KEY")
    key_id, seed, pub = load_secret_key(key_text)

    base = args.base_url if args.base_url.endswith("/") else args.base_url + "/"
    assets = {}
    missing = []
    for kind, name in asset_names(version).items():
        path = args.assets / name
        if not path.is_file():
            missing.append(name)
            continue
        assets[kind] = {"url": base + name, "sha256": sha256_of(path), "size": path.stat().st_size}
    if missing:
        raise SystemExit("missing release assets in %s: %s" % (args.assets, ", ".join(missing)))

    manifest = {
        "version": version,
        "tag": f"v{version}",
        "date": args.date,
        "notes_url": args.notes_url,
        "assets": assets,
    }
    data = (json.dumps(manifest, indent=2) + "\n").encode()
    signature = minisign_sign(data, key_id, seed, pub, f"dentalcraft {version} update.json")
    (args.assets / "update.json").write_bytes(data)
    (args.assets / "update.json.minisig").write_text(signature)
    print(f"wrote {args.assets / 'update.json'} and update.json.minisig (key {key_id[::-1].hex().upper()})")


if __name__ == "__main__":
    sys.exit(main())

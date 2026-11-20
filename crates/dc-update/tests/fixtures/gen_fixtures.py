#!/usr/bin/env python3
"""Writes the signed update manifests the dc-update tests read.

    python3 crates/dc-update/tests/fixtures/gen_fixtures.py

Needs the `cryptography` package (Ed25519). The key is a TEST key derived from a
fixed seed, so the output is the same on every run; it is never the release key
(`dc_update::PUBLIC_KEY`) and the app never trusts it.

Files (next to this script):
- test-key.pub: the test public key, minisign format;
- newer.json, newer.json.minisig: version 999.0.0, every asset, prehashed signature;
- older.json, older.json.minisig: version 0.0.1;
- newer.json.legacy.minisig: a legacy (not prehashed) signature of newer.json,
  which the app refuses.
"""

import base64
import hashlib
import json
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

HERE = Path(__file__).resolve().parent
SEED = hashlib.sha256(b"dentalcraft-update-test-key").digest()
KEY_ID = b"DCTESTK1"

SECRET = Ed25519PrivateKey.from_private_bytes(SEED)
PUBLIC = SECRET.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode()


def signature(data: bytes, trusted: str, prehashed: bool = True) -> str:
    """Minisign signature file: prehashed (`ED`, BLAKE2b-512) or legacy (`Ed`)."""
    algorithm = b"ED" if prehashed else b"Ed"
    message = hashlib.blake2b(data, digest_size=64).digest() if prehashed else data
    sig = SECRET.sign(message)
    global_sig = SECRET.sign(sig + trusted.encode())
    return (
        "untrusted comment: signature from the DentalCraft TEST key\n"
        f"{b64(algorithm + KEY_ID + sig)}\n"
        f"trusted comment: {trusted}\n"
        f"{b64(global_sig)}\n"
    )


def manifest(version: str) -> bytes:
    base = "https://dentalcraft.app/download/"
    names = {
        "linux-standalone": f"dentalcraft_{version}_linux_x86_64.tar.gz",
        "appimage": f"DentalCraft-{version}-x86_64.AppImage",
        "windows-standalone": f"dentalcraft_{version}_windows_x86_64.zip",
        "windows-exe": f"DentalCraft-{version}-setup.exe",
        "windows-msi": f"DentalCraft-{version}-x86_64.msi",
        "macos-universal": f"dentalcraft_{version}_macos_universal.zip",
    }
    assets = {
        key: {
            "url": base + name,
            "sha256": hashlib.sha256(name.encode()).hexdigest(),
            "size": len(name),
        }
        for key, name in names.items()
    }
    doc = {
        "version": version,
        "tag": f"v{version}",
        "date": "2026-11-15",
        "notes_url": "https://dentalcraft.app/en/download/",
        "assets": assets,
    }
    return (json.dumps(doc, indent=2) + "\n").encode()


def main() -> None:
    key_id_hex = KEY_ID[::-1].hex().upper()
    (HERE / "test-key.pub").write_text(
        f"untrusted comment: minisign public key {key_id_hex} (DentalCraft TEST key)\n"
        f"{b64(b'Ed' + KEY_ID + PUBLIC)}\n"
    )
    for name, version in (("newer", "999.0.0"), ("older", "0.0.1")):
        data = manifest(version)
        (HERE / f"{name}.json").write_bytes(data)
        trusted = f"dentalcraft {version} update.json"
        (HERE / f"{name}.json.minisig").write_text(signature(data, trusted))
        if name == "newer":
            (HERE / f"{name}.json.legacy.minisig").write_text(
                signature(data, trusted, prehashed=False)
            )


if __name__ == "__main__":
    main()

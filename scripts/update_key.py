"""Makes the key pair that signs the launcher's update manifest on the mirror (see publish_mirror.py).

The private key is written outside the repository, to %APPDATA%\\stellaris-launcher-release\\update-signing.pem (or --out), and is
never overwritten. Keep a copy offline: losing it means shipping a release with a new public key before the mirror can be used again.
The public key printed at the end goes into MIRROR_KEY in crates/stl-core/src/selfupdate.rs.

    python scripts/update_key.py            # make the key (once)
    python scripts/update_key.py --public   # print the public key of the existing one
"""
import argparse
import os
import sys
from pathlib import Path

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


def default_path() -> Path:
    return Path(os.environ["APPDATA"]) / "stellaris-launcher-release" / "update-signing.pem"


def load(path: Path) -> Ed25519PrivateKey:
    key = serialization.load_pem_private_key(path.read_bytes(), password=None)
    if not isinstance(key, Ed25519PrivateKey):
        sys.exit(f"{path} is not an Ed25519 key")
    return key


def public_hex(key: Ed25519PrivateKey) -> str:
    return key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=None)
    ap.add_argument("--public", action="store_true", help="only print the public key of the existing private key")
    a = ap.parse_args()
    path = a.out or default_path()
    if a.public:
        print(public_hex(load(path)))
        return
    if path.exists():
        sys.exit(f"{path} already exists; it is never overwritten (use --public to print its public key)")
    path.parent.mkdir(parents=True, exist_ok=True)
    key = Ed25519PrivateKey.generate()
    pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption())
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as f:
        f.write(pem)
    print(f"private key: {path}")
    print(f"public key:  {public_hex(key)}")


if __name__ == "__main__":
    main()

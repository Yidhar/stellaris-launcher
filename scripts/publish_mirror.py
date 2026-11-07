"""Copies a GitHub release of the launcher to its update mirror (Cloudflare R2, served at https://dl.stellarismod.com/launcher/).

Run on the release machine after CI has published the release:

    python scripts/publish_mirror.py v0.1.10
    python scripts/publish_mirror.py v0.1.10 --dry-run     # download, check and sign; upload nothing

1. downloads the release's zip and setup program and their .sha256 files with `gh`, and checks each file against its .sha256;
2. uploads them under launcher/<tag>/ (never changed again, so cached for a year);
3. writes launcher/latest.json last: the manifest (version, notes, the files and their SHA-256) signed with the key made by
   update_key.py. The launcher installs nothing from the mirror whose manifest that key did not sign, so whoever can write to the
   bucket still cannot ship an update.

Needs `gh` (signed in), `wrangler`, CLOUDFLARE_API_TOKEN (R2 write on the bucket) and CLOUDFLARE_ACCOUNT_ID in the environment.
Nothing secret is printed or written into the repository.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import update_key  # noqa: E402

REPO = "Yidhar/stellaris-launcher"
BUCKET = "stellaris-modswiki"
PREFIX = "launcher"
PUBLIC = "https://dl.stellarismod.com/launcher"
PRODUCT = "stellaris-launcher"
IMMUTABLE = "public, max-age=31536000, immutable"
SHORT = "public, max-age=60"


def run(args: list[str], **kw) -> subprocess.CompletedProcess:
    exe = shutil.which(args[0])
    if not exe:
        sys.exit(f"{args[0]} is not on PATH")
    return subprocess.run([exe, *args[1:]], check=True, **kw)


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def put(local: Path, key: str, content_type: str, cache: str) -> None:
    run(["wrangler", "r2", "object", "put", f"{BUCKET}/{key}", "--file", str(local), "--remote",
         "--content-type", content_type, "--cache-control", cache], stdout=subprocess.DEVNULL)
    print(f"uploaded {PUBLIC}/{key.removeprefix(PREFIX + '/')}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("tag")
    ap.add_argument("--key", type=Path, default=None, help="the private key (default: where update_key.py writes it)")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    tag = a.tag if a.tag.startswith("v") else "v" + a.tag
    version = tag[1:]
    if "-" in version:
        sys.exit(f"{tag} is a pre-release; the launcher does not offer those")
    key = update_key.load(a.key or update_key.default_path())

    info = json.loads(run(["gh", "release", "view", tag, "-R", REPO, "--json", "tagName,url,body,isPrerelease,isDraft,assets"],
                          capture_output=True, text=True, encoding="utf-8").stdout)
    if info["isDraft"] or info["isPrerelease"]:
        sys.exit(f"{tag} is a draft or a pre-release")
    names = {x["name"] for x in info["assets"]}
    wanted = [f"stellaris-launcher-{tag}.zip", f"stellaris-launcher-setup-{tag}.exe"]
    for n in wanted:
        if n not in names or f"{n}.sha256" not in names:
            sys.exit(f"{tag} has no {n} with its .sha256")

    with tempfile.TemporaryDirectory(prefix="stl-mirror-") as tmp:
        tmp = Path(tmp)
        for n in wanted:
            run(["gh", "release", "download", tag, "-R", REPO, "-p", n, "-p", f"{n}.sha256", "-D", str(tmp)], stdout=subprocess.DEVNULL)
        files = {}
        for n in wanted:
            want = (tmp / f"{n}.sha256").read_text(encoding="utf-8").split()[0].lower()
            got = sha256(tmp / n)
            if want != got:
                sys.exit(f"{n} does not match its .sha256 (expected {want}, got {got})")
            files[n] = {"path": f"{tag}/{n}", "sha256": got, "size": (tmp / n).stat().st_size}

        manifest = json.dumps({
            "product": PRODUCT,
            "version": version,
            "tag": tag,
            "page": info["url"],
            "notes": info["body"] or "",
            "zip": files[wanted[0]],
            "setup": files[wanted[1]],
        }, ensure_ascii=False, indent=1)
        signature = key.sign(manifest.encode("utf-8")).hex()
        latest = tmp / "latest.json"
        latest.write_text(json.dumps({"manifest": manifest, "signature": signature}, ensure_ascii=False), encoding="utf-8")
        key.public_key().verify(bytes.fromhex(signature), manifest.encode("utf-8"))
        print(f"signed {tag} with key {update_key.public_hex(key)[:16]}…")

        if a.dry_run:
            print(manifest)
            return
        for t in ("CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID"):
            if not os.environ.get(t):
                sys.exit(f"{t} is not set")
        for n in wanted:
            for name, ctype in ((n, "application/octet-stream"), (f"{n}.sha256", "text/plain; charset=utf-8")):
                put(tmp / name, f"{PREFIX}/{tag}/{name}", ctype, IMMUTABLE)
        # the manifest last: until it is replaced, launchers still see the previous release, whose files are all in place
        put(latest, f"{PREFIX}/latest.json", "application/json; charset=utf-8", SHORT)


if __name__ == "__main__":
    main()

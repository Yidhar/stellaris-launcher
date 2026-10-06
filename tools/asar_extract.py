"""Extracts an Electron `app.asar` archive (no dependencies), to read the code of an Electron application such as the Paradox Launcher.

    python tools/asar_extract.py <app.asar> <output folder> [--list] [--only <path prefix>]...

--only extracts just the files under a prefix (`dist`, `package.json`); node_modules of an Electron app is hundreds of megabytes of native SDKs.

The format: a 16-byte prefix (UInt32 sizes, Chromium "pickle" framing), then a JSON header describing a tree of files (`size`, `offset` as a
string, `unpacked` for files stored next to the archive in `<app.asar>.unpacked`), then the file data; offsets are relative to the end of
the header. Files marked `unpacked` are copied from the `.unpacked` folder when it exists.
"""
import json
import os
import shutil
import struct
import sys


def read_header(f):
    prefix = f.read(16)
    if len(prefix) < 16:
        raise SystemExit("not an asar archive (too short)")
    pickle_size, header_size, _, json_len = struct.unpack("<IIII", prefix)
    if pickle_size != 4:
        raise SystemExit(f"not an asar archive (first word is {pickle_size}, expected 4)")
    header = json.loads(f.read(json_len).decode("utf-8"))
    return header, 8 + header_size


def walk(node, path=""):
    for name, entry in sorted(node.get("files", {}).items()):
        full = f"{path}/{name}" if path else name
        if "files" in entry:
            yield from walk(entry, full)
        else:
            yield full, entry


def main():
    argv = sys.argv[1:]
    only = []
    while "--only" in argv:
        i = argv.index("--only")
        only.append(argv[i + 1])
        del argv[i:i + 2]
    args = [a for a in argv if not a.startswith("--")]
    if len(args) < 2 and "--list" not in sys.argv:
        raise SystemExit(__doc__)
    asar = args[0]
    out = args[1] if len(args) > 1 else None
    unpacked = asar + ".unpacked"
    with open(asar, "rb") as f:
        header, base = read_header(f)
        files = [(p, e) for p, e in walk(header) if not only or any(p == o or p.startswith(o.rstrip("/") + "/") for o in only)]
        if "--list" in sys.argv or out is None:
            for path, e in files:
                print(f"{e['size']:>10}  {'U' if e.get('unpacked') else ' '}  {path}")
            print(f"{len(files)} files", file=sys.stderr)
            return
        count = skipped = 0
        for path, e in files:
            dst = os.path.join(out, *path.split("/"))
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            if e.get("unpacked"):
                src = os.path.join(unpacked, *path.split("/"))
                if os.path.exists(src):
                    shutil.copyfile(src, dst)
                    count += 1
                else:
                    skipped += 1
                continue
            f.seek(base + int(e["offset"]))
            with open(dst, "wb") as g:
                remaining = e["size"]
                while remaining:
                    chunk = f.read(min(remaining, 1 << 20))
                    if not chunk:
                        raise SystemExit(f"{asar} is truncated at {path}")
                    g.write(chunk)
                    remaining -= len(chunk)
            count += 1
    print(f"extracted {count} files to {out}" + (f" ({skipped} unpacked files not found)" if skipped else ""))


if __name__ == "__main__":
    main()

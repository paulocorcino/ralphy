"""Build a fake release for the install scripts to read.

    python install-fixture.py <ralphy binary> <target> <out folder>

The out folder then holds what the scripts read from GitHub: `releases` (the
release list) and `download/<tag>/<archive>` with its `.sha256`, laid out as
the release workflow packs them. Serve it with `python -m http.server`.
"""

import hashlib
import json
import pathlib
import sys
import tarfile
import zipfile

TAG = "v0.0.0-fixture"


def main() -> None:
    binary, target, out = pathlib.Path(sys.argv[1]), sys.argv[2], pathlib.Path(sys.argv[3])
    name = f"ralphy-{TAG}-{target}"
    folder = out / "download" / TAG
    folder.mkdir(parents=True, exist_ok=True)
    inner = "ralphy.exe" if target.startswith("windows-") else "ralphy"
    if target.startswith("windows-"):
        archive = folder / f"{name}.zip"
        with zipfile.ZipFile(archive, "w") as z:
            z.write(binary, f"{name}/{inner}")
    else:
        archive = folder / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as t:
            info = t.gettarinfo(binary, f"{name}/{inner}")
            info.mode = 0o755
            with open(binary, "rb") as f:
                t.addfile(info, f)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (folder / f"{archive.name}.sha256").write_text(f"{digest}  {archive.name}\n")
    (out / "releases").write_text(json.dumps([{"tag_name": TAG, "draft": False}]))


if __name__ == "__main__":
    main()

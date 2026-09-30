#!/usr/bin/env python3
"""Validate release metadata and upload missing assets without replacing files."""
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True).strip()


def release(tag):
    return json.loads(gh("release", "view", tag, "--json", "tagName,assets"))


def main():
    publishing = os.environ.get("PUBLISH_RELEASE") == "true"
    version = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    tag = os.environ.get("RELEASE_TAG") or f"v{version}"
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", tag):
        raise SystemExit("Invalid release tag")
    if tag != f"v{version}":
        raise SystemExit("Release tag must match the Cargo package version")
    if sys.argv[1] == "prepare":
        if publishing:
            if not os.environ.get("RELEASE_TAG"):
                raise SystemExit("Publishing requires an explicit existing release tag")
            release(tag)
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as output:
            output.write(f"RELEASE_VERSION={tag}\n")
        return
    if sys.argv[1] != "publish" or not publishing:
        raise SystemExit("Publication is not enabled")
    assets = {asset["name"] for asset in release(tag)["assets"]}
    existing = frozenset(assets)
    for pattern in sys.argv[2:]:
        paths = sorted(glob.glob(pattern))
        if not paths:
            raise SystemExit(f"No files matched {pattern}")
        for filename in paths:
            name = Path(filename).name
            # A newly built checksum must never describe an older preserved archive.
            if name.endswith(".sha256") and name.removesuffix(".sha256") in existing:
                if name not in assets:
                    archive = name.removesuffix(".sha256")
                    with tempfile.TemporaryDirectory() as directory:
                        subprocess.run(["gh", "release", "download", tag,
                                        "--pattern", archive, "--dir", directory], check=True)
                        with (Path(directory) / archive).open("rb") as source:
                            digest = hashlib.file_digest(source, "sha256").hexdigest()
                        checksum = Path(directory) / name
                        checksum.write_text(f"{digest}  {archive}\n", encoding="ascii", newline="\n")
                        subprocess.run(["gh", "release", "upload", tag, str(checksum)], check=True)
                        assets.add(name)
                else:
                    print(f"Preserving checksum for existing archive: {name}")
                continue
            if name not in existing and f"{name}.sha256" in existing:
                raise SystemExit(f"Release has an orphan checksum for {name}; inspect it before recovery")
            if name in assets:
                print(f"Preserving existing asset: {name}")
                continue
            subprocess.run(["gh", "release", "upload", tag, filename], check=True)
            assets.add(Path(filename).name)


if __name__ == "__main__":
    main()

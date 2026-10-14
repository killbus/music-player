#!/usr/bin/env python3
"""Check portable asset names, local checksum pairs and uploaded bytes."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys


def file_digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def validate_asset_names(paths):
    for path in paths:
        # Use a conservative subset that GitHub preserves, notably excluding ~.
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*[A-Za-z0-9]", path.name):
            raise SystemExit(f"Nonportable release asset name: {path.name}")


def validate_local_files(paths):
    validate_asset_names(paths)
    selected = set(paths)
    for path in paths:
        if not path.is_file():
            raise SystemExit(f"Missing release file: {path}")
        if not path.name.endswith(".sha256"):
            if path.with_name(path.name + ".sha256") not in selected:
                raise SystemExit(f"Missing adjacent checksum: {path.name}")
            continue
        archive = path.with_name(path.name.removesuffix(".sha256"))
        if archive not in selected:
            raise SystemExit(f"Missing checksum archive: {path.name}")
        match = re.fullmatch(r"([0-9a-f]{64}) [ *]([^\r\n]+)\n?", path.read_text(encoding="ascii"))
        if not match or match[2] != archive.name:
            raise SystemExit(f"Checksum target mismatch: {path.name}")
        if match[1] != file_digest(archive):
            raise SystemExit(f"Checksum digest mismatch: {archive.name}")


def verify_remote_files(paths, assets):
    remote = {asset["name"]: asset for asset in assets}
    for path in paths:
        asset = remote.get(path.name)
        if asset is None or asset.get("state") != "uploaded":
            raise SystemExit(f"Missing uploaded asset with exact name: {path.name}")
        if asset.get("digest") != "sha256:" + file_digest(path):
            raise SystemExit(f"Uploaded asset digest mismatch: {path.name}")


def verify_uploaded_files(tag, paths):
    release = json.loads(subprocess.check_output(
        ["gh", "release", "view", tag, "--json", "assets"], text=True))
    verify_remote_files(paths, release["assets"])


if __name__ == "__main__":
    files = sorted(Path(sys.argv[1]).iterdir())
    if not files:
        raise SystemExit("No release files to validate")
    validate_local_files(files)

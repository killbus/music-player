#!/usr/bin/env python3
"""Resolve one revision and version for the entire release run."""
import json
import os
import re
import subprocess
import tomllib
from pathlib import Path

version = tomllib.loads(Path('Cargo.toml').read_text())['package']['version']
sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
tag = os.environ.get('REQUESTED_TAG', '')
publish = os.environ.get('PUBLISH') == 'true'
if tag:
    if not re.fullmatch(r'v\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?', tag):
        raise SystemExit('tag must be vMAJOR.MINOR.PATCH[-prerelease]')
    if tag[1:] != version:
        raise SystemExit(f'tag {tag} does not match Cargo version {version}')
elif publish:
    raise SystemExit('publishing requires an existing version tag')
else:
    version += '-dev.' + sha[:12]
if publish:
    release = json.loads(subprocess.check_output(
        ['gh', 'release', 'view', tag, '--json', 'tagName,isDraft'], text=True))
    if release['tagName'] != tag:
        raise SystemExit('release tag mismatch')
with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
    for key, value in dict(sha=sha, version=version, tag=tag,
                           image='ghcr.io/' + os.environ['GITHUB_REPOSITORY'].lower(),
                           publish=str(publish).lower()).items():
        output.write(f'{key}={value}\n')

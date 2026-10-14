#!/usr/bin/env python3
"""Publish already-tested files/images without rebuilding or overwriting."""
import json
import os
from pathlib import Path
import subprocess

from release_checksums import validate_local_files, verify_uploaded_files


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def require_missing(ref):
    result = subprocess.run(['docker', 'manifest', 'inspect', ref], text=True, capture_output=True)
    if result.returncode == 0:
        raise SystemExit(f'refusing to replace existing image tag: {ref}')
    # A network/authorization error is not evidence that the tag is absent.
    if not any(message in result.stderr.lower() for message in ('manifest unknown', 'no such manifest')):
        raise SystemExit(f'cannot establish that {ref} is absent: {result.stderr}')


def main():
    tag, version, image = (os.environ[key] for key in ('RELEASE_TAG', 'RELEASE_VERSION', 'IMAGE'))
    out = Path('dist/build/release')
    files = sorted(out.iterdir())
    if not files:
        raise SystemExit('missing release files')
    validate_local_files(files)
    release = json.loads(run('gh', 'release', 'view', tag, '--json', 'assets'))
    existing = {asset['name'] for asset in release['assets']}
    collision = existing.intersection(p.name for p in files)
    if collision:
        raise SystemExit(f'refusing to replace release assets: {sorted(collision)}')
    subprocess.run(['docker', 'login', 'ghcr.io', '-u', os.environ['GITHUB_ACTOR'], '--password-stdin'],
                   input=os.environ['GH_TOKEN'], text=True, check=True)
    try:
        digests = []
        for arch in ('amd64', 'arm64'):
            archive = Path(f'dist/build/image-export/image-{arch}.tar.gz')
            subprocess.run(['docker', 'load', '-i', str(archive)], check=True)
            source = f'music-player-ci:{arch}'
            if run('docker', 'image', 'inspect', source, '--format', '{{.Architecture}}') != arch:
                raise SystemExit('image architecture mismatch')
            metadata = json.loads((out / f'build-{arch}.json').read_text())
            if run('docker', 'image', 'inspect', source, '--format',
                   '{{index .Config.Labels "org.opencontainers.image.revision"}}') != metadata['SOURCE_REVISION']:
                raise SystemExit('image revision mismatch')
            # Establish even a new GHCR package before probing a missing version.
            # Unique CI staging tags also let a failed publish be investigated.
            destination = f'{image}:build-{os.environ["GITHUB_RUN_ID"]}-{os.environ["GITHUB_RUN_ATTEMPT"]}-{arch}'
            subprocess.run(['docker', 'tag', source, destination], check=True)
            subprocess.run(['docker', 'push', destination], check=True)
            digest = run('docker', 'image', 'inspect', destination, '--format', '{{index .RepoDigests 0}}')
            digests.append(digest)
        require_missing(f'{image}:{version}')
        subprocess.run(['docker', 'buildx', 'imagetools', 'create', '--tag', f'{image}:{version}', *digests], check=True)
        manifest = json.loads(run('docker', 'buildx', 'imagetools', 'inspect', '--raw', f'{image}:{version}'))
        platforms = {(m['platform']['os'], m['platform']['architecture']) for m in manifest['manifests']}
        if platforms != {('linux', 'amd64'), ('linux', 'arm64')}:
            raise SystemExit(f'unexpected manifest platforms: {platforms}')
        # gh release upload without --clobber refuses overwrites as well.
        subprocess.run(['gh', 'release', 'upload', tag, *(str(p) for p in files)], check=True)
        verify_uploaded_files(tag, files)
    finally:
        subprocess.run(['docker', 'logout', 'ghcr.io'], check=False)


if __name__ == '__main__':
    main()

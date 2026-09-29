#!/usr/bin/env bash
set -euo pipefail
: "${ARCH:?}" "${IMAGE_SOURCE:?}" "${SOURCE_REVISION:?}" "${RELEASE_VERSION:?}"

# Retain the former Ubuntu + DEB layout as a clean package test and size baseline.
mkdir -p dist/build/deb-image dist/build/evidence
cp dist/music-player-cli_*.deb dist/build/deb-image/
cat > dist/build/deb-image/Dockerfile <<'DOCKER'
FROM ubuntu:24.04
COPY *.deb /tmp/music-player.deb
RUN apt-get update \
    && apt-get install -y --no-install-recommends /tmp/music-player.deb \
    && rm -rf /var/lib/apt/lists/* /tmp/music-player.deb \
    && groupadd --gid 10001 music-player \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /data music-player \
    && install -d -o 10001 -g 10001 /data /data/config /data/cache /music
DOCKER
sed -n '/^ENV /,$p' dist/Dockerfile >> dist/build/deb-image/Dockerfile
docker build --platform "linux/$ARCH" -t "music-player-deb-test:$ARCH" dist/build/deb-image
docker build --platform "linux/$ARCH" -f dist/Dockerfile \
  --label "org.opencontainers.image.source=$IMAGE_SOURCE" \
  --label "org.opencontainers.image.revision=$SOURCE_REVISION" \
  --label "org.opencontainers.image.version=$RELEASE_VERSION" \
  --label 'org.opencontainers.image.licenses=MIT' \
  -t "music-player-ci:$ARCH" dist/build/image

python3 - "$ARCH" <<'PY'
import json
from pathlib import Path
import subprocess
import sys

arch = sys.argv[1]
evidence = Path('dist/build/evidence')

def run(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT, timeout=60).strip()

images = {}
for kind, image in [('ubuntu_deb', f'music-player-deb-test:{arch}'),
                    ('debian_slim', f'music-player-ci:{arch}')]:
    inspect = json.loads(run('docker', 'image', 'inspect', image))[0]
    prefix = ['docker', 'run', '--rm', '--entrypoint']
    linking = run(*prefix, 'ldd', image, '/usr/bin/music-player')
    (evidence / f'container-ldd-{kind}-{arch}.txt').write_text(linking + '\n')
    assert 'not found' not in linking, f'{image}: unresolved dynamic dependencies'
    packages = run(*prefix, 'dpkg-query', image, '-W', '-f=${Package}\t${Version}\n')
    (evidence / f'container-packages-{kind}-{arch}.txt').write_text(packages + '\n')
    os_release = run(*prefix, 'cat', image, '/etc/os-release')
    images[kind] = {'image_id': inspect['Id'], 'size_bytes': inspect['Size'],
                    'os_release': os_release}
    if kind == 'debian_slim':
        installed = {line.split()[0] for line in packages.splitlines()}
        assert not {'music-player', 'music-player-cli'}.intersection(installed)
        run(*prefix, 'sh', image, '-ec',
            '. /etc/os-release; test "$ID" = debian; test "$VERSION_CODENAME" = trixie; '
            'test ! -e /usr/lib/systemd/system/music-player.service; '
            'test ! -e /etc/default/music-player; '
            'test -s /usr/share/licenses/music-player/LICENSE; '
            'test -s /etc/ssl/certs/ca-certificates.crt')
        images[kind]['standalone_runtime'] = True

baseline = images['ubuntu_deb']['size_bytes']
saved = baseline - images['debian_slim']['size_bytes']
report = {'architecture': arch, 'measurement': 'docker image inspect Size (uncompressed layers)',
          'images': images, 'saved_bytes': saved, 'saved_percent': round(100 * saved / baseline, 2)}
(evidence / f'image-comparison-{arch}.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report))
PY

# Each image must independently pass binary identity, Web UI, FIFO and persistence.
python3 scripts/test-headless.py "music-player-deb-test:$ARCH" "$ARCH"
python3 scripts/test-headless.py "music-player-ci:$ARCH" "$ARCH"

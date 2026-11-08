#!/usr/bin/env bash
set -euo pipefail
: "${ARCH:?}"
mkdir -p dist/build/rpm-image
cp dist/music-player-cli-*.rpm dist/build/rpm-image/
cat > dist/build/rpm-image/Dockerfile <<'DOCKER'
FROM fedora:44
COPY *.rpm /tmp/music-player.rpm
RUN dnf install -y /tmp/music-player.rpm \
    && dnf clean all \
    && rm /tmp/music-player.rpm \
    && groupadd --gid 10001 music-player \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /data music-player \
    && install -d -o 10001 -g 10001 /data /data/config /data/cache /music
DOCKER
# Use exactly the same runtime path/user/entrypoint contract as the shipped image.
sed -n '/^ENV /,$p' dist/Dockerfile >> dist/build/rpm-image/Dockerfile
docker build --platform "linux/$ARCH" -t "music-player-rpm-test:$ARCH" dist/build/rpm-image
python3 scripts/test-headless.py "music-player-rpm-test:$ARCH" "$ARCH"
# Verify conflict handling against a synthetic package with the legacy name.
# It intentionally contains no player: this isolates package identity conflicts.
mkdir -p dist/build/legacy/DEBIAN
printf 'Package: music-player\nVersion: 0.0.1\nArchitecture: all\nMaintainer: CI <ci@example.invalid>\nDescription: package conflict fixture\n' > dist/build/legacy/DEBIAN/control
dpkg-deb --build --root-owner-group dist/build/legacy dist/build/legacy.deb
cat > dist/build/legacy.spec <<'SPEC'
Name: music-player
Version: 0.0.1
Release: 1
Summary: package conflict fixture
License: MIT
BuildArch: noarch
%description
Package identity conflict fixture.
%files
SPEC
rpmbuild --define "_topdir $(pwd)/dist/build/legacy-rpm" -bb dist/build/legacy.spec
cp dist/build/legacy-rpm/RPMS/noarch/*.rpm dist/build/legacy.rpm
for format in deb rpm; do
  if [[ "$format" == deb ]]; then
    test_image="music-player-deb-test:$ARCH"
    install_command='dpkg -i /legacy.deb'
  else
    test_image="music-player-rpm-test:$ARCH"
    install_command='rpm -i /legacy.rpm'
  fi
  if docker run --rm --user 0 --entrypoint sh \
      -v "$(pwd)/dist/build/legacy.$format:/legacy.$format:ro" \
      "$test_image" -ec "$install_command" > "dist/build/evidence/conflict-$format-$ARCH.txt" 2>&1; then
    echo "legacy $format unexpectedly coinstalled with CLI" >&2
    exit 1
  fi
  grep -i 'conflict' "dist/build/evidence/conflict-$format-$ARCH.txt"
done
# Include completed runtime checks in the public per-architecture evidence.
cp dist/build/evidence/* dist/build/release/
(cd dist/build/release && for file in *.log *.json *.txt; do sha256sum "$file" > "$file.sha256"; done)

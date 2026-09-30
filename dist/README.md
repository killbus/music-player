# Linux CLI distribution

The CLI contains the daemon, terminal client and embedded Web UI. The Slint
desktop application is a separate build. This pipeline produces amd64 and arm64
archives, CLI DEB/RPM packages and a multi-platform GHCR image from the same
binary per architecture. The runtime image copies that executable directly;
it does not install the CLI DEB or run Cargo.

The build and DEB baseline is **Ubuntu 24.04**; the runtime image uses the official
**Debian 13 (`trixie-slim`)** image. RPM installation and playback are checked on
**Fedora 44**. Package dependencies are derived from the ELF binary. The container
installs CA certificates, ALSA, glibc and the GCC/C++ runtime libraries; CI checks
dynamic linking and playback on both architectures. Current binaries require
glibc >= 2.39, so Alpine/musl is not a drop-in base replacement. Compatibility
with other releases, including Debian 11/12, is not established by these checks.
See the release's per-architecture dependency, build and runtime evidence.
Native dependencies still apply to tar archives.

## Install a package

Download an artifact and its adjacent `.sha256` from this fork's release (or
unpack a validation run's `linux-cli-<arch>` Actions artifact), then verify it:

```sh
sha256sum -c music-player-cli_VERSION_ARCH.deb.sha256
sudo apt install ./music-player-cli_VERSION_ARCH.deb
# Fedora:
sudo dnf install ./music-player-cli-VERSION-1.ARCH.rpm
music-player --version
```

Replace VERSION and ARCH with the actual downloaded filename. `music-player-cli`
conflicts with the existing combined `music-player` package, which owns the same
executable. Remove the combined package before switching. Its desktop build and
four-argument `dist/package-linux.sh` interface remain available.

RPM and archives run the CLI directly. The **CLI DEB also includes a systemd
unit**, without requiring systemd as the container's init process.

## Run as a system service

First installation leaves `music-player.service` disabled and stopped. Set the
output and media directory in `/etc/default/music-player` before starting it:

```sh
sudoedit /etc/default/music-player
sudo systemctl enable --now music-player
systemctl status music-player
journalctl -u music-player -f
```

The DEB creates a fixed non-root music-player account. systemd manages data under
`/var/lib/music-player` and cache under `/var/cache/music-player`. Settings and SQLite share
`/var/lib/music-player/config/music-player`. The default music directory is
`/var/lib/music-player/music`. For an existing library, configure
`MUSIC_PLAYER_MUSIC_DIRECTORY=/srv/music` and grant read/traverse access, using
`SupplementaryGroups=` in a service drop-in if required. Grant access through groups instead of changing external file ownership. Interactive CLI settings in your login account
are separate from the service's settings.

For example, edit `/etc/default/music-player` to change the HTTP port and
periodic library scan interval (in minutes; `0` disables periodic scans):

```ini
MUSIC_PLAYER_HTTP_PORT=8080
MUSIC_PLAYER_LIBRARY_REFRESH_INTERVAL=10
```

Use `KEY=value` lines without `export`, then run
`sudo systemctl restart music-player`. Editing this environment file does not
require `daemon-reload`. `HTTP_PORT` controls the Web UI and HTTP API; `PORT`
is a separate setting.
The service grants `CAP_NET_BIND_SERVICE` to its non-root process, so an
explicit `MUSIC_PLAYER_HTTP_PORT=80` is also supported. The default remains 5053.

The complete configuration is
`/var/lib/music-player/config/music-player/settings.toml`, normally created on
the first successful start. Edit settings such as `http_port = 8080` there,
including nested TOML tables, then restart the service. Environment variables
override the corresponding TOML values. A drop-in overriding `XDG_CONFIG_HOME`
also changes the settings file location.

For Snapcast, keep Snapserver's standard `/tmp/snapfifo` source and service.
music-player shares the host `/tmp`; it does not enable `PrivateTmp` or
`DynamicUser`. In `/etc/default/music-player`, set:

```ini
MUSIC_PLAYER_AUDIO_OUTPUT=fifo:/tmp/snapfifo
```

Start Snapserver first, then check `ls -l /tmp/snapfifo`. The playback engine
opens an existing FIFO read/write; it does not create it. If its permissions
require group access, grant the service membership in the FIFO's actual group
using `sudo systemctl edit music-player`, for example:

```ini
[Service]
SupplementaryGroups=snapserver
```

Use that example only when the FIFO belongs to the snapserver group and that
group has read/write permission. Run `sudo systemctl daemon-reload` after
changing a drop-in, then `sudo systemctl restart music-player`. No writable-path
mount or Snapserver service override is required. Permissions must remain
suitable when Snapserver recreates the FIFO. For local audio, grant the device
group instead.

| Package operation | Service behavior |
| --- | --- |
| First install | Disabled and stopped |
| Upgrade/reinstall while active | Restart, preserving enabled/disabled state |
| Upgrade while stopped | Remain stopped, even if enabled |
| Remove | Stop; retain administrator configuration and data |
| Purge | Remove packaged configuration/unit enablement; retain database, media and cache |

Package service actions respect Debian's `policy-rc.d`. Package upgrades do not
override administrator masks. Delete retained data explicitly only when no
longer needed. `/etc/default/music-player` is a DEB conffile, so local edits are
preserved by normal package upgrades.

### Containers (LXC, systemd as PID 1)

The unit uses a fixed account and shared temporary directories, avoiding the
mount isolation previously implied by DynamicUser. Native CI checks that the
service shares PID 1's mount namespace; it does not certify every LXC host
policy. Existing administrator drop-ins can still enable isolation: inspect
`systemctl cat music-player` and `journalctl -u music-player -b`. Do not loosen
permissions of systemd's root-owned `/var/cache/private` directory.

The service account is retained on purge alongside application data. Migration
of state from earlier dynamic-user packages has not been validated.

## Run the published container

Use `ghcr.io/<fork-owner>/music-player:<version>` or its digest after publication.
The pipeline does not maintain a `latest` tag. Publishing in the fork requires
the fork's own release/dispatch; an upstream release does not trigger it.

`compose.snapcast.yml` consumes an already-built image:

```sh
export MUSIC_PLAYER_IMAGE=ghcr.io/<fork-owner>/music-player:<version>
docker compose -f dist/compose.snapcast.yml up -d
```

Adjust the relative `./music` and `./snapcast` directories for your Compose file.
They resolve relative to that file. Snapserver must share the same host FIFO
directory and create/read `music-player.fifo`. The image runs as UID/GID 10001;
give it read access to the library and read/write access to the FIFO. Prefer a
shared numeric group with Compose `group_add` and a FIFO with group read/write
permissions. Do not mount a regular file where a FIFO is expected. The sample
does not bundle Snapserver.

`/data` stores configuration, SQLite, covers and cache; persist it across
replacements. All of `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and
`MUSIC_PLAYER_APPLICATION_DIRECTORY` are set consistently by the image. Merely
changing the application directory does not relocate settings/SQLite.
`MUSIC_PLAYER_AUDIO_OUTPUT` remains configurable. Web UI is on port 5053,
gRPC on 5051 and WebSocket on 5052. Map the ports you use.

The container starts the daemon directly. It does not install the music-player
DEB, its systemd unit or `/etc/default/music-player`. Configure the container
with environment variables and its persisted settings. The `trixie-slim` tag
stays on Debian 13 while
receiving base updates at build time. See the [official image documentation](https://github.com/docker-library/docs/tree/master/debian)
and [supported tags/architectures](https://github.com/docker-library/official-images/blob/master/library/debian).

## Build, validate and publish

Work on an independent branch. `Linux CLI distribution` runs on PRs and the
distribution feature branch, producing artifacts without release writes. Native
amd64/arm64 CI performs the heavy Rust/UI builds, byte identity checks, DEB
systemd lifecycle tests, clean DEB/RPM container installs, library/Web UI
readiness, FIFO audio and persistence checks. Ubuntu DEB and Fedora RPM installs
are tested separately from the published Debian image. CI also records installed
library versions and compares full uncompressed image sizes against the previous
Ubuntu + DEB layout using the same binary (`image-comparison-<arch>.json` in the
release evidence). This measures stored layers, including the DEB retained in
the old layout's COPY layer; it is not a registry download-size comparison.
Merge a reviewed, passing PR with
squash to keep mainline history focused. Packaging tests use deterministic local
audio; they do not certify Emby/Jellyfin TV/STRM playback or a physical speaker.

To prepare a release, update Cargo versions as usual and create the corresponding
`vMAJOR.MINOR.PATCH[-prerelease]` tag. A published release event builds and
publishes; a manual dispatch can validate an existing tag with `publish=false`,
or publish against an existing release with `publish=true`. The tag must match
Cargo's version. This implementation does not itself create a release.

The publisher loads the exact images tested in CI and pushes unique
`build-<run-id>-<attempt>-<arch>` staging tags, then creates a version tag from
their digests. This also initializes a new GHCR package before checking whether
its version tag exists. It refuses existing version tags and asset names;
authorization/network errors are failures, never proof of a missing tag. Make
the GHCR package public separately if anonymous pulls are wanted.

GitHub release uploads and GHCR publishing are not atomic. On a partial failure,
inspect the run's artifact checksums, image digests and existing release assets.
Do not blindly rerun after the final image tag or assets exist: complete missing
uploads from the **same tested artifacts**, verifying existing ones match, or
explicitly withdraw an unpublished candidate before rebuilding. Never overwrite
an already distributed version. Orphan staging tags can be removed once no
published manifest or recovery process needs them.

When changing workflows, fetch each Action's latest stable release, README and
action metadata from its GitHub repository. Record the repository/release/docs
URLs, check date and resolved SHA in `.github/action-versions.json`, then pin
that SHA in the workflow. Recheck the recorded facts with
`python3 scripts/check-action-versions.py .github/workflows/release.yml`.

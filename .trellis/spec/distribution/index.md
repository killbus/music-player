# Distribution

## Before development

Read the task artifacts, dist/README.md and ../guides/github-actions.md.
Search existing packaging scripts and workflows before adding another path.

## Contracts

- The CLI Cargo package includes daemon, terminal client and embedded Web UI.
  A CLI-only OS package must not contain the Slint executable or desktop assets.
- Existing music-player packages own both binaries. music-player-cli conflicts
  with music-player; changing the package name alone does not allow coexistence.
- Build one canonical Linux CLI binary per architecture. Archive, CLI packages
  and runtime image consume it. Package tools must not strip it a second time.
- Generate DEB shared-library requirements with dpkg-shlibdeps; use RPM ELF
  auto-dependencies. Record ldd/readelf evidence. Do not guess glibc compatibility.
- The first build/runtime baseline is Ubuntu 24.04. Test DEB there and RPM in
  Fedora 44. Other distributions require their own installation/runtime checks.
- Container persistence must cover XDG config and the application directory:
  MUSIC_PLAYER_APPLICATION_DIRECTORY alone does not relocate settings/SQLite.
- PR/ordinary dispatch runs validate only. Publishing requires a release event
  or an explicit publish dispatch against an existing matching version tag.
  Never overwrite existing release assets or repoint a released image tag.

## Service and development lifecycle

- CLI DEB includes music-player.service and /etc/default/music-player.
  First install neither enables nor starts it. Upgrade restarts only active
  instances and leaves enablement unchanged. Respect policy-rc.d.
- Use DynamicUser and systemd-managed state/cache; never assign an external
  FIFO or media directory to a transient UID. External FIFO access uses a
  pre-existing pipe, group permissions and an explicit ReadWritePaths drop-in.
- Remove stops the unit; purge removes package configuration and enablement
  but retains library/database/cache. No systemd PID1 is required in Docker.
- Iterate with recorded commits/CI on an independent branch. Do not merge
  before required checks pass; use a reviewed squash merge for mainline.

## Quality checks

Run actionlint and Bash syntax checks, verify Action evidence against GitHub,
then run the native amd64/arm64 workflow. It must check package contents,
dependencies, clean installation, package conflicts, binary identity, daemon
HTTP readiness, FIFO PCM output and persistent settings. The native runner
also verifies opt-in systemd installation, non-root FIFO playback, active
and stopped upgrades, disabled-but-active upgrade, removal and retained data. Record what actually
ran locally separately from checks merely configured in CI.

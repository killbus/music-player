# CLI and container release distribution

## User requirement

Ship Linux CLI-only packages, binary archives and Docker images from the
music-player fork. Deployment hosts consume published artifacts without building
an image themselves. Fetch current releases and documentation for each GitHub
Action and establish that as a repository rule.

## Acceptance

- Native amd64/arm64 builds produce CLI archives, DEB, RPM and one multi-platform
  image; CLI excludes Slint desktop but retains daemon/TUI/Web UI.
- Existing combined desktop package remains available with explicit conflict
  handling for the new CLI package.
- Validation precedes publication; branch/PR testing has no publish side effects.
- Source revision, version, checksums and architecture are recorded consistently.
- Container supports configurable output and persisted configuration/database;
  a Snapcast FIFO example uses a shared directory and does not assume Docker is
  the only way to run music-player.
- Actions are current, documented and pinned from live upstream facts.
- CLI DEB includes an opt-in systemd service; upgrades preserve enabled and
  running state, removal stops playback, purge retains application data.
- Heavy builds and installation tests run in native GitHub CI on the feature
  branch. Keep iteration history there; merge only after review and passing
  checks, with squash to avoid fragmented mainline history.

## Boundaries

Packaging is the authorized implementation. No release is published as part of
this implementation/CI work. Emby TV/STRM consumption remains a user requirement for subsequent
application acceptance; no user decision replaced it with a local-files-only
library. Packaging tests do not establish Emby/STRM playback compatibility.
No production access, SSH, media-source redesign or playback-control changes.

# Implementation and validation

1. Record live release/README/action.yml facts for every new workflow Action.
2. Add CLI package variant and generated dependencies; retain full default.
3. Add runtime Dockerfile and native clean-install/daemon/FIFO checks.
4. Consolidate Linux workflow, transfer canonical artifacts and gate publishing.
5. Run available local syntax/lint/evidence checks; review failure paths and
   package ownership. Native Linux tests require GitHub CI on both architectures.
6. Document usage, baseline, conflicts and partial-publication recovery.

Status: native dual-architecture validation passed; ready for review on
feat/headless-release. Merge and release publication have not been performed.

Local checks (2026-09-29): actionlint 1.7.12, ShellCheck 0.11.0, individual Bash
syntax checks and Python AST parsing passed. Live Action release/SHA/docs audit
passed for all four Actions. Metadata gates checked development builds, matching
tag validation, mismatched/invalid tag rejection and missing-tag publication
rejection. git diff --check passed. No local Linux build or production access.

Added actual native-runner DEB service checks for disabled first install,
non-root FIFO PCM, persisted configuration, active/stopped upgrades,
disabled-but-active upgrades, remove/stop and purge/data retention.
Their native CI results are recorded below.

Iterate with commits on the feature branch; only merge a reviewed passing PR
with squash. No release publication is authorized by this implementation task.

First native CI: https://github.com/killbus/music-player/actions/runs/36564859751
Both architectures built the Web UI, Rust CLI and DEB successfully, then failed
rendering the RPM spec. Bash expanded the unescaped replacement tilde to HOME,
which injected slashes into the sed expression. Escape the tilde so development
and prerelease versions retain a literal RPM prerelease separator. No service
or container runtime checks ran in this failed attempt.

Second native CI: https://github.com/killbus/music-player/actions/runs/36567622534
Both architectures passed package byte identity and every DEB service lifecycle
check, including non-root FIFO PCM. The Ubuntu containers passed first playback
but the persistence test queried the old ephemeral host port after restart.
Logs show the HTTP server listening again inside both containers. Re-resolve
the Docker port after restart and record both endpoints plus container inspect
data. Container persistence and Fedora RPM tests had not yet passed in this attempt.

Passing native CI: https://github.com/killbus/music-player/actions/runs/36574272956
Code revision: 06fe7cd0b865acd48aed8507d3a03d416262a214. Prepare and both build
jobs succeeded; publication was correctly skipped. Verified job reports show:

- amd64 and arm64 CLI archives, DEB and RPM contain the canonical binary;
  Ubuntu/Fedora containers also pass the binary identity check.
- All systemd lifecycle assertions passed on both native runners.
- Both container variants passed non-root startup, Web UI, decoded FIFO PCM
  and settings/library persistence after restart.
- Docker changed the Ubuntu HTTP port from 32768 to 32769 and Fedora's from
  32770 to 32771 on both architectures; resolving it again fixed the failure.
- Real DEB/RPM installers rejected the conflicting legacy package fixture.
- Four artifacts uploaded: linux-cli-amd64, linux-cli-arm64,
  linux-image-amd64 and linux-image-arm64. Package artifacts include checksums
  and detailed validation evidence; image archives are the tested images.

Subsequent changes are Markdown-only: FIFO read/write permissions, source facts
and these CI results. Application source and validated packaging/test scripts
are unchanged. See research/runtime.md for the existing MPRIS diagnostic and
the limits of the local-media acceptance scope. GHCR publication itself remains
unexecuted; no production deployment or Emby/STRM acceptance is claimed.

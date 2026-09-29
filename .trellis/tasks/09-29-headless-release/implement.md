# Implementation and validation

1. Record live release/README/action.yml facts for every new workflow Action.
2. Add CLI package variant and generated dependencies; retain full default.
3. Add runtime Dockerfile and native clean-install/daemon/FIFO checks.
4. Consolidate Linux workflow, transfer canonical artifacts and gate publishing.
5. Run available local syntax/lint/evidence checks; review failure paths and
   package ownership. Native Linux tests require GitHub CI on both architectures.
6. Document usage, baseline, conflicts and partial-publication recovery.

Status: iterating on feat/headless-release after the first native CI failure.

Local checks (2026-09-29): actionlint 1.7.12, ShellCheck 0.11.0, individual Bash
syntax checks and Python AST parsing passed. Live Action release/SHA/docs audit
passed for all four Actions. Metadata gates checked development builds, matching
tag validation, mismatched/invalid tag rejection and missing-tag publication
rejection. git diff --check passed. No local Linux build or production access.

Added actual native-runner DEB service checks for disabled first install,
non-root FIFO PCM, persisted configuration, active/stopped upgrades,
disabled-but-active upgrades, remove/stop and purge/data retention. These and
both package/container architectures still need real CI evidence.

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
data. Container persistence and Fedora RPM tests still require a passing run.

# Design

The gap lives in dist/package-linux.sh (requires both executables) and separate
Linux release workflows (archives only for CLI, no image).

Extend the packaging script with an optional fifth cli/full variant; keep the
four-argument full default used by the existing desktop workflow. CLI templates
use a distinct music-player-cli name and conflict with the combined music-player
package. They exclude desktop files. The DEB also owns the opt-in service, conffile
and maintscripts; RPM owns the CLI and license. Full-package interfaces stay
compatible. RPM postprocessing must preserve the canonical CLI bytes.

Consolidate Linux CLI builds in release.yml and remove its separate arm workflow.
Native Ubuntu 24.04 runners build the embedded Web UI and locked CLI package,
then package the same binary. The runtime Dockerfile installs that generated DEB
on Ubuntu 24.04; it never invokes Cargo. Both native images are tested and saved
as compressed Docker archives, then loaded unchanged by publishing jobs. A final
manifest joins their immutable digests after both architectures passed.

Use read-only default token permissions. A prepare job resolves the source SHA
once, checks release tag/version, and passes metadata to all jobs. PR/default
manual runs produce Actions artifacts only. Explicit publication adds assets to
an existing release, refusing replacements; version image tags are also refused
if already present. No latest tag is maintained initially, avoiding accidental
promotion of prereleases or an old version. Uploads to GitHub and GHCR cannot be
transactional; document partial failure recovery instead of claiming atomicity.

Container uses numeric non-root uid 10001, aligned XDG config/application paths
under /data, exec-form entrypoint, and explicit server mode. Output remains an
application setting; the FIFO example sets it through the existing flat env
key. No bundled Snapserver or extra playback owner.

Files: dist templates/script/README/Dockerfile/compose for package and deployment
contracts; release.yml and its smoke/publish helpers for orchestration; repository
spec, action evidence and checker for the requested engineering rule.

The DEB service uses DynamicUser, StateDirectory and CacheDirectory.
A conffile overrides output/media settings; external FIFO permissions are
an administrator drop-in, not a hardcoded deployment assumption. Native
CI tests real package lifecycle and FIFO writes inside the service sandbox.
Containers do not run systemd; package installation never auto-starts it.
Publishing initializes GHCR with unique run/attempt architecture staging
tags before testing absence of the final version tag.

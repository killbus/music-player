# Live upstream Action audit — 2026-09-29

Queried releases/latest, resolved release commits, fetched and read README.md
and action.yml at each release. Machine-readable URLs and pins are recorded in
.github/action-versions.json.

- checkout v7.0.1: node24, runner >=2.327.1; authenticated container actions need
  >=2.329.0. Set persist-credentials:false; use pull_request, not a privileged
  trigger that executes untrusted PR code.
- upload-artifact v7.0.1: node24; unique immutable artifact names per matrix job;
  zipped upload loses Unix executable modes. Ship binary/image tarballs and
  packages, not a raw executable. if-no-files-found:error catches missing output.
- download-artifact v8.0.1: node24, runner >=2.327.1; defaults digest-mismatch to
  error. name selects one artifact directly; pattern/merge-multiple combines
  both architecture packages (names must not collide).
- setup-bun v2.2.0: node24; bun-version accepts a concrete version. Bun's own
  releases/latest currently reports bun-v1.4.2; pin 1.4.2, keep frozen lockfile.

Source corrections: Linux combined DEB/RPM already covers amd64 AND arm64.
The old cross-repository deployment notes did not verify Debian 11 compatibility,
all configuration relocation, or Emby STRM behavior and are not a runbook for
this work. Only dist/README.md in this repository describes the new pipeline.

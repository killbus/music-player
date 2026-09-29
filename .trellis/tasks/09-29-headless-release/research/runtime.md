# Headless runtime evidence

Checked 2026-09-29 against the locked source and native CI artifacts.

## FIFO contract

- Cargo.lock selects rockbox-playback 0.7.0. The immutable crate from
  https://static.crates.io/crates/rockbox-playback/rockbox-playback-0.7.0.crate
  has SHA256 `29c9f82837524c81bebf5c8590bfdce1f211f6aba0403ea79fe3593bb1c282a9`,
  matching Cargo.lock.
- Its `src/output.rs` defines FIFO output as interleaved stereo S16LE, paced
  in real time. `open_fifo` uses `read(true).write(true)` on an existing path.
  Deployment permissions must therefore grant both read and write, not merely
  write. Keep FIFO creation and ownership with the administrator/Snapserver.
- Its `src/lib.rs:1021` defaults non-CPAL output to 44100 Hz. This application's
  `playback/src/player.rs` sets only the output on PlayerConfig, preserving that
  default. The documented Snapcast format `44100:16:2` follows these sources.

## Native CI 36567622534

https://github.com/killbus/music-player/actions/runs/36567622534

Downloaded both `linux-diagnostics-*` artifacts. Both `systemd-*.json` reports
confirm disabled/stopped first install, non-root decoded FIFO audio, active
upgrade restart, stopped upgrade retention, disabled-but-running upgrade and
remove/purge behavior. Service PCM: amd64 52920 bytes; arm64 49392 bytes.

Both Ubuntu container reports captured nonzero first-playback PCM (amd64 52920;
arm64 56448 bytes). Logs show HTTP listening inside the container after restart,
while the harness continued querying its original random host port and timed
out. Commit 06fe7cd queries the mapping again and records both endpoints and
Docker inspect data; the next run must verify persistence and Fedora tests.

The logs also expose an existing souvlaki 0.8.3 MPRIS worker panic when no session
D-Bus exists. `server/src/media_controls.rs` starts the desktop integration in
a separate thread unconditionally on Linux. This is an application follow-up,
not an established fix in the distribution changes. Retain this diagnostic
when assessing headless readiness; playback and service checks passed despite
it in this run. Do not claim these tests certify desktop media-key integration
or Emby TV/STRM consumption.

## Passing native CI 36574272956

https://github.com/killbus/music-player/actions/runs/36574272956

Both architecture jobs passed at code revision
`06fe7cd0b865acd48aed8507d3a03d416262a214`. The recorded HTTP ports changed
32768 -> 32769 (Ubuntu) and 32770 -> 32771 (Fedora), proving the stale-port
diagnosis. Both container variants now pass persistence after restart.

| Assertion | amd64 | arm64 |
| --- | --- | --- |
| All systemd lifecycle checks | passed | passed |
| Service nonzero PCM bytes | 45864 | 52920 |
| Ubuntu nonzero PCM bytes | 52920 | 52920 |
| Fedora nonzero PCM bytes | 52920 | 52920 |
| Non-root / Web UI / binary identity / persistence | passed | passed |
| DEB and RPM conflict installation checks | passed | passed |

Uploaded packages/evidence:
- amd64: https://github.com/killbus/music-player/actions/runs/36574272956/artifacts/11037857603
- arm64: https://github.com/killbus/music-player/actions/runs/36574272956/artifacts/11037931446

Uploaded tested Docker image archives (seven-day retention):
- amd64: https://github.com/killbus/music-player/actions/runs/36574272956/artifacts/11037997653
- arm64: https://github.com/killbus/music-player/actions/runs/36574272956/artifacts/11037611660

The publish job was skipped as intended. No GHCR manifest or public release was
published by validation. Image assembly, installation and runtime are verified;
registry/release publication still awaits a separately authorized release.

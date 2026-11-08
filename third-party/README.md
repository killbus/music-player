# Patched dependencies

`rockbox-playback-0.9.1` is based on the crates.io source archive from
https://crates.io/crates/rockbox-playback/0.9.1 (GPL-2.0-or-later).
The original archive checksum recorded in upstream Cargo.lock is SHA-256
`21c1a1cfeee1e849b5f71699636a731ff04c61b7edbc5624b1f6af9528a884ee`.
The root `[patch.crates-io]` selects this vendored copy; no 0.7.0 dependency
is used by the migrated workspace.

The fork preserves saturating ReplayGain conversion from i64 to the DSP's
platform C long type, including its boundary test. Windows C long is 32-bit.
The host-stream and output patches described below were ported from the fork's
0.7.0 vendor onto 0.9.1, retaining upstream's newer playback interfaces.
They are still needed for host-owned forward-only stream sessions. Remove a
patch only after an upstream equivalent passes the corresponding fork tests.

Provenance: account-bound source work in fc58c25, managed queue advancement
in bd156b0ac6b79cd0976441288f92da516c796722, and self-cast/readiness work in
2005260. The initial 0.9.1 transplant is 47ae315; 5e2481f permits character
devices such as `fifo:/dev/null` in addition to named pipes for CI sinks.
This changes accepted FIFO sink types, not receiver-consumption guarantees.

The M1b patch adds a host-owned forward-only reader API to Player. Each stream
has its own generation and independent cancellation callback, reader release,
codec status and codec join observations. Stream EOF remains EndUnconfirmed
(not a content integrity witness); pause and replacement cancel the reader.
`StreamSession::clean_end` separately reports clean reader/decoder EOF and
complete output delivery, including pending writer frames. The daemon uses it
with finite-interval timing guards for queue navigation, not listening statistics.
`StreamSession::is_draining` distinguishes clean EOF with pending observable PCM
from unconfirmed completion. Managed EOF retains volume control until the ring
drains, so restoring volume can resume a muted tail. Upstream's zero-volume
buffer freeze is unchanged; silence is not treated as successful media delivery.
The callback must wake blocked IO and reader destruction must join the host
transport. Legacy URL handling remains on the upstream path.

The daemon uses this API for managed sources. The isolated engine experiments
under tools/emby-runtime-probe remain in the old fork history and are deliberately
not included in this migration. FIFO and connected TCP/Unix writers use nonblocking
writes. Cancellation revokes unsent PCM under the same gate as each final write;
partial frame cancellation is zero-padded before subsequent stereo frames.
Normal reader EOF releases input without revoking buffered output. Direct libc
use is Unix-only for O_NONBLOCK and FIFO type validation.

The FIFO experiment holds its reader open without consuming and requires actual
nonzero delivery and WouldBlock before Drop. Output counters include silence and
describe kernel acceptance, not confirmed receiver consumption. Existing stdout
remains blocking and is outside the managed cancellation guarantee; socket
connect/accept during construction is also still blocking. CPAL callback buffers
already handed to the device and bytes already accepted by a kernel/receiver
cannot be retracted by clearing the local ring.

StreamSession::output_snapshot records complete post-DSP media frames per
generation, excluding underrun silence, alignment padding and cancelled partial
frames. ByteStream means kernel acceptance; DeviceBuffer means CPAL callback
submission. Stdout is Unavailable. Output duration is not a source checkpoint:
the host still needs to map pitch/rate and account for receiver buffering.
Retained cancelled sessions keep their final count while newer sessions advance.

The public Player replacement fixture uses distinct 440/880 Hz sources, including
a source stalled before response headers. It checks B audio, cancellation of A,
late cancellation of A while B continues, and resource joins. FIFO evidence checks
that the session clock freezes during backpressure and survives Drop.
Confirmed consumption clocks, latest-intent coordination, TCP/Unix backpressure
and all real Emby acceptance remain separate M1b/M2/M6 gates.

## Regression witnesses

- `src/managed.rs` and its tests cover stream lifecycle and output accounting.
- `src/output_tests.rs` covers cancellation and nonblocking output behavior.
- The workspace's `playback/src/managed/tests.rs` and
  `playback/src/queue_advance_tests.rs` cover clean-end guards, repeat behavior,
  and manual-command precedence. A clean end is not proof of remote listening.
- Workspace CI alone does not imply that the separate vendored crate's tests
  ran, or that real Emby devices and production receivers are compatible.

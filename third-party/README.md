# Patched dependencies

`rockbox-playback-0.7.0` is the crates.io source archive, SHA-256
`29c9f82837524c81bebf5c8590bfdce1f211f6aba0403ea79fe3593bb1c282a9`,
from https://crates.io/crates/rockbox-playback/0.7.0 (GPL-2.0-or-later).

The original source patch converts ReplayGain i64 metadata to the DSP's platform
C long type, saturating at its bounds. On Windows C long is 32-bit; upstream
0.7.0 fails to compile at this call. A boundary test covers preservation and
saturation. Remove this patch when an upstream release fixes the conversion.

The M1b patch adds a host-owned forward-only reader API to Player. Each stream
has its own generation and independent cancellation callback, reader release,
codec status and codec join observations. Stream EOF is EndUnconfirmed and
cannot trigger queue auto-advance; pause and replacement cancel the reader.
The callback must wake blocked IO and reader destruction must join the host
transport. Legacy URL handling remains on the upstream path.

This is an experimental integration under tools/emby-runtime-probe. The daemon
has not adopted it yet. Writer backpressure cancellation, confirmed consumption
clocks, latest-intent coordination and all real Emby/output acceptance remain
separate M1b/M2/M6 gates. A successful TCP fixture is not those guarantees.

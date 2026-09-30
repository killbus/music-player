# Patched dependencies

`rockbox-playback-0.7.0` is the crates.io source archive, SHA-256
`29c9f82837524c81bebf5c8590bfdce1f211f6aba0403ea79fe3593bb1c282a9`,
from https://crates.io/crates/rockbox-playback/0.7.0 (GPL-2.0-or-later).

The sole source change converts ReplayGain i64 metadata to the DSP's platform
C long type, saturating at its bounds. On Windows C long is 32-bit; upstream
0.7.0 fails to compile at this call. A boundary test covers preservation and
saturation. Remove this patch when an upstream release fixes the conversion.

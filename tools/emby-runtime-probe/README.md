# Managed Player runtime probe

Build and run only in GitHub Actions (Emby engine feasibility, runtime job).
This independent locked Cargo package uses the repository's patched Player and
host-owned transport. The sibling emby-engine-probe still uses registry 0.7.0
as a control. Both feed real MP3 through the public Player into a TCP collector.

The fixtures exercise finite/chunked input, blocked headers/body, header auth
positive and negative controls, fake audio, short EOF, pause, and Player drop.
verify.py recomputes cancellation latency from per-generation reader release,
codec join and transport worker exit events. A five-second process watchdog is
failure cleanup; the registered reader/codec cancellation threshold is two seconds.

Results retain raw events, PCM delivery counts, request facts, source hashes,
compiler environment and summaries. Only synthetic loopback credentials are used.
No private server is reached by CI. The transport owns bounded memory and never
materializes a media file. HTTP content length does not grant random access.

The FIFO output experiment keeps a real read descriptor open without reading.
It waits for nonzero PCM delivery and actual writer backpressure, then requests
Player Drop. The two-second cancellation threshold is measured from the child's
pre-call event through its completed Drop event; five seconds is failure cleanup.
Raw evidence is retained in results/output-fifo.json. This checks an opened FIFO
writer, not blocking socket construction, stdout, CPAL, or receiver consumption.
TCP delivery, decoder elapsed time and ring drain are not proof of audible consumption. No automatic media completion is claimed;
short clean decoder EOF retains position as EndUnconfirmed. ServerOffset resume
requires a future host resolver to supply a fresh pinned stream and checkpoint.

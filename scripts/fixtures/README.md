# Local Emby resume instance

This fixture runs the real Rust queue, source resolver, HTTP transport, audio
decoder and GraphQL schema with the current Web UI. The Emby server and media are
synthetic. PCM is counted by a loopback TCP receiver; it does not play on speakers.
This validates the host path, not compatibility with a particular Emby server,
STRM proxy or Snapcast FIFO consumer.
The unrelated `GetAccount` badge query returns a signed-out fixture response so
it cannot initialize the user's shared atradio session. Playback queries,
mutations and subscriptions execute the production schema unchanged.

From the repository root, in separate terminals:

```powershell
node scripts/fixtures/emby-resume.mjs target/emby-local-fixture C:/path/to/ffmpeg.exe
cargo run -p music-player-graphql --example emby_resume_fixture -- target/emby-local-fixture/app
```

Then from `webui/musicplayer`:

```powershell
$env:VITE_API_URL = 'http://127.0.0.1:18098/graphql'
bun run dev --host 127.0.0.1 --port 18099
```

Open <http://127.0.0.1:18099>. The synthetic queue starts paused with two entries.
Ports 18096 (Emby), 18097 (PCM), 18098 (API), and 18099 (UI) bind only to loopback.
Each instance uses an in-memory database and writes settings only to the directory
passed to the Rust example. Stop each process with Ctrl+C. Restart the Rust and
Node instances to reset the test.

On Windows, the native Rockbox C libraries need a GNU compiler. Use the installed
`x86_64-pc-windows-gnu` Rust target, put MinGW GCC/binutils on this terminal's PATH,
and set `CC_x86_64_pc_windows_gnu=gcc`, `AR_x86_64_pc_windows_gnu=ar`, and
`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=gcc`. Add
`--target x86_64-pc-windows-gnu` to the Cargo command.

The full GraphQL dependency graph also needs `protoc` and DuckDB 1.5.5. On
Windows, download the official `protoc` and `libduckdb-windows-amd64.zip` release
archives into the test directory, then set `PROTOC` to `bin/protoc.exe`,
`DUCKDB_LIB_DIR` to the extracted DuckDB directory, and `DUCKDB_STATIC=0`.
Include that directory on PATH when starting the executable so `duckdb.dll`
can be loaded. These are terminal-local environment settings. Do not change
the repository's distribution configuration for this test.

`MUSIC_PLAYER_APPLICATION_DIRECTORY` now applies to settings reads and audio
settings writes as well as queue/cache files. The fixture sets it internally;
the default OS-profile directory is used only when it is unset.

Browser acceptance uses the installed OpenCLI bridge and the user's Chrome:

```powershell
node scripts/fixtures/check-emby-resume.mjs C:/path/to/node_modules/@jackwener/opencli target/emby-local-fixture
```

The script requires captured UI mutations, verifies nonzero PCM, pauses longer
than the signed URL lifetime, checks fresh resolution with the retained offset,
injects HTTP 403, retries through Play, cancels a stalled startup, and waits for
automatic queue advance and stop. Evidence goes to `acceptance.json`,
`server-state.json`, and `final-ui.txt`. Playback is controlled with actual
OpenCLI clicks; GraphQL observation queries and synthetic fault controls are
used alongside captured network traffic to establish the outcome.
The tested OpenCLI extension (1.1.1) can stall coordinate clicks after a background
tab has been idle, so the script dispatches native button clicks through OpenCLI
evaluation. The page's React handlers still issue the playback mutations. CDP
captures the requests; an additional passive fetch observer retains the UI's own
mutation responses because this extension sometimes omits CDP response bodies.

Validated on 2026-11-23: expired-link resume retained 3,680 ms and requested
`StartTimeTicks=36800000` with a new session; HTTP 403 retry, stalled-start cancel,
and automatic `a -> b -> stopped` all passed. The four browser checks complement
9 native playback queue tests, 3 GraphQL status tests, 1 settings isolation test,
and 11 focused frontend tests. This does not close the reported production
incident: the real Emby/STRM upstream and Snapcast output are outside this fixture.

`GET http://127.0.0.1:18096/fixture/state` reports the synthetic request history and
PCM counters. `POST /fixture/mode` accepts `{"mode":"ok"}`, `{"mode":"deny-next"}`
or `{"mode":"slow-next"}` for controlled stream failures. These controls affect
only the local fixture.

/** Synthetic Emby and PCM receiver for graphql/examples/emby_resume_fixture.rs.
 * GET /fixture/state shows resolutions, stream URLs, offsets and nonzero PCM.
 * POST /fixture/mode with {mode:"ok"|"deny-next"|"slow-next"} injects one failure.
 * Every PlaybackInfo creates a new signed stream; cleanup invalidates the old one.
 * All ports bind to loopback. No real media, accounts or external requests.
 */
import http from 'node:http';
import net from 'node:net';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawn } from 'node:child_process';

const output = resolve(process.argv[2] ?? 'target/emby-local-fixture');
mkdirSync(output, { recursive: true });
const ffmpeg = process.argv[3] ?? 'ffmpeg';
const state = { mode: 'ok', resolutions: 0, requests: [], pcmBytes: 0, nonzeroPcmBytes: 0 };
const sessions = new Map();
const durationMs = id => id === 'b' ? 6000 : 60000;
function save() { writeFileSync(resolve(output, 'server-state.json'), JSON.stringify(state, null, 2)); }
function json(res, body, status = 200) {
  res.writeHead(status, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(body));
}
const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1:18096');
  if (url.pathname === '/fixture/state') return json(res, state);
  if (url.pathname === '/fixture/mode' && req.method === 'POST') {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const { mode } = JSON.parse(Buffer.concat(chunks));
    if (!['ok', 'deny-next', 'deny-persistent', 'slow-next'].includes(mode)) return json(res, { error: 'invalid mode' }, 400);
    state.mode = mode; save(); return json(res, { mode });
  }
  state.requests.push({ at: new Date().toISOString(), method: req.method, path: url.pathname, query: Object.fromEntries(url.searchParams) });
  save();
  if (url.pathname === '/Users/AuthenticateByName') {
    return json(res, { AccessToken: 'synthetic-token', ServerId: 'fixture-server', User: { Id: 'fixture-user' } });
  }
  const item = url.pathname.match(/^\/Users\/fixture-user\/Items\/([ab])$/);
  if (item) return json(res, { Id: item[1], Name: `Resume fixture ${item[1]}`, Type: 'Episode', MediaType: 'Video', IsFolder: false, RunTimeTicks: durationMs(item[1]) * 10000 });
  const playback = url.pathname.match(/^\/Items\/([ab])\/PlaybackInfo$/);
  if (playback) {
    const session = `session-${++state.resolutions}`;
    const duration = durationMs(playback[1]);
    sessions.set(session, { expires: Date.now() + 30000, duration }); save();
    return json(res, { PlaySessionId: session, MediaSources: [{ Id: 'version-a', RunTimeTicks: duration * 10000,
      DefaultAudioStreamIndex: 1, MediaStreams: [{ Type: 'Audio', Index: 1, Codec: 'aac', Channels: 2, SampleRate: 44100 }] }] });
  }
  if (url.pathname === '/Videos/ActiveEncodings' && req.method === 'DELETE') {
    sessions.delete(url.searchParams.get('PlaySessionId')); return json(res, {});
  }
  if (/^\/Audio\/[ab]\/stream.mp3$/.test(url.pathname)) {
    const session = url.searchParams.get('PlaySessionId');
    if (state.mode === 'deny-next') { state.mode = 'ok'; save(); return json(res, { error: 'synthetic expired link' }, 403); }
    if (state.mode === 'deny-persistent') { return json(res, { error: 'synthetic expired link' }, 403); }
    if (state.mode === 'slow-next') {
      state.mode = 'ok'; save();
      const timer = setTimeout(() => res.end(), 20000);
      res.on('close', () => clearTimeout(timer)); return;
    }
    res.writeHead(302, { Location: `/signed/${session}.mp3?offset=${Number(url.searchParams.get('StartTimeTicks')) / 10000}` });
    return res.end();
  }
  const signed = url.pathname.match(/^\/signed\/(session-\d+)\.mp3$/);
  if (signed) {
    const lease = sessions.get(signed[1]);
    if (!lease || lease.expires < Date.now()) return json(res, { error: 'expired fixture session' }, 403);
    const remaining = Math.max(0.01, (lease.duration - Number(url.searchParams.get('offset'))) / 1000);
    const encoder = spawn(ffmpeg, ['-v', 'error', '-f', 'lavfi', '-i',
      `sine=frequency=440:sample_rate=44100:duration=${remaining}`,
      '-ac', '2', '-c:a', 'libmp3lame', '-b:a', '128k', '-write_xing', '0', '-id3v2_version', '0', '-f', 'mp3', 'pipe:1'],
      { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    encoder.once('error', error => json(res, { error: error.message }, 500));
    encoder.stderr.on('data', data => process.stderr.write(data));
    res.setHeader('Content-Type', 'audio/mpeg');
    encoder.stdout.pipe(res);
    res.on('close', () => encoder.kill());
    return;
  }
  json(res, { error: 'unknown fixture route' }, 404);
});
const pcm = net.createServer(socket => {
  socket.on('data', chunk => {
    state.pcmBytes += chunk.length;
    for (const byte of chunk) if (byte) state.nonzeroPcmBytes++;
  });
  socket.on('error', error => {
    if (error.code !== 'ECONNRESET') console.error(error);
  });
});
server.listen(18096, '127.0.0.1');
pcm.listen(18097, '127.0.0.1');
const timer = setInterval(save, 1000);
console.log(JSON.stringify({ emby: 'http://127.0.0.1:18096', pcm: '127.0.0.1:18097', output }));
process.on('SIGINT', () => { clearInterval(timer); save(); server.close(); pcm.close(); process.exit(0); });

/** OpenCLI acceptance against the isolated local instance. Requires all three
 * fixture services and the user's Chrome/OpenCLI extension. Writes a report;
 * closes its own tab and leaves the local queue stopped. No production requests.
 * node scripts/fixtures/check-emby-resume.mjs OPENCLI_ROOT [ARTIFACT_DIRECTORY]
 * Validated 2026-11-23 with OpenCLI 1.9.0 / extension 1.1.1. Playback buttons
 * use DOM clicks via OpenCLI because background coordinate clicks can stall.
 */
import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';

const directory = resolve(process.argv[3] ?? 'target/emby-local-fixture');
mkdirSync(directory, { recursive: true });
const { BrowserBridge } = await import(pathToFileURL(join(process.argv[2], 'dist/src/browser/bridge.js')));
const bridge = new BrowserBridge();
const session = `local-emby-${randomUUID().slice(0, 8)}`;
const report = { session, started: new Date().toISOString(), checks: [], samples: [], network: [] };
const network = new Map();
let page;
const save = () => writeFileSync(join(directory, 'acceptance.json'), JSON.stringify(report, null, 2));
const fixture = () => JSON.parse(readFileSync(join(directory, 'server-state.json'), 'utf8'));
async function capture() {
  for (const entry of await page.readNetworkCapture()) {
    const key = `${entry.timestamp}:${entry.method}:${entry.url}`;
    network.set(key, { ...network.get(key), ...entry });
  }
  report.network = [...network.values()];
  save();
}
async function sample(label) {
  const snapshot = await page.evaluate(`(async () => {
    const response = await fetch('http://127.0.0.1:18098/graphql', {
      method:'POST',headers:{'Content-Type':'application/json'},signal:AbortSignal.timeout(10000),
      body:JSON.stringify({query:'query LocalPlaybackObservation { currentlyPlayingSong { track { id title } index isPlaying positionMs } sourcePlayback { source occurrenceId phase error } }'})
    });
    return {at:new Date().toISOString(),body:await response.json(),
      buttons:[...document.querySelectorAll('button')].map(b=>b.getAttribute('aria-label')).filter(Boolean),
      alerts:[...document.querySelectorAll('[role="alert"]')].map(e=>e.textContent)};
  })()`);
  assert.equal(snapshot.body.errors, undefined, JSON.stringify(snapshot.body.errors));
  report.samples.push({ label, ...snapshot, fixture: fixture() }); save();
  return snapshot.body.data;
}
async function until(label, predicate, timeout = 25000) {
  const deadline = Date.now() + timeout;
  while (true) {
    const data = await sample(label);
    if (predicate(data)) return data;
    assert.ok(Date.now() < deadline, `${label} exceeded deadline`);
    await delay(700);
  }
}
async function click(label, mutation) {
  await capture();
  const selector = `button[aria-label="${label}"]:not(:disabled)`;
  const readyDeadline = Date.now() + 10000;
  while (await page.evaluate(`document.querySelectorAll(${JSON.stringify(selector)}).length`) !== 1) {
    assert.ok(Date.now() < readyDeadline, `expected exactly one enabled ${label} button`);
    await delay(400);
  }
  const clickedAt = Date.now();
  const responseIndex = await page.evaluate('window.__fixtureControlResponses.length');
  // The installed extension's coordinate click waits on animation frames and
  // can stall for minutes in an isolated background window after idle. Dispatch
  // the button's native DOM click through OpenCLI; React still issues the command.
  await page.evaluate(`(() => {
    const button = document.querySelector(${JSON.stringify(selector)});
    if (!button) throw new Error('Playback button disappeared');
    button.click();
  })()`);
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    await delay(400); await capture();
    const request = report.network.find(entry => {
      const body = entry.requestBodyPreview;
      return entry.timestamp >= clickedAt &&
        (typeof body === 'string' ? body : JSON.stringify(body))?.includes(`mutation ${mutation}`);
    });
    const response = await page.evaluate(`window.__fixtureControlResponses.slice(${responseIndex}).find(r => r.operation === ${JSON.stringify(mutation)}) ?? null`);
    if (request && response) {
      assert.equal(response.status, 200, `${mutation} HTTP response`);
      assert.equal(response.body.errors, undefined, JSON.stringify(response.body.errors));
      assert.equal(response.body.data[mutation.toLowerCase()], true, `${mutation} was rejected`);
      // CDP may drain a request before response metadata arrives. Independently
      // observe the UI's own fetch response; this never sends a playback command.
      report.checks.push({ action: label, mutation, captured: true,
        cdpResponseBodyCaptured: !!request.responsePreview, response, at: new Date().toISOString() });
      return;
    }
  }
  throw new Error(`${label} click did not produce a captured ${mutation} request`);
}
async function mode(value) {
  const response = await fetch('http://127.0.0.1:18096/fixture/mode', {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ mode: value }),
  });
  assert.equal(response.status, 200);
}
try {
  const health = await fetch('http://127.0.0.1:18098/graphql', { method: 'POST',
    headers: { 'Content-Type': 'application/json' }, signal: AbortSignal.timeout(5000),
    body: JSON.stringify({ query: '{ playerRuntime { phase error } }' }) });
  const runtime = await health.json();
  assert.equal(runtime.data?.playerRuntime.phase, 'Ready', JSON.stringify(runtime));
  page = await bridge.connect({ session, windowMode: 'isolated' });
  await page.goto('http://127.0.0.1:18099');
  assert.equal(new URL(await page.getCurrentUrl()).origin, 'http://127.0.0.1:18099');
  assert.ok(await page.startNetworkCapture('/graphql'));
  await page.evaluate(`(() => {
    const fetch = window.fetch.bind(window);
    window.__fixtureControlResponses = [];
    window.fetch = async (...args) => {
      const response = await fetch(...args);
      try {
        const query = typeof args[1]?.body === 'string' ? JSON.parse(args[1].body).query : '';
        const operation = query?.match(/mutation (Play|Pause)\\s*\\{/u)?.[1];
        if (operation) response.clone().json().then(body =>
          window.__fixtureControlResponses.push({operation, status:response.status, body})).catch(() => {});
      } catch {}
      return response;
    };
  })()`);
  const baseline = await until('ready', d => d.sourcePlayback?.phase === 'PAUSED');
  await page.snapshot();
  await click('Play', 'Play');
  await until('first-play', d => d.currentlyPlayingSong.isPlaying && d.currentlyPlayingSong.positionMs > 0);
  await until('ui-playing', d => d.currentlyPlayingSong.isPlaying);
  // UI polling is a fallback when subscription delivery has not arrived yet.
  await delay(1500);
  await click('Pause', 'Pause');
  const paused = await until('paused', d => d.sourcePlayback?.phase === 'PAUSED');
  const offset = paused.currentlyPlayingSong.positionMs;
  const oldResolutions = fixture().resolutions;
  await delay(35000); // Exceed the synthetic signed URL lifetime.
  const retained = await sample('expired-while-paused');
  assert.equal(retained.currentlyPlayingSong.positionMs, offset);
  assert.equal(retained.sourcePlayback.occurrenceId, baseline.sourcePlayback.occurrenceId);
  const pcmBefore = fixture().nonzeroPcmBytes;
  await click('Play', 'Play');
  const resumed = await until('resumed', d => d.currentlyPlayingSong.isPlaying && d.currentlyPlayingSong.positionMs > offset);
  assert.equal(resumed.sourcePlayback.occurrenceId, paused.sourcePlayback.occurrenceId);
  assert.ok(fixture().resolutions > oldResolutions, 'resume must resolve a fresh session');
  await delay(1200);
  assert.ok(fixture().nonzeroPcmBytes > pcmBefore, 'resume must deliver nonzero PCM');
  const audio = fixture().requests.filter(r => r.path.startsWith('/Audio/')).at(-1);
  assert.equal(Number(audio.query.StartTimeTicks), offset * 10000);
  assert.equal(audio.query.AudioStreamIndex, '1');
  report.checks.push({ test: 'expired-link-resume', passed: true, offset });

  await click('Pause', 'Pause');
  await until('pause-before-failure', d => d.sourcePlayback?.phase === 'PAUSED');
  // 1. Transient 403 automatically recovers without user clicking Play.
  await mode('deny-next');
  await click('Play', 'Play');
  await until('transient-recovery', d => d.sourcePlayback?.phase === 'RECOVERING');
  const autoResumed = await until('transient-auto-resumed', d => d.currentlyPlayingSong.isPlaying && d.sourcePlayback?.phase === 'STREAMING');
  assert.ok(autoResumed.currentlyPlayingSong.positionMs > 0);
  report.checks.push({ test: 'transient-403-auto-recovery', passed: true });

  // 2. Persistent 403 exhausts recovery attempts and lands on FAILED.
  await click('Pause', 'Pause');
  await until('pause-before-persistent-failure', d => d.sourcePlayback?.phase === 'PAUSED');
  await mode('deny-persistent');
  await click('Play', 'Play');
  const failed = await until('persistent-http-failed', d => d.sourcePlayback?.phase === 'FAILED', 25000);
  assert.equal(failed.currentlyPlayingSong.isPlaying, false);
  await mode('ok');
  await page.screenshot({ path: join(directory, 'recovery-exhausted.png') });
  await click('Play', 'Play');
  await until('failure-retry', d => d.currentlyPlayingSong.isPlaying && d.currentlyPlayingSong.positionMs > failed.currentlyPlayingSong.positionMs);
  report.checks.push({ test: 'http-error-retry', passed: true });

  await click('Pause', 'Pause');
  const beforeSlow = await until('pause-before-slow-start', d => d.sourcePlayback?.phase === 'PAUSED');
  await mode('slow-next');
  await click('Play', 'Play');
  await until('slow-start', d => d.sourcePlayback?.phase === 'OPENING');
  await click('Cancel playback startup', 'Pause');
  const cancelled = await until('startup-cancelled', d => d.sourcePlayback?.phase === 'PAUSED');
  assert.equal(cancelled.currentlyPlayingSong.positionMs, beforeSlow.currentlyPlayingSong.positionMs);
  assert.equal(cancelled.currentlyPlayingSong.isPlaying, false);
  report.checks.push({ test: 'cancel-slow-start', passed: true });
  await click('Play', 'Play');
  await until('retry-after-cancel', d => d.currentlyPlayingSong.isPlaying);
  const next = await until('automatic-next', d => d.currentlyPlayingSong.track?.title === 'Resume fixture b', 80000);
  assert.notEqual(next.sourcePlayback.occurrenceId, paused.sourcePlayback.occurrenceId);
  await until('queue-exhausted', d => d.sourcePlayback?.phase === 'STOPPED', 18000);
  const items = fixture().requests.filter(r => r.path.startsWith('/Audio/')).map(r => r.path.split('/')[2]);
  assert.equal(items.at(-1), 'b');
  assert.equal(items.filter(id => id === 'b').length, 1);
  report.checks.push({ test: 'automatic-queue-advance-and-stop', passed: true });
  report.passed = true;
} catch (error) {
  report.error = String(error.stack ?? error);
  process.exitCode = 1;
} finally {
  if (page) {
    try { await capture(); writeFileSync(join(directory, 'final-ui.txt'), String(await page.snapshot())); }
    catch (error) { report.captureError = String(error); }
    try { await page.screenshot({ path: join(directory, 'final-ui.png') }); } catch {}
    if (!report.passed) {
      try {
        // Failure cleanup is separate from the browser-driven acceptance checks.
        await fetch('http://127.0.0.1:18098/graphql', { method: 'POST',
          headers: { 'Content-Type': 'application/json' }, signal: AbortSignal.timeout(5000),
          body: JSON.stringify({ query: 'mutation FixtureCleanup { pause }' }) });
      } catch {}
    }
    try { await page.closeWindow(); report.sessionClosed = true; } catch {}
  }
  await bridge.close();
  report.finished = new Date().toISOString(); save();
  console.log(JSON.stringify({ artifact: join(directory, 'acceptance.json'), passed: report.passed, checks: report.checks, error: report.error }));
}

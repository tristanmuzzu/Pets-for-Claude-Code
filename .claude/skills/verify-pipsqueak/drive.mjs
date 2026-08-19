#!/usr/bin/env node
/*
 * Drives the real Pipsqueak UI and captures evidence. Zero dependencies:
 * Chrome over CDP, using the global WebSocket that Node 22 ships.
 *
 *   node .claude/skills/verify-pipsqueak/drive.mjs doctor
 *   node .claude/skills/verify-pipsqueak/drive.mjs hint
 *   node .claude/skills/verify-pipsqueak/drive.mjs all
 *
 * Assumes `npm run dev` is already serving on APP_PORT. Launches its own
 * headless Chrome on a private profile and port, and kills only that one.
 */
import { spawn } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const APP_PORT = Number(process.env.APP_PORT || 1420);
const APP_URL = `http://localhost:${APP_PORT}/`;
const CDP_PORT = Number(process.env.CDP_PORT || 9333);
const EVIDENCE = process.env.EVIDENCE_DIR || '.verify/evidence';
const CHROME = process.env.CHROME_BIN || 'google-chrome';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log(...a);

async function waitFor(fn, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    try { if (await fn()) return true; } catch (e) { last = e; }
    await sleep(200);
  }
  throw new Error(`timed out waiting for ${label}${last ? `: ${last.message}` : ''}`);
}

// --- CDP ---------------------------------------------------------------
class Session {
  constructor(ws) { this.ws = ws; this.id = 0; this.pending = new Map(); }
  static async open(wsUrl) {
    const ws = new WebSocket(wsUrl);
    const s = new Session(ws);
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(ev.data);
      const p = s.pending.get(msg.id);
      if (!p) return;
      s.pending.delete(msg.id);
      msg.error ? p.reject(new Error(msg.error.message)) : p.resolve(msg.result);
    });
    await new Promise((res, rej) => {
      ws.addEventListener('open', res, { once: true });
      ws.addEventListener('error', () => rej(new Error('CDP socket failed')), { once: true });
    });
    return s;
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async eval(expression) {
    const r = await this.send('Runtime.evaluate', {
      expression, awaitPromise: true, returnByValue: true,
    });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text + ' ' + (r.exceptionDetails.exception?.description || ''));
    return r.result.value;
  }
  close() { try { this.ws.close(); } catch { /* already gone */ } }
}

async function launchChrome(profileDir) {
  const child = spawn(CHROME, [
    '--headless=new',
    `--remote-debugging-port=${CDP_PORT}`,
    `--user-data-dir=${profileDir}`,
    '--no-first-run', '--no-default-browser-check',
    '--window-size=1280,800',
    'about:blank',
  ], { stdio: 'ignore', detached: false });
  await waitFor(async () => {
    const r = await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`).catch(() => null);
    return r && r.ok;
  }, 15000, 'chrome devtools endpoint');
  return child;
}

async function openPage() {
  const r = await fetch(`http://127.0.0.1:${CDP_PORT}/json/new?${encodeURIComponent(APP_URL)}`, { method: 'PUT' });
  const target = await r.json();
  const s = await Session.open(target.webSocketDebuggerUrl);
  await s.send('Runtime.enable');
  await s.send('Page.enable');
  return s;
}

// --- checks ------------------------------------------------------------

// Doctor: is this instance worth driving? Read-only, no interaction.
async function doctor(s) {
  const out = await s.eval(`(() => JSON.stringify({
    title: document.title,
    isTauri: '__TAURI_INTERNALS__' in window,
    pet: !!document.getElementById('pet'),
    hint: !!document.getElementById('hint'),
    viewport: [innerWidth, innerHeight],
  }))()`);
  const d = JSON.parse(out);
  const problems = [];
  if (d.title !== 'Pipsqueak') problems.push(`wrong page, title is "${d.title}"`);
  if (!d.pet) problems.push('#pet missing');
  if (!d.hint) problems.push('#hint missing');
  // The pet parks itself off-screen at -104,-104 while the viewport is 0x0,
  // so every hit test silently misses. Catch it here, not three steps later.
  if (!d.viewport[0] || !d.viewport[1]) problems.push(`viewport is ${d.viewport.join('x')}, the pet will be off-screen`);
  return { ...d, ok: problems.length === 0, problems };
}

// Feature: hover hint. Covers the three fixes in e242576, bb55af0, 74f7262.
async function hintLifecycle(s) {
  const out = await s.eval(`(async () => {
    const pet = document.getElementById('pet'), hint = document.getElementById('hint');
    const r = pet.getBoundingClientRect();
    const cx = Math.round(r.x + r.width / 2), cy = Math.round(r.y + r.height / 2);
    const sleep = ms => new Promise(s => setTimeout(s, ms));
    const ev = (t, x, y) => pet.dispatchEvent(new PointerEvent(t, {
      clientX: x, clientY: y, bubbles: true, cancelable: true, pointerId: 1, pointerType: 'mouse',
    }));
    // A real hovering cursor keeps emitting pointermove ON the pet. The
    // capture-phase listener at src/main.js:1466 reads ANY move that is not
    // over the pet as a departure and kills the pending hint, so a probe that
    // sends one pointerenter and then goes silent is not a hovering user, it
    // is a user who left. Dwell the way a hand does.
    const dwell = async (ms) => {
      const until = Date.now() + ms;
      while (Date.now() < until) { ev('pointermove', cx, cy); await sleep(150); }
    };
    const o = { petBox: [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)] };
    o.beforeHover = hint.hidden;
    ev('pointerenter', cx, cy);
    await dwell(1200); o.at1200ms = hint.hidden;
    await dwell(1800); o.at3000ms = hint.hidden;
    o.hintBox = (() => { const h = hint.getBoundingClientRect(); return [Math.round(h.x), Math.round(h.y), Math.round(h.width), Math.round(h.height)]; })();
    ev('pointerleave', 20, 20);
    await sleep(400); o.afterLeave = hint.hidden;
    ev('pointerenter', cx, cy);
    await dwell(3000); o.secondVisit = hint.hidden;
    return JSON.stringify(o);
  })()`);
  const o = JSON.parse(out);
  const checks = [
    ['hidden before the cursor arrives', o.beforeHover === true],
    ['still hidden at 1200ms, so a passing cursor never triggers it', o.at1200ms === true],
    ['showing by 3000ms (HINT_DELAY_MS is 2500)', o.at3000ms === false],
    ['positioned beside the pet, not at the origin', o.hintBox[0] > 0 && o.hintBox[1] > 0],
    ['hidden again once the cursor leaves (74f7262)', o.afterLeave === true],
    ['shows again on a second visit (bb55af0, e242576)', o.secondVisit === false],
  ];
  return { observed: o, checks, ok: checks.every(([, p]) => p) };
}

async function shot(s, file) {
  const r = await s.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(file, Buffer.from(r.data, 'base64'));
  return file;
}

// --- main --------------------------------------------------------------
const what = process.argv[2] || 'all';
const profile = mkdtempSync(join(tmpdir(), 'pipsqueak-verify-'));
let chrome = null, session = null, failed = false;

try {
  await waitFor(async () => {
    const r = await fetch(APP_URL).catch(() => null);
    return r && r.ok;
  }, 5000, `the dev server on ${APP_URL} (run "npm run dev" first)`);

  chrome = await launchChrome(profile);
  session = await openPage();
  await waitFor(async () => (await session.eval(`!!document.getElementById('pet')`)) === true, 10000, '#pet to render');

  mkdirSync(EVIDENCE, { recursive: true });
  const evidence = { url: APP_URL, ranAt: new Date().toISOString() };

  const d = await doctor(session);
  evidence.doctor = d;
  log(`doctor: ${d.ok ? 'ok' : 'PROBLEMS'} ${d.problems.join('; ')}`);
  if (!d.ok) failed = true;

  if (!failed && (what === 'hint' || what === 'all')) {
    const h = await hintLifecycle(session);
    evidence.hint = h;
    for (const [name, pass] of h.checks) log(`  ${pass ? 'PASS' : 'FAIL'}  ${name}`);
    if (!h.ok) failed = true;
    evidence.screenshot = await shot(session, join(EVIDENCE, 'hint-after-run.png'));
  }

  writeFileSync(join(EVIDENCE, 'run.json'), JSON.stringify(evidence, null, 2));
  log(`\nevidence: ${join(EVIDENCE, 'run.json')}`);
} catch (e) {
  failed = true;
  console.error(`drive failed: ${e.message}`);
} finally {
  // Cleanup removes what this run started, never the evidence.
  session?.close();
  if (chrome) { chrome.kill('SIGTERM'); await sleep(300); chrome.kill('SIGKILL'); }
  rmSync(profile, { recursive: true, force: true });
}

process.exit(failed ? 1 : 0);

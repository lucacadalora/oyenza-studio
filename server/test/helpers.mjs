// Shared bits for the meeting server tests (not a test file itself).
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { createDropServer } from '../drop-server.mjs';

export const b64u = (buf) => Buffer.from(buf).toString('base64url');

// room, proof and K for a 16-byte secret, with node:crypto (independent of drop-core.js).
export function derive(secret) {
  const S = Buffer.isBuffer(secret) ? secret : Buffer.from(secret, 'base64url');
  const h = (label) => crypto.createHash('sha256').update(Buffer.from(label, 'utf8')).update(S).digest();
  return { S: b64u(S), room: b64u(h('oyen-drop/1 room')), proof: b64u(h('oyen-drop/1 proof')), K: h('oyen-drop/1 mac') };
}

export function sign(keys, { t, sid, ts = Date.now(), name = '', sdp = '' }) {
  const text = ['oyen-drop/1', keys.room, t, sid, String(ts), name, sdp].join('\n');
  const mac = b64u(crypto.createHmac('sha256', keys.K).update(text, 'utf8').digest());
  return { t, sid, ts, name, sdp, mac };
}

export const newPair = () => derive(crypto.randomBytes(16));
export const newSid = () => b64u(crypto.randomBytes(16));
export const rp = (k) => ({ room: k.room, proof: k.proof });

export function tempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'drop-server-test-'));
}

// Starts a server on a free port. Returns { base, drop, call, logs, close }.
export async function start(options = {}) {
  const logs = [];
  const dataDir = options.dataDir || tempDir();
  const drop = createDropServer({ dataDir, logError: (l) => logs.push(l), ...options });
  const addr = await drop.listen(0, '127.0.0.1');
  const base = `http://127.0.0.1:${addr.port}`;
  async function call(route, body, { method = 'POST', headers = {}, raw = null, signal } = {}) {
    const init = { method, headers: { ...headers }, signal };
    if (raw !== null) init.body = raw;
    else if (body !== undefined) {
      init.body = JSON.stringify(body);
      init.headers['Content-Type'] = 'application/json';
    }
    const r = await fetch(base + route, init);
    const text = await r.text();
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {
      // not JSON
    }
    return { status: r.status, json, text, headers: r.headers, ms: 0 };
  }
  // Like call, but also measures how long the answer took.
  async function timed(route, body, opts) {
    const t0 = Date.now();
    const r = await call(route, body, opts);
    r.ms = Date.now() - t0;
    return r;
  }
  return { base, drop, call, timed, logs, dataDir, close: () => drop.close() };
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

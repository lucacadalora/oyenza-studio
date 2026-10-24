#!/usr/bin/env node
// Oyen Poni meeting server for paired phone drop (MIT).
//
// The contract is docs/paired-drop.md. Node 20+, no dependencies. It tells a
// phone whether its PC is online, passes the two sides' signed connection
// details, hands out relay passes, and serves the phone page in ./public.
// Everything lives in memory except the list of removed rooms (gone.json).
// It never logs rooms, proofs, messages or addresses.

import http from 'node:http';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

export const DEFAULTS = Object.freeze({
  holdMs: 25_000, // longest a wait is held open
  onlineMs: 35_000, // a PC counts as online this long after its last wait ended
  sessionMs: 120_000, // a phone session lives this long after its last use
  rateLimit: 120, // requests per address per window (a long poll is one request)
  rateWindowMs: 60_000,
  bodyMax: 64 * 1024,
  sdpMax: 32 * 1024,
  nameMax: 60,
  roomsPerRequest: 32,
  roomsMax: 200_000, // rooms kept in memory; the least recently used idle one makes way
  roomIdleMs: 30 * 24 * 3600_000, // an unused room is forgotten after this
  queueMax: 20, // queued messages per room (to the PC) and per phone session
  sessionsMax: 100_000,
  sessionsPerRoom: 32,
  goneMs: 30 * 24 * 3600_000, // removed rooms are remembered 30 days
  goneMax: 200_000,
  kickMs: 2_000, // a kick that found no open wait ends the client's next wait within this time
  kicksMax: 50_000,
  bucketsMax: 200_000,
  iceCacheMs: 50 * 60_000, // one relay pass set (valid 1 h) is shared for 50 minutes
  iceRetryMs: 60_000, // after a failed relay request, STUN only for this long
  iceTimeoutMs: 5_000,
  // Relay passes cost money once the free tier is used up, and anyone can make
  // up a room, so they are rationed: only for a room whose PC is online, at most
  // this many per address per hour, and this many in all per hour.
  turnPerAddressHour: 30,
  turnPerHour: 600,
  sweepMs: 5_000,
});

const STUN = Object.freeze({ urls: ['stun:stun.cloudflare.com:3478'] });
const TURN_API = 'https://rtc.live.cloudflare.com/v1/turn/keys/';

const SECURITY_HEADERS = [
  ['Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; manifest-src 'self'; worker-src 'self'; base-uri 'none'; frame-ancestors 'none'"],
  ['Referrer-Policy', 'no-referrer'],
  ['X-Content-Type-Options', 'nosniff'],
  ['Permissions-Policy', 'camera=(), microphone=(), geolocation=()'],
];

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.webmanifest': 'application/manifest+json',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
  '.webp': 'image/webp',
  '.woff2': 'font/woff2',
  '.txt': 'text/plain; charset=utf-8',
};

// ---------- validation ----------

const B64U_RE = /^[A-Za-z0-9_-]+$/;
// The last character of canonical b64u carries only the bits that exist.
const LAST_43 = /[AEIMQUYcgkosw048]$/; // 32 bytes: 2 spare bits
const LAST_22 = /[AQgw]$/; // 16 bytes: 4 spare bits
const T_RE = /^[a-z][a-z-]{0,15}$/;
const CONTROL_RE = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/;
const MSG_KEYS = ['t', 'sid', 'ts', 'name', 'sdp', 'mac'];

export const idOk = (v) => typeof v === 'string' && v.length === 43 && B64U_RE.test(v) && LAST_43.test(v);
export const sidOk = (v) => typeof v === 'string' && v.length === 22 && B64U_RE.test(v) && LAST_22.test(v);

function nameOk(v, max) {
  if (typeof v !== 'string' || CONTROL_RE.test(v)) return false;
  let n = 0;
  for (const _ of v) if (++n > max) return false;
  return true;
}

function msgOk(m, o) {
  if (!m || typeof m !== 'object' || Array.isArray(m)) return false;
  const keys = Object.keys(m);
  if (keys.length !== MSG_KEYS.length || !MSG_KEYS.every((k) => Object.prototype.hasOwnProperty.call(m, k))) return false;
  return (
    typeof m.t === 'string' && T_RE.test(m.t) &&
    sidOk(m.sid) &&
    Number.isSafeInteger(m.ts) && m.ts >= 0 &&
    nameOk(m.name, o.nameMax) &&
    typeof m.sdp === 'string' && m.sdp.length <= o.sdpMax && Buffer.byteLength(m.sdp, 'utf8') <= o.sdpMax &&
    idOk(m.mac)
  );
}

function roomListOk(list, max) {
  if (!Array.isArray(list) || list.length < 1 || list.length > max) return false;
  return list.every((e) => e && typeof e === 'object' && !Array.isArray(e) && idOk(e.room) && idOk(e.proof));
}

// ---------- addresses ----------

// IPv6 addresses are grouped by /64 (one home or phone usually owns a whole /64).
function expandV6(a) {
  const pct = a.indexOf('%');
  if (pct >= 0) a = a.slice(0, pct);
  const halves = a.split('::');
  if (halves.length > 2) return null;
  const part = (s) => {
    if (!s) return [];
    const g = s.split(':');
    const last = g[g.length - 1];
    if (last.includes('.')) {
      const q = last.split('.').map(Number);
      if (q.length !== 4 || q.some((n) => !Number.isInteger(n) || n < 0 || n > 255)) return null;
      g.splice(g.length - 1, 1, ((q[0] << 8) | q[1]).toString(16), ((q[2] << 8) | q[3]).toString(16));
    }
    return g;
  };
  const left = part(halves[0]);
  const right = halves.length === 2 ? part(halves[1]) : [];
  if (!left || !right) return null;
  const fill = 8 - left.length - right.length;
  if (halves.length === 1 ? fill !== 0 : fill < 1) return null;
  const groups = [...left, ...Array(halves.length === 2 ? fill : 0).fill('0'), ...right];
  if (groups.length !== 8 || !groups.every((x) => /^[0-9a-fA-F]{1,4}$/.test(x))) return null;
  return groups.map((x) => parseInt(x, 16).toString(16));
}

export function addressKey(a) {
  a = String(a || '').trim();
  if (/^::ffff:\d+\.\d+\.\d+\.\d+$/i.test(a)) return a.slice(7);
  if (!a.includes(':')) return a;
  const g = expandV6(a);
  return g ? `${g.slice(0, 4).join(':')}::/64` : a;
}

// ---------- the server ----------

export function createDropServer(options = {}) {
  const o = { ...DEFAULTS };
  for (const [k, v] of Object.entries(options)) if (k in DEFAULTS && v !== undefined) o[k] = v;
  const publicDir = options.publicDir || path.join(HERE, 'public');
  const dataDir = options.dataDir || path.join(HERE, 'data');
  const goneFile = path.join(dataDir, 'gone.json');
  const trustProxy = Boolean(options.trustProxy);
  const turnKeyId = options.turnKeyId || '';
  const turnToken = options.turnToken || '';
  const fetchImpl = options.fetch || globalThis.fetch;
  const logError = options.logError || ((line) => console.error(line));

  // room id -> { ph, used, lastEnd, waits:Set, queue:[{msg, exp}], sessions:Set }
  const rooms = new Map(); // in least-recently-used order
  const gone = new Map(); // room id -> Unix ms it was removed (oldest first)
  const sessions = new Map(); // `${room} ${sid}` -> { room, sid, queue:[msg], waiter, exp }
  const queued = new Set(); // rooms with messages waiting for the PC
  const clients = new Map(); // PC client id -> its open wait
  const kicks = new Map(); // PC client id -> Unix ms of a kick that found no open wait
  const buckets = new Map(); // address -> { start, n }
  const files = loadStatic(publicDir);

  const turnUse = new Map(); // address -> { start, n } relay passes handed out this hour
  let turnHour = { start: 0, n: 0 };
  let turnCapLogged = 0;
  let iceCache = null; // { servers, exp }
  let iceInflight = null;
  let iceFailUntil = 0;
  let goneTimer = null;
  let lastRoomSweep = 0;

  loadGone();

  const BAD = 'bad';
  const GONE = 'gone';
  const FULL = 'full';

  // ----- helpers -----

  function sendJson(res, status, obj, headers = {}) {
    if (res.headersSent || res.writableEnded || res.destroyed) return;
    const body = Buffer.from(JSON.stringify(obj));
    try {
      res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Content-Length': body.length, ...headers });
      res.end(body);
    } catch {
      // the client went away
    }
  }

  const fail = (res, status, error, headers) => sendJson(res, status, { ok: false, error }, headers);
  const badRequest = (res) => fail(res, 400, 'bad-request');
  const slowDown = (res) => fail(res, 429, 'slow-down', { 'Retry-After': '5' });

  function sendText(res, status, text, headers = {}) {
    if (res.headersSent) return;
    const body = Buffer.from(text);
    res.writeHead(status, { 'Content-Type': 'text/plain; charset=utf-8', 'Content-Length': body.length, 'Cache-Control': 'no-store', ...headers });
    res.end(body);
  }

  function proofHash(proof) {
    return crypto.createHash('sha256').update(proof, 'utf8').digest();
  }

  // Looks a room up (registering it on first use) and checks the proof.
  function useRoom(id, proof, now) {
    if (gone.has(id)) return GONE;
    const ph = proofHash(proof);
    let r = rooms.get(id);
    if (r) {
      if (!crypto.timingSafeEqual(r.ph, ph)) return BAD;
      rooms.delete(id); // move to the most recently used end
      rooms.set(id, r);
      r.used = now;
      return r;
    }
    if (rooms.size >= o.roomsMax && !evictIdleRoom()) return FULL;
    r = { ph, used: now, lastEnd: null, waits: new Set(), queue: [], sessions: new Set() };
    rooms.set(id, r);
    return r;
  }

  const idle = (r) => r.waits.size === 0 && r.sessions.size === 0 && r.queue.length === 0;

  function evictIdleRoom() {
    for (const [id, r] of rooms) {
      if (idle(r)) {
        rooms.delete(id);
        return true;
      }
    }
    return false;
  }

  const online = (r, now) => r.waits.size > 0 || (r.lastEnd !== null && now - r.lastEnd < o.onlineMs);

  function roomError(res, r) {
    if (r === GONE) return fail(res, 410, 'gone');
    if (r === BAD) return fail(res, 403, 'bad');
    return slowDown(res); // FULL
  }

  function liveSession(room, sid, now) {
    const s = sessions.get(`${room} ${sid}`);
    if (!s) return null;
    if (!s.waiter && s.exp <= now) {
      dropSession(s);
      return null;
    }
    return s;
  }

  function dropSession(s) {
    sessions.delete(`${s.room} ${s.sid}`);
    const r = rooms.get(s.room);
    if (r) r.sessions.delete(s);
  }

  // ----- PC waits -----

  function answerPcWait(w) {
    if (!w.open) return;
    w.open = false;
    clearTimeout(w.timer);
    const now = Date.now();
    const msgs = [];
    const goneList = [...w.gone];
    for (const id of w.rooms) {
      const r = rooms.get(id);
      if (!r) {
        if (gone.has(id) && !goneList.includes(id)) goneList.push(id);
        continue;
      }
      r.waits.delete(w);
      r.lastEnd = now;
      if (r.queue.length) {
        for (const q of r.queue) if (q.exp > now) msgs.push({ room: id, msg: q.msg });
        r.queue = [];
        queued.delete(id);
      }
    }
    if (clients.get(w.client) === w) clients.delete(w.client);
    sendJson(w.res, 200, { msgs, bad: w.bad, gone: goneList });
  }

  function pcWaitClosed(w) {
    if (!w.open) return;
    w.open = false;
    clearTimeout(w.timer);
    const now = Date.now();
    for (const id of w.rooms) {
      const r = rooms.get(id);
      if (r) {
        r.waits.delete(w);
        r.lastEnd = now;
      }
    }
    if (clients.get(w.client) === w) clients.delete(w.client);
  }

  function newestWait(r) {
    let last = null;
    for (const w of r.waits) last = w;
    return last;
  }

  // ----- phone waits -----

  function answerPhoneWait(s) {
    const w = s.waiter;
    if (!w || !w.open) return;
    w.open = false;
    clearTimeout(w.timer);
    s.waiter = null;
    s.exp = Math.max(s.exp, Date.now() + o.sessionMs);
    const msgs = s.queue;
    s.queue = [];
    sendJson(w.res, 200, { msgs });
  }

  // ----- removed rooms -----

  function markGone(id) {
    gone.delete(id);
    gone.set(id, Date.now());
    while (gone.size > o.goneMax) gone.delete(gone.keys().next().value);
    saveGoneSoon();
    const r = rooms.get(id);
    if (!r) return;
    rooms.delete(id);
    queued.delete(id);
    for (const w of [...r.waits]) answerPcWait(w);
    for (const s of [...r.sessions]) {
      const w = s.waiter;
      if (w && w.open) {
        w.open = false;
        clearTimeout(w.timer);
        fail(w.res, 410, 'gone');
      }
      s.waiter = null;
      sessions.delete(`${s.room} ${s.sid}`);
    }
  }

  function loadGone() {
    let text;
    try {
      text = fs.readFileSync(goneFile, 'utf8');
    } catch (e) {
      if (e.code !== 'ENOENT') logError(`drop-server: can't read ${goneFile}: ${e.message}`);
      return;
    }
    try {
      const j = JSON.parse(text);
      const now = Date.now();
      const entries = Object.entries((j && j.gone) || {})
        .filter(([id, at]) => idOk(id) && Number.isFinite(at) && now - at < o.goneMs)
        .sort((a, b) => a[1] - b[1]);
      for (const [id, at] of entries.slice(-o.goneMax)) gone.set(id, at);
    } catch (e) {
      logError(`drop-server: ${goneFile} is not valid JSON, starting with no removed rooms (${e.message})`);
    }
  }

  function writeGone() {
    clearTimeout(goneTimer);
    goneTimer = null;
    try {
      fs.mkdirSync(dataDir, { recursive: true });
      const tmp = `${goneFile}.${process.pid}.tmp`;
      fs.writeFileSync(tmp, JSON.stringify({ v: 1, gone: Object.fromEntries(gone) }));
      fs.renameSync(tmp, goneFile);
    } catch (e) {
      logError(`drop-server: can't write ${goneFile}: ${e.message}`);
    }
  }

  function saveGoneSoon() {
    if (!goneTimer) goneTimer = setTimeout(writeGone, 300);
  }

  // ----- relay passes -----

  function portOf(url) {
    const m = /^[a-z]+:([^?]*)/i.exec(url);
    const p = m && /:(\d+)$/.exec(m[1]);
    return p ? Number(p[1]) : null;
  }

  async function fetchTurn() {
    const r = await fetchImpl(`${TURN_API}${encodeURIComponent(turnKeyId)}/credentials/generate-ice-servers`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${turnToken}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ ttl: 3600 }),
      signal: AbortSignal.timeout(o.iceTimeoutMs),
    });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const j = await r.json();
    let list = j && j.iceServers;
    if (list && !Array.isArray(list)) list = [list];
    const out = [];
    for (const s of list || []) {
      if (!s || typeof s !== 'object' || typeof s.username !== 'string' || typeof s.credential !== 'string') continue;
      const urls = (Array.isArray(s.urls) ? s.urls : [s.urls]).filter(
        (u) => typeof u === 'string' && /^turns?:/i.test(u) && portOf(u) !== 53,
      );
      if (urls.length) out.push({ urls, username: s.username, credential: s.credential });
    }
    if (!out.length) throw new Error('no TURN entry in the answer');
    return out;
  }

  async function iceServers() {
    if (!turnKeyId || !turnToken) return [STUN];
    const now = Date.now();
    if (iceCache && now < iceCache.exp) return [STUN, ...iceCache.servers];
    if (now < iceFailUntil) return [STUN];
    if (!iceInflight) {
      iceInflight = fetchTurn()
        .then((servers) => {
          iceCache = { servers, exp: Date.now() + o.iceCacheMs };
          return servers;
        })
        .catch((e) => {
          iceFailUntil = Date.now() + o.iceRetryMs;
          logError(`drop-server: relay passes failed, answering STUN only (${(e && e.message) || e})`);
          return null;
        })
        .finally(() => {
          iceInflight = null;
        });
    }
    const servers = await iceInflight;
    return servers ? [STUN, ...servers] : [STUN];
  }

  // ----- routes -----

  function pcWait(req, res, b) {
    if (!roomListOk(b.rooms, o.roomsPerRequest) || !sidOk(b.client)) return badRequest(res);
    const now = Date.now();
    const ids = [];
    const badList = [];
    const goneList = [];
    const seen = new Set();
    for (const e of b.rooms) {
      if (seen.has(e.room)) continue;
      seen.add(e.room);
      const r = useRoom(e.room, e.proof, now);
      if (r === GONE) goneList.push(e.room);
      else if (r === BAD) badList.push(e.room);
      else if (r === FULL) return slowDown(res);
      else ids.push(e.room);
    }
    const older = clients.get(b.client);
    if (older) answerPcWait(older); // a newer wait from the same client ends the older one
    const w = { client: b.client, rooms: ids, res, timer: null, open: true, bad: badList, gone: goneList };
    for (const id of ids) rooms.get(id).waits.add(w);
    clients.set(b.client, w);
    res.on('close', () => pcWaitClosed(w));
    const kickedAt = kicks.get(b.client);
    if (kickedAt !== undefined) kicks.delete(b.client);
    const kicked = kickedAt !== undefined && now - kickedAt < o.kickMs;
    const pending = ids.some((id) => rooms.get(id).queue.length > 0);
    if (pending || kicked || badList.length || goneList.length || ids.length === 0) return answerPcWait(w);
    w.timer = setTimeout(() => answerPcWait(w), o.holdMs);
  }

  function pcKick(req, res, b) {
    if (!sidOk(b.client)) return badRequest(res);
    const w = clients.get(b.client);
    if (w) answerPcWait(w);
    else if (kicks.size < o.kicksMax) kicks.set(b.client, Date.now());
    sendJson(res, 200, { ok: true });
  }

  function pcSend(req, res, b) {
    if (!idOk(b.room) || !idOk(b.proof) || !msgOk(b.msg, o)) return badRequest(res);
    const now = Date.now();
    const r = useRoom(b.room, b.proof, now);
    if (typeof r === 'string') return roomError(res, r);
    const s = liveSession(b.room, b.msg.sid, now);
    if (!s) return fail(res, 404, 'no-session');
    if (s.queue.length >= o.queueMax) return slowDown(res);
    s.queue.push(b.msg);
    s.exp = Math.max(s.exp, now + o.sessionMs);
    if (s.waiter) answerPhoneWait(s);
    sendJson(res, 200, { ok: true });
  }

  function pcUnpair(req, res, b) {
    if (!idOk(b.room) || !idOk(b.proof)) return badRequest(res);
    if (gone.has(b.room)) return sendJson(res, 200, { ok: true }); // already removed
    const r = rooms.get(b.room);
    if (r && !crypto.timingSafeEqual(r.ph, proofHash(b.proof))) return fail(res, 403, 'bad');
    markGone(b.room);
    sendJson(res, 200, { ok: true });
  }

  function phoneStatus(req, res, b) {
    if (!roomListOk(b.rooms, o.roomsPerRequest)) return badRequest(res);
    const now = Date.now();
    const out = b.rooms.map((e) => {
      const r = useRoom(e.room, e.proof, now);
      if (r === GONE || r === BAD) return { room: e.room, state: r, seen: null };
      if (r === FULL) return { room: e.room, state: 'offline', seen: null };
      const on = online(r, now);
      return { room: e.room, state: on ? 'online' : 'offline', seen: on ? now : r.lastEnd };
    });
    sendJson(res, 200, { rooms: out });
  }

  function phoneSend(req, res, b) {
    if (!idOk(b.room) || !idOk(b.proof) || !msgOk(b.msg, o)) return badRequest(res);
    const now = Date.now();
    const r = useRoom(b.room, b.proof, now);
    if (typeof r === 'string') return roomError(res, r);
    if (!online(r, now)) return sendJson(res, 200, { ok: false, error: 'offline' });
    if (r.queue.length >= o.queueMax) return slowDown(res);
    let s = liveSession(b.room, b.msg.sid, now);
    if (!s) {
      if (sessions.size >= o.sessionsMax || r.sessions.size >= o.sessionsPerRoom) return slowDown(res);
      s = { room: b.room, sid: b.msg.sid, queue: [], waiter: null, exp: 0 };
      sessions.set(`${b.room} ${b.msg.sid}`, s);
      r.sessions.add(s);
    }
    s.exp = Math.max(s.exp, now + o.sessionMs);
    r.queue.push({ msg: b.msg, exp: now + o.sessionMs });
    queued.add(b.room);
    const w = newestWait(r);
    if (w) answerPcWait(w);
    sendJson(res, 200, { ok: true });
  }

  function phoneWait(req, res, b) {
    if (!idOk(b.room) || !idOk(b.proof) || !sidOk(b.sid)) return badRequest(res);
    const now = Date.now();
    const r = useRoom(b.room, b.proof, now);
    if (typeof r === 'string') return roomError(res, r);
    const s = liveSession(b.room, b.sid, now);
    if (!s) return fail(res, 404, 'no-session');
    if (s.waiter) answerPhoneWait(s); // a newer wait for the session ends the older one
    s.exp = Math.max(s.exp, now + o.sessionMs);
    const w = { res, timer: null, open: true };
    s.waiter = w;
    res.on('close', () => {
      if (!w.open) return;
      w.open = false;
      clearTimeout(w.timer);
      if (s.waiter === w) s.waiter = null;
      s.exp = Math.max(s.exp, Date.now() + o.sessionMs);
    });
    if (s.queue.length) return answerPhoneWait(s);
    w.timer = setTimeout(() => answerPhoneWait(s), o.holdMs);
  }

  // May this request get a relay pass? Counts it when yes.
  function turnAllowed(req, r, now) {
    if (!turnKeyId || !turnToken || !online(r, now)) return false;
    if (now - turnHour.start >= 3600_000) turnHour = { start: now, n: 0 };
    if (turnHour.n >= o.turnPerHour) {
      if (now - turnCapLogged >= 3600_000) {
        turnCapLogged = now;
        logError(`drop-server: ${o.turnPerHour} relay passes this hour, answering STUN only until the hour is over`);
      }
      return false;
    }
    const key = clientAddress(req);
    let u = turnUse.get(key);
    if (!u || now - u.start >= 3600_000) {
      if (!u && turnUse.size >= o.bucketsMax) turnUse.delete(turnUse.keys().next().value);
      u = { start: now, n: 0 };
      turnUse.set(key, u);
    }
    if (u.n >= o.turnPerAddressHour) return false;
    u.n += 1;
    turnHour.n += 1;
    return true;
  }

  async function ice(req, res, b) {
    if (!idOk(b.room) || !idOk(b.proof)) return badRequest(res);
    const now = Date.now();
    const r = useRoom(b.room, b.proof, now);
    if (typeof r === 'string') return roomError(res, r);
    const list = turnAllowed(req, r, now) ? await iceServers() : [STUN];
    sendJson(res, 200, { iceServers: list });
  }

  const ROUTES = new Map([
    ['/api/pc/wait', pcWait],
    ['/api/pc/kick', pcKick],
    ['/api/pc/send', pcSend],
    ['/api/pc/unpair', pcUnpair],
    ['/api/phone/status', phoneStatus],
    ['/api/phone/send', phoneSend],
    ['/api/phone/wait', phoneWait],
    ['/api/ice', ice],
  ]);

  // ----- requests -----

  function clientAddress(req) {
    if (trustProxy) {
      const h = req.headers['cf-connecting-ip'];
      if (typeof h === 'string' && /^[0-9A-Fa-f:.]{2,45}$/.test(h.trim())) return addressKey(h);
    }
    return addressKey(req.socket.remoteAddress);
  }

  // Seconds to wait when over the limit, else 0.
  function rateCheck(req) {
    const key = clientAddress(req);
    const now = Date.now();
    let b = buckets.get(key);
    if (!b || now - b.start >= o.rateWindowMs) {
      if (!b && buckets.size >= o.bucketsMax) buckets.delete(buckets.keys().next().value);
      b = { start: now, n: 0 };
      buckets.set(key, b);
    }
    b.n += 1;
    if (b.n <= o.rateLimit) return 0;
    return Math.max(1, Math.ceil((b.start + o.rateWindowMs - now) / 1000));
  }

  function refuseBody(req, res) {
    fail(res, 413, 'too-large', { Connection: 'close' });
    req.on('data', () => {}); // drain, then let the connection close
    setTimeout(() => req.destroy(), 5_000).unref();
  }

  function readBody(req, res) {
    return new Promise((resolve) => {
      const declared = Number(req.headers['content-length']);
      if (declared > o.bodyMax) {
        refuseBody(req, res);
        return resolve(undefined);
      }
      const chunks = [];
      let size = 0;
      let done = false;
      req.on('data', (c) => {
        if (done) return;
        size += c.length;
        if (size > o.bodyMax) {
          done = true;
          refuseBody(req, res);
          resolve(undefined);
          return;
        }
        chunks.push(c);
      });
      req.on('end', () => {
        if (done) return;
        done = true;
        let v;
        try {
          v = JSON.parse(Buffer.concat(chunks).toString('utf8'));
        } catch {
          v = null;
        }
        if (!v || typeof v !== 'object' || Array.isArray(v)) {
          badRequest(res);
          return resolve(undefined);
        }
        resolve(v);
      });
      req.on('error', () => {
        if (done) return;
        done = true;
        resolve(undefined);
      });
    });
  }

  async function handleApi(req, res, pathname) {
    res.setHeader('Cache-Control', 'no-store');
    const wait = rateCheck(req);
    if (wait) return fail(res, 429, 'slow-down', { 'Retry-After': String(wait) });
    if (pathname === '/api/health') {
      if (req.method !== 'GET' && req.method !== 'HEAD') return fail(res, 405, 'bad-request', { Allow: 'GET, HEAD' });
      return sendJson(res, 200, { ok: true });
    }
    const route = ROUTES.get(pathname);
    if (!route) return fail(res, 404, 'not-found');
    if (req.method !== 'POST') return fail(res, 405, 'bad-request', { Allow: 'POST' });
    const body = await readBody(req, res);
    if (body === undefined) return;
    return route(req, res, body);
  }

  function handleStatic(req, res, pathname) {
    if (pathname === '/share' && req.method === 'POST') {
      // Only reached when no service worker caught the share.
      res.writeHead(303, { Location: '/?shared=lost', 'Cache-Control': 'no-store', 'Content-Length': 0, Connection: 'close' });
      res.end();
      req.on('data', () => {});
      setTimeout(() => req.destroy(), 10_000).unref();
      return;
    }
    const f = files.get(pathname);
    if (!f) return sendText(res, 404, 'Not found');
    if (req.method !== 'GET' && req.method !== 'HEAD') return sendText(res, 405, 'Method not allowed', { Allow: 'GET, HEAD' });
    const headers = { 'Content-Type': f.type, 'Cache-Control': f.cache, ETag: f.etag };
    if (req.headers['if-none-match'] === f.etag) {
      res.writeHead(304, headers);
      return res.end();
    }
    res.writeHead(200, { ...headers, 'Content-Length': f.body.length });
    res.end(req.method === 'HEAD' ? undefined : f.body);
  }

  async function handle(req, res) {
    for (const [k, v] of SECURITY_HEADERS) res.setHeader(k, v);
    let pathname;
    try {
      pathname = new URL(req.url, 'http://drop.invalid').pathname;
    } catch {
      return sendText(res, 400, 'Bad request');
    }
    if (pathname === '/api' || pathname.startsWith('/api/')) return handleApi(req, res, pathname);
    return handleStatic(req, res, pathname);
  }

  function loadStatic(dir) {
    const out = new Map();
    let names = [];
    try {
      names = fs.readdirSync(dir);
    } catch (e) {
      logError(`drop-server: can't read ${dir}: ${e.message}`);
    }
    for (const name of names) {
      if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name)) continue;
      const ext = path.extname(name).toLowerCase();
      if (!TYPES[ext]) continue;
      const full = path.join(dir, name);
      let body;
      try {
        if (!fs.statSync(full).isFile()) continue;
        body = fs.readFileSync(full);
      } catch {
        continue;
      }
      const etag = `"${crypto.createHash('sha256').update(body).digest('base64url').slice(0, 22)}"`;
      const fresh = ext === '.html' || ext === '.webmanifest' || name === 'sw.js';
      out.set(`/${name}`, { body, type: TYPES[ext], etag, cache: fresh ? 'no-cache' : 'public, max-age=300' });
    }
    const index = out.get('/index.html');
    if (index) for (const p of ['/', '/p', '/share']) out.set(p, index);
    return out;
  }

  // ----- housekeeping -----

  function sweep() {
    const now = Date.now();
    for (const s of sessions.values()) if (!s.waiter && s.exp <= now) dropSession(s);
    for (const id of queued) {
      const r = rooms.get(id);
      if (!r) {
        queued.delete(id);
        continue;
      }
      r.queue = r.queue.filter((q) => q.exp > now);
      if (!r.queue.length) queued.delete(id);
    }
    for (const [c, at] of kicks) if (now - at >= o.kickMs) kicks.delete(c);
    for (const [k, b] of buckets) if (now - b.start >= o.rateWindowMs) buckets.delete(k);
    for (const [k, u] of turnUse) if (now - u.start >= 3600_000) turnUse.delete(k);
    if (now - lastRoomSweep >= 60_000) {
      lastRoomSweep = now;
      for (const [id, r] of rooms) {
        if (now - r.used < o.roomIdleMs) break; // least recently used first
        if (idle(r)) rooms.delete(id);
      }
      let dropped = false;
      for (const [id, at] of gone) {
        if (now - at < o.goneMs) break;
        gone.delete(id);
        dropped = true;
      }
      if (dropped) saveGoneSoon();
    }
  }

  const sweeper = setInterval(sweep, o.sweepMs);
  sweeper.unref();

  const server = http.createServer((req, res) => {
    handle(req, res).catch((e) => {
      logError(`drop-server: request failed: ${(e && e.message) || e}`);
      if (!res.headersSent) fail(res, 500, 'server-error');
    });
  });
  server.requestTimeout = 60_000;
  server.headersTimeout = 20_000;

  return {
    server,
    options: o,
    listen(port = 0, host = '127.0.0.1') {
      return new Promise((resolve, reject) => {
        server.once('error', reject);
        server.listen(port, host, () => {
          server.off('error', reject);
          resolve(server.address());
        });
      });
    },
    close() {
      clearInterval(sweeper);
      if (goneTimer) writeGone();
      for (const w of [...clients.values()]) answerPcWait(w);
      for (const s of sessions.values()) answerPhoneWait(s);
      return new Promise((resolve) => {
        server.close(() => resolve());
        server.closeIdleConnections?.();
        setTimeout(() => server.closeAllConnections?.(), 200).unref();
      });
    },
    // For tests: counts only, never contents.
    stats: () => ({ rooms: rooms.size, gone: gone.size, sessions: sessions.size, waits: clients.size, kicks: kicks.size }),
    flushGone: writeGone,
  };
}

// ---------- command line ----------

const USAGE = `Usage: node server/drop-server.mjs --port 8797 [--host 127.0.0.1] [--data <dir>] [--trust-proxy]
Relay passes: set TURN_KEY_ID and TURN_KEY_API_TOKEN (a Cloudflare Realtime TURN key).
Test knobs: --hold-ms <n> --online-ms <n> --session-ms <n> --rate <requests per minute>`;

function parseArgs(argv) {
  const out = { port: 8797, host: '127.0.0.1', data: null, trustProxy: false };
  const num = (v, name) => {
    const n = Number(v);
    if (!Number.isInteger(n) || n < 0) throw new Error(`${name} needs a whole number`);
    return n;
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const val = () => {
      if (i + 1 >= argv.length) throw new Error(`${a} needs a value`);
      return argv[++i];
    };
    if (a === '--port') out.port = num(val(), a);
    else if (a === '--host') out.host = val();
    else if (a === '--data') out.data = path.resolve(val());
    else if (a === '--trust-proxy') out.trustProxy = true;
    else if (a === '--hold-ms') out.holdMs = num(val(), a);
    else if (a === '--online-ms') out.onlineMs = num(val(), a);
    else if (a === '--session-ms') out.sessionMs = num(val(), a);
    else if (a === '--rate') out.rateLimit = num(val(), a);
    else if (a === '--help' || a === '-h') out.help = true;
    else throw new Error(`unknown option ${a}`);
  }
  return out;
}

async function main() {
  let args;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (e) {
    console.error(`drop-server: ${e.message}\n${USAGE}`);
    process.exit(2);
  }
  if (args.help) {
    console.log(USAGE);
    return;
  }
  const turnKeyId = process.env.TURN_KEY_ID || '';
  const turnToken = process.env.TURN_KEY_API_TOKEN || '';
  const drop = createDropServer({
    dataDir: args.data || undefined,
    trustProxy: args.trustProxy,
    turnKeyId,
    turnToken,
    holdMs: args.holdMs,
    onlineMs: args.onlineMs,
    sessionMs: args.sessionMs,
    rateLimit: args.rateLimit,
  });
  let addr;
  try {
    addr = await drop.listen(args.port, args.host);
  } catch (e) {
    console.error(`drop-server: can't listen on ${args.host}:${args.port}: ${e.message}`);
    process.exit(1);
  }
  const host = addr.family === 'IPv6' ? `[${addr.address}]` : addr.address;
  console.log(`drop-server listening on http://${host}:${addr.port} (relay ${turnKeyId && turnToken ? 'on' : 'off'})`);
  let stopping = false;
  const stop = () => {
    if (stopping) return;
    stopping = true;
    drop.close().then(() => process.exit(0));
    setTimeout(() => process.exit(0), 2_000).unref();
  };
  process.on('SIGINT', stop);
  process.on('SIGTERM', stop);
}

// Run when started as a program. Compare real paths: deployments start it
// through a symlink (current/ -> releases/...), and Node resolves the module's
// own URL to the real path.
function startedAsProgram() {
  if (!process.argv[1]) return false;
  try {
    return fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (startedAsProgram()) main();

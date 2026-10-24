// Every API route of docs/paired-drop.md against a real server on a free port.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { start, newPair, newSid, sign, rp, sleep, tempDir } from './helpers.mjs';
import { DEFAULTS, addressKey } from '../drop-server.mjs';

const pcWait = (s, client, pairs, opts) => s.timed('/api/pc/wait', { rooms: pairs.map(rp), client }, opts);
const status = (s, pairs) => s.call('/api/phone/status', { rooms: pairs.map(rp) });
const phoneMsg = (k, t = 'offer', extra = {}) => sign(k, { t, sid: newSid(), name: 'Pixel 10', sdp: t === 'offer' ? 'v=0\r\n' : '', ...extra });

test('defaults follow the contract', () => {
  assert.equal(DEFAULTS.holdMs, 25_000);
  assert.equal(DEFAULTS.onlineMs, 35_000);
  assert.equal(DEFAULTS.sessionMs, 120_000);
  assert.equal(DEFAULTS.rateLimit, 120);
  assert.equal(DEFAULTS.rateWindowMs, 60_000);
  assert.equal(DEFAULTS.bodyMax, 64 * 1024);
  assert.equal(DEFAULTS.sdpMax, 32 * 1024);
  assert.equal(DEFAULTS.nameMax, 60);
  assert.equal(DEFAULTS.roomsPerRequest, 32);
});

test('GET /api/health', async (t) => {
  const s = await start();
  t.after(s.close);
  const r = await s.call('/api/health', undefined, { method: 'GET' });
  assert.equal(r.status, 200);
  assert.deepEqual(r.json, { ok: true });
  assert.equal(r.headers.get('cache-control'), 'no-store');
  assert.match(r.headers.get('content-type'), /^application\/json/);
  const p = await s.call('/api/health', {});
  assert.equal(p.status, 405);
  assert.equal(p.json.error, 'bad-request');
});

test('unknown API routes and wrong methods', async (t) => {
  const s = await start();
  t.after(s.close);
  assert.equal((await s.call('/api/nope', {})).status, 404);
  assert.equal((await s.call('/api/pc/wait', undefined, { method: 'GET' })).status, 405);
  assert.equal((await s.call('/api/phone/status', undefined, { method: 'PUT' })).status, 405);
});

test('a full pair round: phone -> PC -> phone, each message handed out once', async (t) => {
  const s = await start({ holdMs: 1000 });
  t.after(s.close);
  const k = newPair();
  const client = newSid();
  // The PC waits; the phone sends pair; the wait wakes at once.
  const waiting = pcWait(s, client, [k]);
  await sleep(100);
  const pair = phoneMsg(k, 'pair');
  const sent = await s.call('/api/phone/send', { ...rp(k), msg: pair });
  assert.deepEqual(sent.json, { ok: true });
  const w = await waiting;
  assert.equal(w.status, 200);
  assert.ok(w.ms < 800, `wait woke after ${w.ms} ms`);
  assert.deepEqual(w.json, { msgs: [{ room: k.room, msg: pair }], bad: [], gone: [] });
  // Handed out once: the next wait gets nothing.
  const again = await pcWait(s, client, [k], {});
  assert.deepEqual(again.json.msgs, []);
  // The PC answers; the phone's wait gets it.
  const welcome = sign(k, { t: 'welcome', sid: pair.sid, name: 'RTX-5090-Astral' });
  assert.deepEqual((await s.call('/api/pc/send', { ...rp(k), msg: welcome })).json, { ok: true });
  const pw = await s.timed('/api/phone/wait', { ...rp(k), sid: pair.sid });
  assert.ok(pw.ms < 500);
  assert.deepEqual(pw.json, { msgs: [welcome] });
  const pw2 = await s.timed('/api/phone/wait', { ...rp(k), sid: pair.sid });
  assert.deepEqual(pw2.json, { msgs: [] });
  assert.ok(pw2.ms >= 900, `empty wait held ${pw2.ms} ms`);
});

test('phone wait wakes as soon as the PC answers', async (t) => {
  const s = await start({ holdMs: 5000 });
  t.after(s.close);
  const k = newPair();
  const client = newSid();
  const pcw = pcWait(s, client, [k]);
  await sleep(50);
  const offer = phoneMsg(k, 'offer');
  await s.call('/api/phone/send', { ...rp(k), msg: offer });
  await pcw;
  const phoneWaiting = s.timed('/api/phone/wait', { ...rp(k), sid: offer.sid });
  await sleep(300);
  const answer = sign(k, { t: 'answer', sid: offer.sid, name: 'RTX-5090-Astral', sdp: 'v=0\r\n' });
  await s.call('/api/pc/send', { ...rp(k), msg: answer });
  const r = await phoneWaiting;
  assert.deepEqual(r.json, { msgs: [answer] });
  assert.ok(r.ms >= 250 && r.ms < 1500, `${r.ms} ms`);
});

test('long polls hold until the cap and answer empty', async (t) => {
  const s = await start({ holdMs: 400 });
  t.after(s.close);
  const k = newPair();
  const r = await pcWait(s, newSid(), [k]);
  assert.deepEqual(r.json, { msgs: [], bad: [], gone: [] });
  assert.ok(r.ms >= 380 && r.ms < 1500, `${r.ms} ms`);
});

test('the PC is online while waiting and for the online window after', async (t) => {
  const s = await start({ holdMs: 300, onlineMs: 600 });
  t.after(s.close);
  const k = newPair();
  let st = await status(s, [k]);
  assert.deepEqual(st.json, { rooms: [{ room: k.room, state: 'offline', seen: null }] });
  const waiting = pcWait(s, newSid(), [k]);
  await sleep(100);
  st = await status(s, [k]);
  assert.equal(st.json.rooms[0].state, 'online');
  assert.ok(Math.abs(st.json.rooms[0].seen - Date.now()) < 1000);
  await waiting; // ended
  const ended = Date.now();
  await sleep(200);
  st = await status(s, [k]);
  assert.equal(st.json.rooms[0].state, 'online', 'still inside the window');
  await sleep(600);
  st = await status(s, [k]);
  assert.equal(st.json.rooms[0].state, 'offline');
  assert.ok(Math.abs(st.json.rooms[0].seen - ended) < 300, 'seen = when the last wait ended');
});

test('a wait the PC drops counts as ended', async (t) => {
  const s = await start({ holdMs: 5000, onlineMs: 300 });
  t.after(s.close);
  const k = newPair();
  const ac = new AbortController();
  const p = pcWait(s, newSid(), [k], { signal: ac.signal }).catch(() => 'aborted');
  await sleep(100);
  assert.equal((await status(s, [k])).json.rooms[0].state, 'online');
  ac.abort();
  assert.equal(await p, 'aborted');
  await sleep(500);
  assert.equal((await status(s, [k])).json.rooms[0].state, 'offline');
  assert.equal(s.drop.stats().waits, 0);
});

test('phone/send to an offline PC answers offline and queues nothing', async (t) => {
  const s = await start({ holdMs: 300 });
  t.after(s.close);
  const k = newPair();
  const m = phoneMsg(k, 'pair');
  const r = await s.call('/api/phone/send', { ...rp(k), msg: m });
  assert.equal(r.status, 200);
  assert.deepEqual(r.json, { ok: false, error: 'offline' });
  const w = await pcWait(s, newSid(), [k]);
  assert.deepEqual(w.json.msgs, []);
  // and no session was opened
  assert.equal((await s.call('/api/phone/wait', { ...rp(k), sid: m.sid })).status, 404);
});

test('messages sent inside the online window wait for the next PC wait', async (t) => {
  const s = await start({ holdMs: 200, onlineMs: 5000 });
  t.after(s.close);
  const k = newPair();
  const client = newSid();
  await pcWait(s, client, [k]); // ends after 200 ms, PC stays online
  const m = phoneMsg(k, 'offer');
  assert.deepEqual((await s.call('/api/phone/send', { ...rp(k), msg: m })).json, { ok: true });
  const w = await pcWait(s, client, [k]);
  assert.ok(w.ms < 150, 'answers at once when messages are waiting');
  assert.deepEqual(w.json.msgs, [{ room: k.room, msg: m }]);
});

test('one wait covers many rooms; bad and gone rooms come back at once', async (t) => {
  const s = await start({ holdMs: 2000 });
  t.after(s.close);
  const a = newPair();
  const b = newPair();
  const c = newPair();
  const client = newSid();
  // c is removed, b is registered with a different proof
  await s.call('/api/pc/unpair', rp(c));
  await status(s, [b]);
  const badB = { room: b.room, proof: a.proof };
  const r = await s.timed('/api/pc/wait', { rooms: [rp(a), badB, rp(c)], client });
  assert.ok(r.ms < 500);
  assert.deepEqual(r.json, { msgs: [], bad: [b.room], gone: [c.room] });
  // a counts as online after that wait
  assert.equal((await status(s, [a])).json.rooms[0].state, 'online');
  // a wait over a and b(right proof) delivers from both rooms
  const both = pcWait(s, client, [a, b]);
  await sleep(50);
  const mb = phoneMsg(b, 'pair');
  await s.call('/api/phone/send', { ...rp(b), msg: mb });
  assert.deepEqual((await both).json, { msgs: [{ room: b.room, msg: mb }], bad: [], gone: [] });
});

test('a newer wait from the same client ends the older one; other clients are untouched', async (t) => {
  const s = await start({ holdMs: 1500 });
  t.after(s.close);
  const a = newPair();
  const b = newPair();
  const client = newSid();
  const other = newSid();
  const first = pcWait(s, client, [a]);
  const theirs = pcWait(s, other, [b]);
  await sleep(100);
  const second = pcWait(s, client, [a, b]);
  const r1 = await first;
  assert.ok(r1.ms < 500, `older wait ended after ${r1.ms} ms`);
  assert.deepEqual(r1.json, { msgs: [], bad: [], gone: [] });
  const r2 = await second;
  assert.ok(r2.ms >= 1400, 'the newer wait is held');
  const r3 = await theirs;
  assert.ok(r3.ms >= 1400, 'another client is not ended');
});

test('a newer wait carries messages queued before it, the older gets what it had', async (t) => {
  const s = await start({ holdMs: 1000 });
  t.after(s.close);
  const a = newPair();
  const client = newSid();
  const first = pcWait(s, client, [a]);
  await sleep(50);
  const m = phoneMsg(a, 'pair');
  await s.call('/api/phone/send', { ...rp(a), msg: m });
  assert.deepEqual((await first).json.msgs, [{ room: a.room, msg: m }]);
});

test('pc/kick ends that client\'s open wait at once, and only that one', async (t) => {
  const s = await start({ holdMs: 2000 });
  t.after(s.close);
  const a = newPair();
  const client = newSid();
  const other = newSid();
  const mine = pcWait(s, client, [a]);
  const theirs = pcWait(s, other, [newPair()]);
  await sleep(100);
  const k = await s.call('/api/pc/kick', { client });
  assert.deepEqual(k.json, { ok: true });
  const r = await mine;
  assert.ok(r.ms < 600, `kicked wait answered after ${r.ms} ms`);
  assert.deepEqual(r.json, { msgs: [], bad: [], gone: [] });
  assert.ok((await theirs).ms >= 1900, 'the other client kept waiting');
  // A kick with nothing open is still ok
  assert.deepEqual((await s.call('/api/pc/kick', { client: newSid() })).json, { ok: true });
  assert.equal((await s.call('/api/pc/kick', { client: 'short' })).status, 400);
  assert.equal((await s.call('/api/pc/kick', {})).status, 400);
});

test('a kick that arrives before its wait ends that wait as soon as it comes', async (t) => {
  const s = await start({ holdMs: 2000 });
  t.after(s.close);
  const client = newSid();
  await s.call('/api/pc/kick', { client });
  const r = await pcWait(s, client, [newPair()]);
  assert.ok(r.ms < 500, `${r.ms} ms`);
  const r2 = await pcWait(s, client, [newPair()]);
  assert.ok(r2.ms >= 1900, 'only the next wait');
});

test('pc/send needs an open phone session in that room', async (t) => {
  const s = await start({ holdMs: 200, sessionMs: 600, onlineMs: 5000 });
  t.after(s.close);
  const k = newPair();
  const other = newPair();
  await pcWait(s, newSid(), [k, other]);
  const offer = phoneMsg(k, 'offer');
  // no session yet
  const early = await s.call('/api/pc/send', { ...rp(k), msg: sign(k, { t: 'answer', sid: offer.sid }) });
  assert.equal(early.status, 404);
  assert.deepEqual(early.json, { ok: false, error: 'no-session' });
  await s.call('/api/phone/send', { ...rp(k), msg: offer });
  // another room can't reach that session
  const cross = await s.call('/api/pc/send', { ...rp(other), msg: sign(other, { t: 'answer', sid: offer.sid }) });
  assert.equal(cross.status, 404);
  // the right room can, and the message waits for the phone
  assert.deepEqual((await s.call('/api/pc/send', { ...rp(k), msg: sign(k, { t: 'busy', sid: offer.sid }) })).json, { ok: true });
  assert.equal((await s.call('/api/phone/wait', { ...rp(k), sid: offer.sid })).json.msgs[0].t, 'busy');
  // phone/wait with a session nobody opened
  const none = await s.call('/api/phone/wait', { ...rp(k), sid: newSid() });
  assert.equal(none.status, 404);
  assert.equal(none.json.error, 'no-session');
  // the session lives sessionMs after its last use, then goes
  await sleep(900);
  assert.equal((await s.call('/api/phone/wait', { ...rp(k), sid: offer.sid })).status, 404);
  assert.equal((await s.call('/api/pc/send', { ...rp(k), msg: sign(k, { t: 'answer', sid: offer.sid }) })).status, 404);
});

test('a newer phone wait for the same session ends the older one', async (t) => {
  const s = await start({ holdMs: 1500, onlineMs: 10_000 });
  t.after(s.close);
  const k = newPair();
  await s.call('/api/pc/kick', { client: newSid() }); // harmless
  const client = newSid();
  pcWait(s, client, [k]);
  await sleep(50);
  const m = phoneMsg(k, 'pair');
  await s.call('/api/phone/send', { ...rp(k), msg: m });
  const first = s.timed('/api/phone/wait', { ...rp(k), sid: m.sid });
  await sleep(100);
  const second = s.timed('/api/phone/wait', { ...rp(k), sid: m.sid });
  const r1 = await first;
  assert.ok(r1.ms < 600);
  assert.deepEqual(r1.json, { msgs: [] });
  await sleep(50);
  const w = sign(k, { t: 'welcome', sid: m.sid, name: 'RTX' });
  await s.call('/api/pc/send', { ...rp(k), msg: w });
  assert.deepEqual((await second).json, { msgs: [w] });
  await s.call('/api/pc/kick', { client });
});

test('unpair makes the room gone everywhere, and gone survives a restart', async (t) => {
  const dataDir = tempDir();
  let s = await start({ holdMs: 2000, dataDir });
  const k = newPair();
  const client = newSid();
  const waiting = pcWait(s, client, [k]);
  await sleep(50);
  const m = phoneMsg(k, 'pair');
  await s.call('/api/phone/send', { ...rp(k), msg: m });
  await waiting;
  const phoneWaiting = s.call('/api/phone/wait', { ...rp(k), sid: m.sid });
  const pcWaiting = pcWait(s, client, [k]);
  await sleep(100);
  // wrong proof can't remove it
  const other = newPair();
  assert.equal((await s.call('/api/pc/unpair', { room: k.room, proof: other.proof })).status, 403);
  assert.deepEqual((await s.call('/api/pc/unpair', rp(k))).json, { ok: true });
  // open waits end: the phone's with 410, the PC's listing the room as gone
  const pw = await phoneWaiting;
  assert.equal(pw.status, 410);
  assert.deepEqual(pw.json, { ok: false, error: 'gone' });
  const w = await pcWaiting;
  assert.ok(w.ms < 500);
  assert.deepEqual(w.json, { msgs: [], bad: [], gone: [k.room] });
  // every route sees it
  assert.deepEqual((await status(s, [k])).json, { rooms: [{ room: k.room, state: 'gone', seen: null }] });
  assert.equal((await s.call('/api/phone/send', { ...rp(k), msg: phoneMsg(k) })).status, 410);
  assert.equal((await s.call('/api/phone/wait', { ...rp(k), sid: m.sid })).status, 410);
  assert.equal((await s.call('/api/pc/send', { ...rp(k), msg: sign(k, { t: 'answer', sid: m.sid }) })).status, 410);
  assert.equal((await s.call('/api/ice', rp(k))).status, 410);
  assert.deepEqual((await pcWait(s, client, [k])).json.gone, [k.room]);
  assert.deepEqual((await s.call('/api/pc/unpair', rp(k))).json, { ok: true }, 'removing twice is fine');
  await s.close();
  const saved = JSON.parse(fs.readFileSync(path.join(dataDir, 'gone.json'), 'utf8'));
  assert.deepEqual(Object.keys(saved.gone), [k.room]);
  assert.ok(!JSON.stringify(saved).includes(k.proof), 'no proofs on disk');
  // restart on the same data
  s = await start({ dataDir });
  t.after(s.close);
  assert.equal((await status(s, [k])).json.rooms[0].state, 'gone');
});

test('gone entries older than 30 days are dropped at start', async (t) => {
  const dataDir = tempDir();
  const old = newPair();
  const recent = newPair();
  const day = 24 * 3600_000;
  fs.writeFileSync(path.join(dataDir, 'gone.json'), JSON.stringify({ v: 1, gone: { [old.room]: Date.now() - 31 * day, [recent.room]: Date.now() - 29 * day, 'not-a-room': Date.now() } }));
  const s = await start({ dataDir });
  t.after(s.close);
  const st = await status(s, [old, recent]);
  assert.deepEqual(st.json.rooms.map((r) => r.state), ['offline', 'gone']);
  assert.equal(s.drop.stats().gone, 1);
});

test('a broken gone.json is reported and ignored', async (t) => {
  const dataDir = tempDir();
  fs.writeFileSync(path.join(dataDir, 'gone.json'), '{oops');
  const s = await start({ dataDir });
  t.after(s.close);
  assert.equal(s.logs.length, 1);
  assert.match(s.logs[0], /not valid JSON/);
});

test('first proof wins; a different proof is bad', async (t) => {
  const s = await start({ holdMs: 200 });
  t.after(s.close);
  const k = newPair();
  const wrong = { room: k.room, proof: newPair().proof };
  await status(s, [k]); // registers
  const st = await s.call('/api/phone/status', { rooms: [wrong, rp(k)] });
  assert.deepEqual(st.json.rooms.map((r) => r.state), ['bad', 'offline']);
  assert.equal(st.json.rooms[0].seen, null);
  for (const route of ['/api/phone/send', '/api/pc/send']) {
    const r = await s.call(route, { ...wrong, msg: phoneMsg(k) });
    assert.equal(r.status, 403, route);
    assert.deepEqual(r.json, { ok: false, error: 'bad' });
  }
  assert.equal((await s.call('/api/phone/wait', { ...wrong, sid: newSid() })).status, 403);
  assert.equal((await s.call('/api/ice', wrong)).status, 403);
  assert.deepEqual((await s.timed('/api/pc/wait', { rooms: [wrong], client: newSid() })).json, { msgs: [], bad: [k.room], gone: [] });
});

test('bodies and messages are validated', async (t) => {
  const s = await start();
  t.after(s.close);
  const k = newPair();
  const good = phoneMsg(k);
  const bad400 = async (route, body, why) => {
    const r = await s.call(route, body);
    assert.equal(r.status, 400, `${route}: ${why}`);
    assert.deepEqual(r.json, { ok: false, error: 'bad-request' });
  };
  await bad400('/api/phone/status', { rooms: [] }, 'no rooms');
  await bad400('/api/phone/status', { rooms: Array.from({ length: 33 }, () => rp(newPair())) }, '33 rooms');
  await bad400('/api/phone/status', { rooms: [{ room: k.room }] }, 'no proof');
  await bad400('/api/phone/status', { rooms: [{ room: `${k.room}A`, proof: k.proof }] }, 'long room');
  await bad400('/api/phone/status', { rooms: [{ room: k.room.replace(/.$/, 'B'), proof: k.proof }] }, 'non-canonical room');
  await bad400('/api/phone/status', { rooms: [{ room: `${k.room.slice(0, 42)}+`, proof: k.proof }] }, 'not url-safe');
  await bad400('/api/phone/status', { rooms: 'x' }, 'rooms not a list');
  await bad400('/api/pc/wait', { rooms: [rp(k)] }, 'no client');
  await bad400('/api/pc/wait', { rooms: [rp(k)], client: 'x'.repeat(22) }, 'client not b64u of 16 bytes');
  await bad400('/api/phone/wait', { ...rp(k), sid: 'AAECAwQFBgcICQoLDA0OD' }, 'short sid');
  await bad400('/api/ice', { room: k.room }, 'no proof');
  const msgCases = {
    'missing mac': (({ mac, ...m }) => m)(good),
    'extra field': { ...good, room: k.room },
    'ts not integer': { ...good, ts: 1.5 },
    'ts string': { ...good, ts: String(good.ts) },
    'ts negative': { ...good, ts: -1 },
    'name too long': { ...good, name: 'n'.repeat(61) },
    'name two lines': { ...good, name: 'a\nb' },
    'name not string': { ...good, name: null },
    'sdp too long': { ...good, sdp: 'v'.repeat(32 * 1024 + 1) },
    'sdp not string': { ...good, sdp: 5 },
    't uppercase': { ...good, t: 'Offer' },
    't empty': { ...good, t: '' },
    'sid bad': { ...good, sid: 'nope' },
    'mac bad': { ...good, mac: 'x' },
    'msg is a list': [good],
    'msg missing': undefined,
  };
  for (const [why, msg] of Object.entries(msgCases)) {
    await bad400('/api/phone/send', { ...rp(k), msg }, why);
    await bad400('/api/pc/send', { ...rp(k), msg }, why);
  }
  // 60 characters of emoji are fine (characters, not bytes)
  const emoji = sign(k, { t: 'pair', sid: newSid(), name: '📱'.repeat(60) });
  assert.equal((await s.call('/api/phone/send', { ...rp(k), msg: emoji })).status, 200);
  // exactly 32 KB of SDP is fine
  const big = sign(k, { t: 'offer', sid: newSid(), sdp: 'v'.repeat(32 * 1024) });
  assert.equal((await s.call('/api/phone/send', { ...rp(k), msg: big })).status, 200);
  // not JSON, or not an object
  for (const raw of ['not json', '[1,2]', '"text"', 'null', '']) {
    const r = await s.call('/api/phone/status', undefined, { raw, headers: { 'Content-Type': 'application/json' } });
    assert.equal(r.status, 400, JSON.stringify(raw));
  }
});

test('bodies over 64 KB are too large', async (t) => {
  const s = await start();
  t.after(s.close);
  const k = newPair();
  const body = JSON.stringify({ rooms: [rp(k)], pad: 'x'.repeat(64 * 1024) });
  const r = await s.call('/api/phone/status', undefined, { raw: body, headers: { 'Content-Type': 'application/json' } });
  assert.equal(r.status, 413);
  assert.deepEqual(r.json, { ok: false, error: 'too-large' });
  // streamed without a length
  const stream = new ReadableStream({
    start(c) {
      for (let i = 0; i < 70; i++) c.enqueue(new TextEncoder().encode('x'.repeat(1024)));
      c.close();
    },
  });
  const r2 = await fetch(`${s.base}/api/phone/status`, { method: 'POST', body: stream, duplex: 'half' });
  assert.equal(r2.status, 413);
  // just under the limit is read
  const fits = JSON.stringify({ rooms: [rp(k)], pad: 'x'.repeat(64 * 1024 - 200) });
  assert.equal((await s.call('/api/phone/status', undefined, { raw: fits })).status, 200);
});

test('rate limit: 120 requests a minute per address, long polls count once', async (t) => {
  const s = await start({ holdMs: 300 });
  t.after(s.close);
  const k = newPair();
  // a long poll is one request
  await pcWait(s, newSid(), [k]);
  for (let i = 0; i < 119; i++) {
    const r = await s.call('/api/health', undefined, { method: 'GET' });
    assert.equal(r.status, 200, `request ${i + 2}`);
  }
  const over = await status(s, [k]);
  assert.equal(over.status, 429);
  assert.deepEqual(over.json, { ok: false, error: 'slow-down' });
  assert.ok(Number(over.headers.get('retry-after')) >= 1);
  // static files are not counted
  assert.equal((await fetch(`${s.base}/`)).status, 200);
  // CF-Connecting-IP is ignored without --trust-proxy
  const spoof = await s.call('/api/health', undefined, { method: 'GET', headers: { 'CF-Connecting-IP': '203.0.113.9' } });
  assert.equal(spoof.status, 429);
});

test('with --trust-proxy, CF-Connecting-IP is the address', async (t) => {
  const s = await start({ trustProxy: true, rateLimit: 3 });
  t.after(s.close);
  const get = (ip) => s.call('/api/health', undefined, { method: 'GET', headers: ip ? { 'CF-Connecting-IP': ip } : {} });
  for (let i = 0; i < 3; i++) assert.equal((await get('203.0.113.1')).status, 200);
  assert.equal((await get('203.0.113.1')).status, 429);
  assert.equal((await get('203.0.113.2')).status, 200, 'another address has its own budget');
  // IPv6 addresses share a budget per /64
  for (let i = 0; i < 3; i++) assert.equal((await get(`2001:db8:1:2::${i + 1}`)).status, 200);
  assert.equal((await get('2001:db8:1:2:ffff::9')).status, 429);
  assert.equal((await get('2001:db8:1:3::1')).status, 200);
  // without the header, the socket address
  for (let i = 0; i < 3; i++) assert.equal((await get(null)).status, 200);
  assert.equal((await get(null)).status, 429);
});

test('address keys', () => {
  assert.equal(addressKey('203.0.113.7'), '203.0.113.7');
  assert.equal(addressKey('::ffff:203.0.113.7'), '203.0.113.7');
  assert.equal(addressKey('2001:db8::1'), '2001:db8:0:0::/64');
  assert.equal(addressKey('2001:0db8:0000:0000:1234::1'), '2001:db8:0:0::/64');
  assert.equal(addressKey('2001:db8:aa:bb:cc:dd:ee:ff'), '2001:db8:aa:bb::/64');
  assert.equal(addressKey('::1'), '0:0:0:0::/64');
  assert.equal(addressKey('fe80::1%eth0'), 'fe80:0:0:0::/64');
});

test('queues are capped so memory can\'t be flooded', async (t) => {
  const s = await start({ holdMs: 100, onlineMs: 10_000, queueMax: 20 });
  t.after(s.close);
  const k = newPair();
  await pcWait(s, newSid(), [k]); // online for 10 s, no wait open
  for (let i = 0; i < 20; i++) {
    assert.deepEqual((await s.call('/api/phone/send', { ...rp(k), msg: phoneMsg(k) })).json, { ok: true });
  }
  const over = await s.call('/api/phone/send', { ...rp(k), msg: phoneMsg(k) });
  assert.equal(over.status, 429);
  // the PC gets the 20, in order
  const w = await pcWait(s, newSid(), [k]);
  assert.equal(w.json.msgs.length, 20);
  // a phone session holds at most 20 messages for the phone
  const m = phoneMsg(k, 'offer');
  await s.call('/api/phone/send', { ...rp(k), msg: m });
  for (let i = 0; i < 20; i++) assert.equal((await s.call('/api/pc/send', { ...rp(k), msg: sign(k, { t: 'answer', sid: m.sid, ts: i }) })).status, 200);
  assert.equal((await s.call('/api/pc/send', { ...rp(k), msg: sign(k, { t: 'answer', sid: m.sid }) })).status, 429);
  assert.equal((await s.call('/api/phone/wait', { ...rp(k), sid: m.sid })).json.msgs.length, 20);
});

test('the room cap makes way for new rooms by forgetting idle ones', async (t) => {
  const s = await start({ holdMs: 1000, roomsMax: 3 });
  t.after(s.close);
  const busy = newPair();
  const waiting = pcWait(s, newSid(), [busy]); // stays in use
  await sleep(50);
  const idle = [newPair(), newPair()];
  await status(s, idle);
  assert.equal(s.drop.stats().rooms, 3);
  const fresh = newPair();
  assert.equal((await status(s, [fresh])).json.rooms[0].state, 'offline');
  assert.equal(s.drop.stats().rooms, 3);
  // the busy room was not the one forgotten
  assert.equal((await status(s, [busy])).json.rooms[0].state, 'online');
  await waiting;
});

test('when every room is in use, new rooms are refused politely', async (t) => {
  const s = await start({ holdMs: 1000, roomsMax: 1 });
  t.after(s.close);
  const waiting = pcWait(s, newSid(), [newPair()]);
  await sleep(50);
  const k = newPair();
  assert.equal((await s.call('/api/ice', rp(k))).status, 429);
  assert.deepEqual((await status(s, [k])).json.rooms[0], { room: k.room, state: 'offline', seen: null });
  await waiting;
});

test('the server logs nothing about requests', async (t) => {
  const s = await start({ holdMs: 100 });
  t.after(s.close);
  const k = newPair();
  await pcWait(s, newSid(), [k]);
  await status(s, [k]);
  await s.call('/api/phone/send', { ...rp(k), msg: phoneMsg(k) });
  await s.call('/api/pc/unpair', rp(k));
  assert.deepEqual(s.logs, []);
});

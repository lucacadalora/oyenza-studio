// A PC emulator for paired phone drop: the PC half of docs/paired-drop.md,
// with the browser's own WebRTC, for the end-to-end test (run_e2e.py).
// It is served next to the meeting server's origin (Playwright routes
// /__emu/* to this folder) so it shares drop-core.js and the API.
//
// window.emu:
//   addPair(S)        listen on the room of pairing secret S (b64u), kicks the open wait
//   start() / stop()  the /api/pc/wait loop
//   unpair(S)         POST /api/pc/unpair
//   setShelf(list)    [{name, b64}] files the phone can save
//   summary()         what happened, as plain JSON
//   busy, decline, auto, askMs, failAfter, pairDelayMs, paceMs, maxPhones: knobs

import { deriveKeys, signMsg, verifyMsg, ReplayCache, randomSid, FRAME, HIGH_WATER, LOW_WATER } from '/drop-core.js';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const hex = (buf) => Array.from(new Uint8Array(buf), (b) => b.toString(16).padStart(2, '0')).join('');

const emu = {
  name: 'RTX-5090-Astral',
  client: randomSid(),
  pairs: new Map(), // room -> { S, keys, phoneName, paired, gone }
  running: false,
  busy: false, // answer every offer with "busy"
  decline: false, // answer every send with no/declined
  auto: true, // files land without asking
  askMs: 1500, // with auto off: the "user" accepts after this long
  failAfter: 0, // answer {"t":"failed","why":"disk"} once this many bytes of a send arrived
  maxPhones: 3,
  pairDelayMs: 0, // wait this long before "welcome" (for a screenshot of the pairing view)
  paceMs: 0, // spread "got" over at least this long (for a screenshot of the progress bar)
  shelf: [], // [{ id, name, bytes }]
  nextId: 0,
  received: [], // [{ name, size, sha256, from }]
  events: [],
  links: new Set(),
  replay: new ReplayCache(),
  abort: null,
  waits: 0,
};
window.emu = emu;

function log(type, data = {}) {
  emu.events.push({ type, at: Date.now(), ...data });
  const box = document.getElementById('log');
  if (box) box.textContent = emu.events.slice(-40).map((e) => `${new Date(e.at).toISOString().slice(11, 23)} ${e.type} ${JSON.stringify({ ...e, type: undefined, at: undefined })}`).join('\n');
}

async function post(path, body, signal) {
  const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal });
  let json = null;
  try {
    json = await r.json();
  } catch {
    json = null;
  }
  return { status: r.status, json };
}

function gathered(pc, ms) {
  return new Promise((resolve) => {
    if (pc.iceGatheringState === 'complete') return resolve();
    const t = setTimeout(resolve, ms);
    pc.addEventListener('icegatheringstatechange', () => {
      if (pc.iceGatheringState === 'complete') {
        clearTimeout(t);
        resolve();
      }
    });
  });
}

function drained(dc) {
  return new Promise((resolve) => {
    dc.addEventListener('bufferedamountlow', () => resolve(), { once: true });
    dc.addEventListener('close', () => resolve(), { once: true });
  });
}

emu.addPair = async (S) => {
  const keys = await deriveKeys(S);
  emu.pairs.set(keys.room, { S, keys, phoneName: null, paired: false, gone: false });
  log('listen', { room: keys.room.slice(0, 8) });
  if (emu.running) await post('/api/pc/kick', { client: emu.client });
  return keys.room;
};

emu.start = () => {
  if (emu.running) return;
  emu.running = true;
  loop();
};

emu.stop = () => {
  emu.running = false;
  if (emu.abort) emu.abort.abort();
  for (const l of emu.links) {
    try {
      l.pc.close();
    } catch {
      // gone
    }
  }
  emu.links.clear();
  log('stopped');
};

emu.unpair = async (S) => {
  const keys = await deriveKeys(S);
  const r = await post('/api/pc/unpair', { room: keys.room, proof: keys.proof });
  const p = emu.pairs.get(keys.room);
  if (p) p.gone = true;
  log('unpair', { status: r.status });
  return r.status;
};

emu.setShelf = (list) => {
  emu.shelf = list.map((f) => ({ id: emu.nextId++, name: f.name, bytes: Uint8Array.from(atob(f.b64), (c) => c.charCodeAt(0)) }));
};

emu.summary = () => ({
  received: emu.received,
  events: emu.events.map((e) => ({ ...e })),
  links: emu.links.size,
  waits: emu.waits,
  pairs: [...emu.pairs.values()].map((p) => ({ room: p.keys.room, phoneName: p.phoneName, paired: p.paired, gone: p.gone })),
  shelf: emu.shelf.map((s) => ({ id: s.id, name: s.name, size: s.bytes.length })),
});

async function loop() {
  while (emu.running) {
    const pairs = [...emu.pairs.values()].filter((p) => !p.gone);
    if (!pairs.length) {
      await sleep(200);
      continue;
    }
    emu.abort = new AbortController();
    let r;
    try {
      emu.waits += 1;
      r = await post('/api/pc/wait', { rooms: pairs.map((p) => ({ room: p.keys.room, proof: p.keys.proof })), client: emu.client }, emu.abort.signal);
    } catch {
      if (!emu.running) break;
      await sleep(500);
      continue;
    }
    if (r.status !== 200 || !r.json) {
      log('wait-error', { status: r.status, error: r.json && r.json.error });
      await sleep(1000);
      continue;
    }
    for (const room of r.json.gone || []) {
      const p = emu.pairs.get(room);
      if (p) p.gone = true;
      log('gone', { room: room.slice(0, 8) });
    }
    for (const room of r.json.bad || []) log('bad', { room: room.slice(0, 8) });
    for (const { room, msg } of r.json.msgs || []) {
      handle(room, msg).catch((e) => log('error', { what: String(e && e.message ? e.message : e) }));
    }
  }
}

async function reply(p, fields) {
  const msg = await signMsg(p.keys, { ...fields, ts: Date.now() });
  const r = await post('/api/pc/send', { room: p.keys.room, proof: p.keys.proof, msg });
  if (r.status !== 200) log('reply-error', { t: fields.t, status: r.status, error: r.json && r.json.error });
}

async function handle(room, msg) {
  const p = emu.pairs.get(room);
  if (!p) return;
  if (!(await verifyMsg(p.keys, msg, { types: ['pair', 'offer'], replay: emu.replay }))) {
    log('dropped', { t: msg && msg.t });
    return;
  }
  if (msg.t === 'pair') {
    log('pair', { name: msg.name });
    if (emu.pairDelayMs) await sleep(emu.pairDelayMs);
    p.phoneName = msg.name;
    p.paired = true;
    await reply(p, { t: 'welcome', sid: msg.sid, name: emu.name, sdp: '' });
    return;
  }
  log('offer', { name: msg.name });
  if (emu.busy || emu.links.size >= emu.maxPhones) {
    await reply(p, { t: 'busy', sid: msg.sid, name: emu.name, sdp: '' });
    log('busy');
    return;
  }
  const ice = await post('/api/ice', { room: p.keys.room, proof: p.keys.proof });
  const pc = new RTCPeerConnection({ iceServers: (ice.json && ice.json.iceServers) || [] });
  const link = { pc, phone: msg.name };
  emu.links.add(link);
  pc.addEventListener('connectionstatechange', () => {
    if (pc.connectionState === 'failed' || pc.connectionState === 'closed') emu.links.delete(link);
  });
  pc.addEventListener('datachannel', (e) => serve(e.channel, link));
  await pc.setRemoteDescription({ type: 'offer', sdp: msg.sdp });
  await pc.setLocalDescription(await pc.createAnswer());
  await gathered(pc, 3000);
  await reply(p, { t: 'answer', sid: msg.sid, name: emu.name, sdp: pc.localDescription.sdp });
  log('answered', { label: 'oyen-drop' });
}

const shelfList = () => emu.shelf.map((s) => ({ id: s.id, name: s.name, size: s.bytes.length }));

function serve(dc, link) {
  dc.binaryType = 'arraybuffer';
  dc.bufferedAmountLowThreshold = LOW_WATER;
  if (dc.label !== 'oyen-drop' || !dc.ordered) log('protocol', { what: `channel ${dc.label} ordered=${dc.ordered}` });
  let recv = null;
  let failed = false;
  const sendJson = (o) => dc.send(JSON.stringify(o));
  const close = () => {
    try {
      dc.close();
    } catch {
      // gone
    }
    setTimeout(() => {
      try {
        link.pc.close();
      } catch {
        // gone
      }
    }, 100);
    emu.links.delete(link);
  };
  dc.addEventListener('close', () => {
    emu.links.delete(link);
    if (recv) {
      clearInterval(recv.timer);
      log('send-dropped', { got: recv.got, total: recv.total });
      recv = null;
    }
  });
  dc.addEventListener('message', (e) => {
    if (typeof e.data !== 'string') return onBytes(new Uint8Array(e.data));
    let m;
    try {
      m = JSON.parse(e.data);
    } catch {
      return;
    }
    if (m.t === 'hello') {
      link.phone = m.name;
      log('hello', { name: m.name, v: m.v });
      sendJson({ t: 'hello', v: 1, name: emu.name, auto: emu.auto, shelf: shelfList() });
    } else if (m.t === 'send') startRecv(m);
    else if (m.t === 'get') sendFile(m.id);
    else if (m.t === 'bye') {
      log('bye');
      close();
    }
  });

  function startRecv(m) {
    const files = Array.isArray(m.files) ? m.files : [];
    const total = files.reduce((s, f) => s + (Number(f.size) || 0), 0);
    if (recv) return sendJson({ t: 'no', why: 'busy' });
    if (!files.length || files.length > 500) return sendJson({ t: 'no', why: 'too-many' });
    if (total > 64e9) return sendJson({ t: 'no', why: 'too-large' });
    if (emu.decline) return sendJson({ t: 'no', why: 'declined' });
    recv = { files: files.map((f) => ({ name: f.name, size: f.size, parts: [], got: 0 })), idx: 0, got: 0, total, start: Date.now(), reported: -1, done: false };
    log('send', { files: files.map((f) => `${f.name}:${f.size}`) });
    const r = recv;
    const accept = () => {
      if (recv !== r || dc.readyState !== 'open') return;
      sendJson({ t: 'ok' });
      r.timer = setInterval(report, 250);
    };
    if (emu.auto) accept();
    else setTimeout(accept, emu.askMs);
    skipFull();
    if (recv.got === recv.total) recv.done = true;
  }

  function skipFull() {
    while (recv.idx < recv.files.length && recv.files[recv.idx].got === recv.files[recv.idx].size) recv.idx += 1;
  }

  function onBytes(bytes) {
    if (failed) return; // a failed send's remaining bytes are dropped
    if (!recv) {
      log('protocol', { what: 'bytes outside a send' });
      return close();
    }
    let off = 0;
    while (off < bytes.length) {
      const f = recv.files[recv.idx];
      if (!f) {
        log('protocol', { what: 'more bytes than announced' });
        return close();
      }
      const take = Math.min(f.size - f.got, bytes.length - off);
      f.parts.push(bytes.slice(off, off + take));
      f.got += take;
      recv.got += take;
      off += take;
      skipFull();
    }
    if (recv.got === recv.total) recv.done = true;
    if (emu.failAfter && recv.got >= emu.failAfter && !recv.done) {
      clearInterval(recv.timer);
      recv = null;
      failed = true;
      sendJson({ t: 'failed', why: 'disk' });
      log('failed', { why: 'disk' });
    }
  }

  function report() {
    if (!recv) return;
    const paced = emu.paceMs ? Math.floor((recv.total * Math.min(1, (Date.now() - recv.start) / emu.paceMs))) : recv.total;
    const n = Math.min(recv.got, paced);
    if (n !== recv.reported) {
      recv.reported = n;
      sendJson({ t: 'got', n });
    }
    if (recv.done && n === recv.total) finish();
  }

  async function finish() {
    const r = recv;
    recv = null;
    clearInterval(r.timer);
    for (const f of r.files) {
      const bytes = new Uint8Array(await new Blob(f.parts).arrayBuffer());
      emu.received.push({ name: f.name, size: f.size, sha256: hex(await crypto.subtle.digest('SHA-256', bytes)), from: link.phone });
      emu.shelf.unshift({ id: emu.nextId++, name: f.name, bytes });
    }
    sendJson({ t: 'saved', files: r.files.length });
    sendJson({ t: 'shelf', files: shelfList() });
    log('saved', { files: r.files.length });
  }

  async function sendFile(id) {
    const item = emu.shelf.find((s) => s.id === id);
    if (!item) {
      sendJson({ t: 'nofile', id });
      return;
    }
    sendJson({ t: 'file', id, name: item.name, size: item.bytes.length });
    for (let off = 0; off < item.bytes.length; off += FRAME) {
      if (dc.bufferedAmount > HIGH_WATER) await drained(dc);
      if (dc.readyState !== 'open') return;
      dc.send(item.bytes.slice(off, off + FRAME));
    }
    log('file-sent', { id, name: item.name, size: item.bytes.length });
  }
}

log('ready', { client: emu.client.slice(0, 6) });

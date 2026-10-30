// Oyenza Studio phone page: pair with a PC once, then send it photos and files
// from any network, and save Shelf files on the phone.
// Contract: docs/paired-drop.md. No dependencies, no inline code (strict CSP).

import {
  deriveKeys, signMsg, verifyMsg, ReplayCache, randomSid, cleanName, secretOk,
  FRAME, HIGH_WATER, LOW_WATER, MAX_FILES, MAX_TOTAL,
} from './drop-core.js';
import { setIsland, islandProgress, pose, carrying } from './island.js';

const $ = (id) => document.getElementById(id);

const STORE_KEY = 'oyen-drop.v1';
const SEND_RETRY_MS = 1500; // an offline PC is asked again this often...
const PAIR_RETRY_MS = 20_000; // ...for this long while pairing (it may just be starting to listen)
const OFFER_RETRY_MS = 6_000; // ...and this long for the first connect right after pairing
const FRESH_PAIR_MS = 2 * 60_000; // what "right after pairing" means
const ANSWER_MS = 20_000; // wait for welcome / answer
const GATHER_MS = 3_000; // ICE gathering, no trickle
const OPEN_MS = 15_000; // data channel open after the answer
const IDLE_MS = 60_000; // close the connection after this long with nothing to do
const STALL_MS = 30_000; // a transfer with no sign of life for this long has failed
const STATUS_EVERY_MS = 10_000;
const GET_MAX = 4e9; // larger Shelf files are not pulled into a phone's memory

// ---------------------------------------------------------------- platform

const UA = navigator.userAgent || '';
const IS_IPAD = /iPad/.test(UA) || (/Macintosh/.test(UA) && (navigator.maxTouchPoints || 0) > 1);
const IS_IPHONE = /iPhone|iPod/.test(UA);
const IS_IOS = IS_IPHONE || IS_IPAD;
const IS_ANDROID = /Android/i.test(UA);

function isStandalone() {
  try {
    return navigator.standalone === true || matchMedia('(display-mode: standalone)').matches;
  } catch {
    return false;
  }
}

// iOS keeps a Home Screen web app's storage apart from Safari's, and with a
// manifest it would open the app at start_url and lose the #k= pairing link.
// So iOS gets no manifest: Add to Home Screen saves the page's own address.
if (!IS_IOS) {
  const link = document.createElement('link');
  link.rel = 'manifest';
  link.href = '/manifest.webmanifest';
  document.head.appendChild(link);
}

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').catch(() => {});
}

// The page works like an app, so it doesn't pinch-zoom. Android keeps to the viewport tag;
// Safari ignores user-scalable=no, so its pinch gesture is stopped here.
for (const type of ['gesturestart', 'gesturechange']) document.addEventListener(type, (e) => e.preventDefault());

let installPrompt = null;
let installed = false;
addEventListener('beforeinstallprompt', (e) => {
  e.preventDefault();
  installPrompt = e;
  renderKeep();
});
addEventListener('appinstalled', () => {
  installPrompt = null;
  installed = true;
  renderKeep();
});

let defaultName = IS_IPHONE ? 'iPhone' : IS_IPAD ? 'iPad' : IS_ANDROID ? 'Android phone' : 'Phone';

async function loadDefaultName() {
  const uad = navigator.userAgentData;
  if (!IS_ANDROID || !uad || typeof uad.getHighEntropyValues !== 'function') return;
  try {
    const v = await uad.getHighEntropyValues(['model']);
    const model = cleanName(v && v.model);
    if (model) defaultName = model;
  } catch {
    // keep "Android phone"
  }
  if ($('phone-name')) $('phone-name').placeholder = defaultName;
}

// ---------------------------------------------------------------- storage

function readStore() {
  let v = null;
  try {
    v = JSON.parse(localStorage.getItem(STORE_KEY) || 'null');
  } catch {
    v = null;
  }
  const pairs = [];
  const seen = new Set();
  if (v && Array.isArray(v.pairs)) {
    for (const p of v.pairs) {
      if (!p || !secretOk(p.k) || seen.has(p.k)) continue;
      seen.add(p.k);
      pairs.push({ k: p.k, name: cleanName(p.name), added: Number.isFinite(p.added) ? p.added : 0 });
    }
  }
  return { pairs, phoneName: v && typeof v.phoneName === 'string' ? cleanName(v.phoneName) : '' };
}

let store = readStore();

function writeStore() {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(store));
  } catch {
    // private browsing or full: it works for this visit only
  }
}

const findPair = (k) => store.pairs.find((p) => p.k === k) || null;

function savePair(k, name, { fresh = false, keepName = false } = {}) {
  store = readStore();
  let p = findPair(k);
  const clean = cleanName(name);
  if (!p) {
    p = { k, name: clean, added: Date.now() };
    store.pairs.push(p);
  } else {
    if (clean && (!keepName || !p.name)) p.name = clean;
    if (fresh) p.added = Date.now();
  }
  writeStore();
  return p;
}

function renamePair(k, name) {
  const clean = cleanName(name);
  if (!clean) return;
  store = readStore();
  const p = findPair(k);
  if (p && p.name !== clean) {
    p.name = clean;
    writeStore();
  }
}

function forgetPair(k) {
  store = readStore();
  store.pairs = store.pairs.filter((p) => p.k !== k);
  writeStore();
  statusByK.delete(k);
  // iOS Safari keeps the pairing link in the address after pairing; a forgotten
  // PC must not come back from it on the next reload.
  if (new URLSearchParams(location.hash.slice(1)).get('k') === k) history.replaceState(history.state, '', '/');
}

const phoneName = () => store.phoneName || defaultName;
const pcName = (p) => (p && p.name) || 'your PC';
const PcName = (p) => (p && p.name) || 'Your PC';

// ---------------------------------------------------------------- little helpers

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const ICON = {
  photo: '<path d="M4 7.5A1.5 1.5 0 0 1 5.5 6h13A1.5 1.5 0 0 1 20 7.5v9a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 16.5z"/><path d="M4 15l4.5-4.5L13 15l2.5-2.5L20 17"/><circle cx="15.5" cy="9.5" r="1.3"/>',
  video: '<path d="M4 7.5A1.5 1.5 0 0 1 5.5 6h9A1.5 1.5 0 0 1 16 7.5v9a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 4 16.5z"/><path d="M16 10.5l4-2.5v8l-4-2.5"/>',
  file: '<path d="M7.5 3H14l5 5v11.5a1.5 1.5 0 0 1-1.5 1.5h-10A1.5 1.5 0 0 1 6 19.5v-15A1.5 1.5 0 0 1 7.5 3z"/><path d="M14 3v5h5"/>',
  pc: '<rect x="3.5" y="4.5" width="17" height="11.5" rx="2"/><path d="M9 20h6M12 16v4"/>',
  chev: '<path d="M9.5 5.5 16 12l-6.5 6.5"/>',
  wifi: '<path d="M4.5 9.6a11 11 0 0 1 15 0"/><path d="M7.4 12.9a6.6 6.6 0 0 1 9.2 0"/><path d="M10.3 16.1a2.4 2.4 0 0 1 3.4 0"/><path d="M12 19h.01"/>',
  globe: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17"/><path d="M12 3.5c2.3 2.4 3.5 5.2 3.5 8.5s-1.2 6.1-3.5 8.5c-2.3-2.4-3.5-5.2-3.5-8.5s1.2-6.1 3.5-8.5z"/>',
  lock: '<rect x="5.5" y="10.5" width="13" height="9.5" rx="2"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
  pause: '<circle cx="12" cy="12" r="8.5"/><path d="M10 9.5v5M14 9.5v5"/>',
  off: '<circle cx="12" cy="12" r="8.5"/><path d="M8.5 12h7"/>',
};

function svg(name) {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = ICON[name];
  return s;
}

// "Send to" + the PC's name on its own line, sized so a long name still fits.
function titleWithName(id, lead, name) {
  const h = $(id);
  h.textContent = '';
  h.appendChild(document.createTextNode(`${lead} `));
  const n = el('span', 'pcname', name);
  const len = [...name].length;
  n.dataset.len = len > 18 ? 'xl' : len > 12 ? 'l' : 's';
  h.appendChild(n);
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function fmt(n) {
  if (n < 1000) return n + (n === 1 ? ' byte' : ' bytes');
  if (n < 1e6) return `${Math.round(n / 1e3)} KB`;
  if (n < 1e9) {
    const mb = n / 1e6;
    return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
  }
  return `${(n / 1e9).toFixed(1)} GB`;
}

const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

function kindOf(name, type) {
  const ext = (String(name).split('.').pop() || '').toLowerCase();
  if (/^image\//.test(type || '') || /^(jpe?g|png|gif|webp|heic|heif|avif|bmp|tiff?|dng)$/.test(ext)) return 'photo';
  if (/^video\//.test(type || '') || /^(mp4|mov|m4v|avi|mkv|webm|3gp)$/.test(ext)) return 'video';
  return 'file';
}

// "3 photos", "1 video", "2 files"
function describe(files, n = files.length) {
  if (n !== files.length) return plural(n, 'file', 'files');
  const kinds = new Set(files.map((f) => kindOf(f.name, f.type)));
  if (kinds.size === 1 && kinds.has('photo')) return plural(n, 'photo', 'photos');
  if (kinds.size === 1 && kinds.has('video')) return plural(n, 'video', 'videos');
  return plural(n, 'file', 'files');
}

function ago(ms) {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return 'just now';
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.floor(h / 24);
  return d === 1 ? '1 day ago' : `${d} days ago`;
}

const MIME = {
  jpg: 'image/jpeg', jpeg: 'image/jpeg', png: 'image/png', gif: 'image/gif', webp: 'image/webp', heic: 'image/heic',
  heif: 'image/heif', avif: 'image/avif', bmp: 'image/bmp', tif: 'image/tiff', tiff: 'image/tiff', svg: 'image/svg+xml',
  mp4: 'video/mp4', m4v: 'video/mp4', mov: 'video/quicktime', webm: 'video/webm', mkv: 'video/x-matroska', avi: 'video/x-msvideo',
  mp3: 'audio/mpeg', m4a: 'audio/mp4', wav: 'audio/wav', ogg: 'audio/ogg', flac: 'audio/flac',
  pdf: 'application/pdf', txt: 'text/plain', csv: 'text/csv', md: 'text/markdown', html: 'text/html', json: 'application/json',
  zip: 'application/zip', doc: 'application/msword', xls: 'application/vnd.ms-excel', ppt: 'application/vnd.ms-powerpoint',
  docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  pptx: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
};
const mimeOf = (name) => MIME[(String(name).split('.').pop() || '').toLowerCase()] || 'application/octet-stream';

// A file name from the PC, as one plain name.
function safeFileName(name, fallback = 'file') {
  const base = String(name || '').split(/[\\/]/).pop() || '';
  const one = base.replace(/[\u0000-\u001f\u007f]+/g, ' ').replace(/\s+/g, ' ').trim();
  return [...one].slice(0, 200).join('') || fallback;
}

function fileRow(name, size, action, file) {
  const li = el('li', 'file');
  const k = kindOf(name, file && file.type);
  const kind = el('span', `kind ${k}`);
  if (file && /^image\/(jpeg|png|gif|webp|avif)$/.test(file.type || '') && file.size < 40e6) {
    // A real thumbnail of the photo (decoded, then the blob URL is let go).
    const url = URL.createObjectURL(file);
    const img = el('img', 'thumb');
    img.alt = '';
    img.decoding = 'async';
    img.addEventListener('load', () => URL.revokeObjectURL(url), { once: true });
    img.addEventListener('error', () => {
      URL.revokeObjectURL(url);
      img.replaceWith(svg(k));
    }, { once: true });
    img.src = url;
    kind.appendChild(img);
  } else {
    kind.appendChild(svg(k));
  }
  li.appendChild(kind);
  const text = el('span', 'name');
  text.appendChild(el('b', '', name));
  text.appendChild(el('span', '', fmt(size)));
  li.appendChild(text);
  li.appendChild(action || el('span'));
  return li;
}

function setStatus(prefix, text, kind = 'wait') {
  const box = $(`${prefix}-status`);
  islandNews(prefix, kind, text);
  if (!text) {
    box.hidden = true;
    return;
  }
  box.hidden = false;
  box.className = `status ${kind}`;
  const spin = $(`${prefix}-spin`);
  if (spin) spin.hidden = kind !== 'wait';
  $(`${prefix}-status-text`).textContent = text;
}

// Progress bar for a send: "Sending 2 of 3" and "1.2 MB of 3.0 MB".
function showProgress(prefix, done, total, files) {
  const box = $(prefix === 'send' ? 'progress' : 'share-progress');
  const f = Math.max(0, Math.min(1, total ? done / total : 1));
  const sub = `${Math.round(f * 100)}% · ${fmt(Math.min(done, total))} of ${fmt(total)}`;
  if (box.hidden) {
    const kinds = new Set(files.map((x) => kindOf(x.name, x.type)));
    setIsland('busy', { title: `Sending ${describe(files)}`, sub, progress: f, thumb: files[0], icon: kinds.size === 1 && kinds.has('photo') ? 'photo' : 'file' });
    carrying(prefix === 'send' ? 'pc-oyen' : 'share-oyen', true);
  } else {
    islandProgress(f, sub);
  }
  box.hidden = false;
  $(prefix === 'send' ? 'bar' : 'share-bar').style.transform = `scaleX(${f})`;
  let i = 0;
  let sum = 0;
  while (i < files.length - 1 && sum + files[i].size <= done) sum += files[i++].size;
  $(prefix === 'send' ? 'p-file' : 'share-p-file').textContent = files.length > 1 ? `Sending ${i + 1} of ${files.length}` : 'Sending';
  $(prefix === 'send' ? 'p-bytes' : 'share-p-bytes').textContent = `${fmt(Math.min(done, total))} of ${fmt(total)}`;
}

function hideProgress(prefix) {
  const box = $(prefix === 'send' ? 'progress' : 'share-progress');
  if (!box.hidden) {
    carrying(prefix === 'send' ? 'pc-oyen' : 'share-oyen', false);
    restoreIsland();
  }
  box.hidden = true;
}

// ---------------------------------------------------------------- the island

// What the island shows when nothing is on the move, per view.
let islandRest = () => setIsland('idle');

function restoreIsland() {
  islandRest();
}

// A finished send, a save, a problem: short news on the island.
function islandNews(prefix, kind, text) {
  if (!text || (kind !== 'ok' && kind !== 'bad')) return;
  const who = pcv.pair ? PcName(pcv.pair) : '';
  if (kind === 'ok') {
    if (prefix === 'send' || prefix === 'share') {
      setIsland('done', { title: 'On the Shelf', sub: who, hold: 3200 });
      pose(prefix === 'send' ? 'pc-oyen' : 'share-oyen', 'shelf', 'hop');
    } else if (prefix === 'save') {
      setIsland('done', { title: 'Saved', sub: 'On this phone', hold: 2400 });
    }
  } else if (prefix === 'send' || prefix === 'share' || prefix === 'save') {
    setIsland('oops', { title: prefix === 'save' ? 'Not saved' : 'Not sent', sub: 'Nothing was kept', hold: 3600 });
  }
}


// ---------------------------------------------------------------- wake lock

let wakeLock = null;
let wakeWanted = 0;

async function holdWake() {
  wakeWanted += 1;
  try {
    if (!wakeLock && navigator.wakeLock && document.visibilityState === 'visible') {
      wakeLock = await navigator.wakeLock.request('screen');
      wakeLock.addEventListener('release', () => {
        wakeLock = null;
      });
    }
  } catch {
    // not allowed or not supported: fine
  }
}

function releaseWake() {
  wakeWanted = Math.max(0, wakeWanted - 1);
  if (wakeWanted || !wakeLock) return;
  try {
    wakeLock.release();
  } catch {
    // already gone
  }
  wakeLock = null;
}

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible' && wakeWanted && !wakeLock) {
    wakeWanted -= 1;
    holdWake();
  }
});

// ---------------------------------------------------------------- meeting server

class Problem extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}

const keyCache = new Map();
function keysFor(k) {
  let p = keyCache.get(k);
  if (!p) {
    p = deriveKeys(k);
    keyCache.set(k, p);
  }
  return p;
}

async function api(path, body, signal) {
  let r;
  try {
    r = await fetch(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      cache: 'no-store',
      signal,
    });
  } catch {
    throw new Problem(signal && signal.aborted ? 'aborted' : 'network');
  }
  let j = null;
  try {
    j = await r.json();
  } catch {
    if (signal && signal.aborted) throw new Problem('aborted');
  }
  if (!r.ok) throw new Problem((j && typeof j.error === 'string' && j.error) || 'server');
  return j || {};
}

// Sends one signed message. While the PC is offline it asks again every 1.5 s
// for `retryFor` ms (a PC can take a moment to start listening on a new room).
async function phoneSend(keys, fields, retryFor, signal) {
  const until = Date.now() + retryFor;
  for (;;) {
    if (signal && signal.aborted) throw new Problem('cancelled');
    const msg = await signMsg(keys, { ...fields, ts: Date.now() });
    const r = await api('/api/phone/send', { room: keys.room, proof: keys.proof, msg }, signal);
    if (r.ok) return;
    if (r.error !== 'offline') throw new Problem(r.error || 'server');
    if (Date.now() + SEND_RETRY_MS > until) throw new Problem('offline');
    await sleep(SEND_RETRY_MS);
  }
}

// Long-polls the session until a valid signed reply of one of `types` comes, or `ms` pass.
async function waitReply(keys, sid, types, ms, signal) {
  const until = Date.now() + ms;
  const replay = new ReplayCache();
  while (Date.now() < until) {
    if (signal && signal.aborted) throw new Problem('cancelled');
    const ac = new AbortController();
    const timer = setTimeout(() => ac.abort(), until - Date.now());
    const stop = () => ac.abort();
    if (signal) signal.addEventListener('abort', stop);
    let r;
    try {
      r = await api('/api/phone/wait', { room: keys.room, proof: keys.proof, sid }, ac.signal);
    } catch (e) {
      if (signal && signal.aborted) throw new Problem('cancelled');
      if (e.code === 'aborted' || e.code === 'no-session') return null;
      if (e.code === 'network') {
        await sleep(1000);
        continue;
      }
      throw e;
    } finally {
      clearTimeout(timer);
      if (signal) signal.removeEventListener('abort', stop);
    }
    for (const m of Array.isArray(r.msgs) ? r.msgs : []) {
      if (await verifyMsg(keys, m, { types, sid, replay })) return m;
    }
  }
  return null;
}

// ---------------------------------------------------------------- the link to one PC

function gathered(pc, ms) {
  return new Promise((resolve) => {
    if (pc.iceGatheringState === 'complete') return resolve();
    const done = () => {
      clearTimeout(t);
      pc.removeEventListener('icegatheringstatechange', check);
      resolve();
    };
    const check = () => {
      if (pc.iceGatheringState === 'complete') done();
    };
    const t = setTimeout(done, ms);
    pc.addEventListener('icegatheringstatechange', check);
  });
}

function shelfList(files) {
  if (!Array.isArray(files)) return [];
  return files
    .filter((f) => f && Number.isSafeInteger(f.id) && typeof f.name === 'string' && Number.isSafeInteger(f.size) && f.size >= 0)
    .map((f) => ({ id: f.id, name: safeFileName(f.name), size: f.size }));
}

class Link {
  constructor(pair) {
    this.pair = pair;
    this.pc = null;
    this.dc = null;
    this.ready = false;
    this.closed = false;
    this.busy = false;
    this.waitingForUser = false;
    this.auto = true;
    this.shelf = [];
    this.waiters = [];
    this.incoming = null;
    this.pendingGet = null;
    this.onprogress = null;
    this.onshelf = () => {};
    this.onname = () => {};
    this.onclose = () => {};
    this.abort = new AbortController();
    this.lastActivity = Date.now();
    this.lastBuffered = 0;
    this.watch = null;
    this.streaming = false;
    this.finishing = false;
    this.stopStream = null;
  }

  async open() {
    const keys = await keysFor(this.pair.k);
    const signal = this.abort.signal;
    const ice = await api('/api/ice', { room: keys.room, proof: keys.proof }, signal);
    if (this.closed) throw new Problem('cancelled');
    let pc;
    try {
      pc = new RTCPeerConnection({ iceServers: Array.isArray(ice.iceServers) ? ice.iceServers : [] });
    } catch {
      throw new Problem('no-webrtc');
    }
    this.pc = pc;
    const dc = pc.createDataChannel('oyen-drop', { ordered: true });
    this.dc = dc;
    dc.binaryType = 'arraybuffer';
    dc.bufferedAmountLowThreshold = LOW_WATER;
    dc.addEventListener('message', (e) => this.onMessage(e.data));
    dc.addEventListener('close', () => this.shutdown('closed'));
    pc.addEventListener('connectionstatechange', () => {
      if (pc.connectionState === 'failed') this.shutdown('no-route');
    });
    await pc.setLocalDescription(await pc.createOffer());
    await gathered(pc, GATHER_MS);
    if (this.closed) throw new Problem('cancelled');

    const sid = randomSid();
    const fresh = Date.now() - (this.pair.added || 0) < FRESH_PAIR_MS;
    await phoneSend(keys, { t: 'offer', sid, name: phoneName(), sdp: pc.localDescription.sdp }, fresh ? OFFER_RETRY_MS : 0, signal);
    const reply = await waitReply(keys, sid, ['answer', 'busy'], ANSWER_MS, signal);
    if (this.closed) throw new Problem('cancelled');
    if (!reply) throw new Problem('no-answer');
    if (reply.name) this.onname(reply.name);
    if (reply.t === 'busy') throw new Problem('busy');
    try {
      await pc.setRemoteDescription({ type: 'answer', sdp: reply.sdp });
    } catch {
      throw new Problem('protocol');
    }
    await this.opened();
    this.sendJson({ t: 'hello', v: 1, name: phoneName() });
    const hello = await this.expect((m) => m.t === 'hello', 10_000, 'no-answer');
    this.auto = hello.auto !== false;
    this.shelf = shelfList(hello.shelf);
    if (typeof hello.name === 'string' && cleanName(hello.name)) this.onname(cleanName(hello.name));
    this.ready = true;
    this.watch = setInterval(() => {
      const buffered = this.dc ? this.dc.bufferedAmount : 0;
      if (buffered !== this.lastBuffered) {
        // bytes still leaving the phone count as progress, however rarely the PC reports
        this.lastBuffered = buffered;
        this.lastActivity = Date.now();
      }
      const limit = this.finishing ? STALL_MS * 4 : STALL_MS;
      if (this.busy && !this.waitingForUser && Date.now() - this.lastActivity > limit) this.shutdown('stalled');
    }, 5_000);
    this.onshelf(this.shelf);
    return this;
  }

  opened() {
    return new Promise((resolve, reject) => {
      if (this.dc.readyState === 'open') return resolve();
      const w = { match: null, resolve, reject, timer: null };
      w.timer = setTimeout(() => this.drop(w, new Problem('no-route')), OPEN_MS);
      this.waiters.push(w);
      this.dc.addEventListener('open', () => this.drop(w, null), { once: true });
    });
  }

  drop(w, err, value) {
    const i = this.waiters.indexOf(w);
    if (i < 0) return;
    this.waiters.splice(i, 1);
    clearTimeout(w.timer);
    if (err) w.reject(err);
    else w.resolve(value);
  }

  // Resolves with the first text message that matches, or rejects after ms (0 = no limit).
  expect(match, ms, code = 'timeout') {
    if (this.closed) return Promise.reject(new Problem(this.closeReason || 'closed'));
    return new Promise((resolve, reject) => {
      const w = { match, resolve, reject, timer: null };
      if (ms) w.timer = setTimeout(() => this.drop(w, new Problem(code)), ms);
      this.waiters.push(w);
    });
  }

  sendJson(obj) {
    if (!this.dc || this.dc.readyState !== 'open') throw new Problem(this.closeReason || 'closed');
    this.dc.send(JSON.stringify(obj));
    this.lastActivity = Date.now();
  }

  onMessage(data) {
    this.lastActivity = Date.now();
    if (typeof data !== 'string') return this.onBinary(data);
    let m;
    try {
      m = JSON.parse(data);
    } catch {
      return;
    }
    if (!m || typeof m !== 'object' || typeof m.t !== 'string') return;
    if (m.t === 'file' && this.pendingGet && m.id === this.pendingGet.id && !this.incoming) {
      if (!Number.isSafeInteger(m.size) || m.size < 0) return this.shutdown('protocol');
      if (m.size > GET_MAX) return this.shutdown('too-big');
      const g = this.pendingGet;
      if (m.size === 0) g.resolve([]);
      else this.incoming = { size: m.size, got: 0, parts: [], blobs: [], partBytes: 0, g };
    } else if (m.t === 'shelf') {
      this.shelf = shelfList(m.files);
      this.onshelf(this.shelf);
    } else if (m.t === 'got' && this.onprogress && Number.isFinite(m.n)) {
      this.onprogress(m.n);
    } else if (m.t === 'failed' && this.streaming) {
      this.stopStream = `failed-${m.why}`;
    }
    for (const w of this.waiters) {
      if (w.match && w.match(m)) {
        this.drop(w, null, m);
        break;
      }
    }
  }

  onBinary(data) {
    const inc = this.incoming;
    if (!inc) return this.shutdown('protocol');
    const n = typeof data.byteLength === 'number' ? data.byteLength : data.size;
    inc.parts.push(data);
    inc.partBytes += n;
    inc.got += n;
    if (inc.got > inc.size) return this.shutdown('protocol');
    if (inc.partBytes >= 16 * 1024 * 1024) {
      inc.blobs.push(new Blob(inc.parts));
      inc.parts = [];
      inc.partBytes = 0;
    }
    if (inc.g.onProgress) inc.g.onProgress(inc.got, inc.size);
    if (inc.got === inc.size) {
      this.incoming = null;
      inc.g.resolve([...inc.blobs, ...inc.parts]);
    }
  }

  drained() {
    return new Promise((resolve, reject) => {
      const dc = this.dc;
      const done = (err) => {
        dc.removeEventListener('bufferedamountlow', onLow);
        dc.removeEventListener('close', onClose);
        if (err) reject(err);
        else resolve();
      };
      const onLow = () => done(null);
      const onClose = () => done(new Problem(this.closeReason || 'closed'));
      dc.addEventListener('bufferedamountlow', onLow);
      dc.addEventListener('close', onClose);
    });
  }

  async stream(file) {
    const READ = 1024 * 1024;
    for (let off = 0; off < file.size; off += READ) {
      const end = Math.min(file.size, off + READ);
      let buf;
      try {
        buf = await file.slice(off, end).arrayBuffer();
      } catch {
        buf = null;
      }
      if (!buf || buf.byteLength !== end - off) {
        // the PC is still counting bytes: only closing ends this send
        this.shutdown('read');
        throw new Problem('read');
      }
      for (let i = 0; i < buf.byteLength; i += FRAME) {
        if (this.closed) throw new Problem(this.closeReason || 'closed');
        if (this.stopStream) throw new Problem(this.stopStream);
        if (this.dc.bufferedAmount > HIGH_WATER) await this.drained();
        this.dc.send(buf.slice(i, Math.min(buf.byteLength, i + FRAME)));
      }
      this.lastActivity = Date.now();
    }
  }

  // Sends files to the Shelf. Resolves with how many the PC saved.
  async send(files, { onAsk, onStart, onProgress } = {}) {
    if (this.busy) throw new Problem('busy-here');
    this.busy = true;
    try {
      this.sendJson({ t: 'send', files: files.map((f) => ({ name: f.name || 'file', size: f.size })) });
      if (!this.auto) {
        this.waitingForUser = true;
        if (onAsk) onAsk();
      }
      const first = await this.expect((m) => m.t === 'ok' || m.t === 'no', this.auto ? 30_000 : 120_000, 'ask-timeout');
      this.waitingForUser = false;
      this.lastActivity = Date.now();
      if (first.t === 'no') throw new Problem(`no-${first.why}`);
      if (onStart) onStart();
      const end = this.expect((m) => m.t === 'saved' || m.t === 'failed', 0);
      end.catch(() => {});
      this.onprogress = (n) => {
        if (onProgress) onProgress(n);
      };
      this.streaming = true;
      this.stopStream = null;
      for (const f of files) await this.stream(f);
      this.streaming = false;
      this.finishing = true;
      const res = await end;
      if (res.t === 'failed') throw new Problem(`failed-${res.why}`);
      return Number.isSafeInteger(res.files) && res.files > 0 ? res.files : files.length;
    } catch (e) {
      // After a send went wrong part way, the byte stream is in an unknown
      // state: start the next send on a fresh connection.
      if (this.streaming || this.finishing) this.close('failed');
      throw e;
    } finally {
      this.busy = false;
      this.waitingForUser = false;
      this.streaming = false;
      this.finishing = false;
      this.stopStream = null;
      this.onprogress = null;
    }
  }

  // Pulls one Shelf file onto the phone. Resolves with a File.
  async get(item, onProgress) {
    if (this.busy) throw new Problem('busy-here');
    this.busy = true;
    try {
      const bytes = new Promise((resolve, reject) => {
        this.pendingGet = { id: item.id, resolve, reject, onProgress };
      });
      bytes.catch(() => {});
      this.sendJson({ t: 'get', id: item.id });
      const head = await this.expect((m) => (m.t === 'file' || m.t === 'nofile') && m.id === item.id, 20_000, 'no-answer');
      if (head.t === 'nofile') throw new Problem('nofile');
      const parts = await bytes;
      const name = safeFileName(head.name, item.name);
      return new File(parts, name, { type: mimeOf(name) });
    } finally {
      this.busy = false;
      this.pendingGet = null;
      this.incoming = null;
    }
  }

  // Which way the bytes go, from the selected ICE candidate pair.
  async road() {
    try {
      const stats = await this.pc.getStats();
      const byId = new Map();
      let pairId = null;
      stats.forEach((s) => {
        byId.set(s.id, s);
        if (s.type === 'transport' && s.selectedCandidatePairId) pairId = s.selectedCandidatePairId;
      });
      let pair = pairId ? byId.get(pairId) : null;
      if (!pair) {
        stats.forEach((s) => {
          if (!pair && s.type === 'candidate-pair' && (s.selected || (s.nominated && s.state === 'succeeded'))) pair = s;
        });
      }
      if (!pair) return 'connected';
      const local = byId.get(pair.localCandidateId) || {};
      const remote = byId.get(pair.remoteCandidateId) || {};
      // candidate types only (no addresses), for support questions
      this.path = `${local.candidateType || '?'}>${remote.candidateType || '?'}`;
      if (local.candidateType === 'relay' || remote.candidateType === 'relay') return 'relay';
      if (local.candidateType === 'host') return 'wifi';
      if (local.candidateType === 'srflx' || local.candidateType === 'prflx') return 'internet';
      return 'connected';
    } catch {
      return 'connected';
    }
  }

  // Ends everything at once; pending promises reject with `reason`.
  shutdown(reason) {
    if (this.closed) return;
    const wasReady = this.ready;
    this.closed = true;
    this.closeReason = reason;
    this.ready = false;
    this.abort.abort();
    clearInterval(this.watch);
    const err = new Problem(reason);
    for (const w of this.waiters.splice(0)) {
      clearTimeout(w.timer);
      w.reject(err);
    }
    if (this.pendingGet) this.pendingGet.reject(err);
    this.incoming = null;
    const { dc, pc } = this;
    // let a last message (bye) leave before the connection goes
    setTimeout(() => {
      try {
        if (dc) dc.close();
      } catch {
        // gone
      }
      try {
        if (pc) pc.close();
      } catch {
        // gone
      }
    }, 250);
    this.onclose(reason, wasReady);
  }

  // Polite close: {"t":"bye"} first.
  close(reason = 'bye') {
    if (this.closed) return;
    try {
      if (this.dc && this.dc.readyState === 'open') this.dc.send(JSON.stringify({ t: 'bye' }));
    } catch {
      // closing anyway
    }
    this.shutdown(reason);
  }
}

// Hands a received file to the phone: the share sheet when it takes files,
// else a download link. `button` becomes the way to retry with a fresh tap.
async function deliver(file, button) {
  let canShare = false;
  try {
    canShare = Boolean(navigator.share && navigator.canShare && navigator.canShare({ files: [file] }));
  } catch {
    canShare = false;
  }
  if (canShare) {
    const share = () => navigator.share({ files: [file] });
    try {
      await share();
      return 'shared';
    } catch (e) {
      if (e && e.name === 'AbortError') return 'cancelled';
      // the tap that asked for the file has expired: one more tap opens the share sheet
      const again = el('button', 'save ready', 'Share');
      again.type = 'button';
      again.addEventListener('click', () => {
        share().catch(() => {});
      });
      button.replaceWith(again);
      return 'tap';
    }
  }
  const url = URL.createObjectURL(file);
  const a = el('a', 'save ready', 'Download');
  a.href = url;
  a.download = file.name;
  a.rel = 'noopener';
  a.setAttribute('aria-label', `Download ${file.name}`);
  button.replaceWith(a);
  setTimeout(() => URL.revokeObjectURL(url), 10 * 60_000);
  return 'link';
}

// ---------------------------------------------------------------- plain sentences

function problemText(code, name, during = 'connect') {
  switch (code) {
    case 'offline': return `${name} is offline. Start Oyenza Studio on it, then try again.`;
    case 'no-answer': return `${name} didn't answer. Check that Oyenza Studio is running on it, then try again.`;
    case 'busy': return `${name} is busy with other phones. Try again in a moment.`;
    case 'no-route': return `Couldn't reach ${name} from this network. Try again, or join the same Wi-Fi as the PC.`;
    case 'no-webrtc': return 'This browser can\'t make direct connections. Open this page in Safari or Chrome.';
    case 'gone': return `${name} removed this phone. To use it again, pair on the PC: Shelf › Phone › Pair a phone.`;
    case 'bad': return 'This pairing stopped working. Pair again on the PC: Shelf › Phone › Pair a phone.';
    case 'network': return 'Can\'t reach the internet. Check your connection and try again.';
    case 'slow-down': return 'Too many tries at once. Wait a minute and try again.';
    case 'no-busy': return `${name} is taking files from another phone. Try again in a moment.`;
    case 'no-too-many': return 'That\'s more than 500 files. Send fewer at once.';
    case 'no-too-large': return 'That\'s too much for one send. Send fewer files at once.';
    case 'no-declined': return `Declined on ${name}.`;
    case 'ask-timeout': return `No answer from ${name}. Send again to ask once more.`;
    case 'failed-size': return 'A file changed while it was sent. Nothing was kept. Try again.';
    case 'failed-disk': return `${name} is out of disk space. Nothing was kept.`;
    case 'failed-stopped': return `Stopped on ${name}. Nothing was kept.`;
    case 'read': return 'A file couldn\'t be read on this phone. Nothing was kept.';
    case 'nofile': return 'That file isn\'t on the Shelf any more.';
    case 'too-big': return 'That file is too big to save on this phone.';
    default:
      if (during === 'send') return `The connection to ${name} dropped. Nothing was kept. Try again.`;
      if (during === 'save') return `The connection to ${name} dropped. Try again.`;
      return `Couldn't connect to ${name}. Try again.`;
  }
}

// ---------------------------------------------------------------- views

let view = null;

function showView(name) {
  if (view === 'pc' && name !== 'pc') closePcLink();
  view = name;
  for (const v of ['pair', 'home', 'pc', 'share']) $(`v-${v}`).hidden = v !== name;
  document.body.dataset.view = name;
  if (name !== 'pc') {
    islandRest = () => setIsland('idle');
    islandRest();
  }
  document.title = name === 'pc' && pcv.pair ? `${PcName(pcv.pair)} · Oyenza Studio` : 'Oyenza Studio';
  if (name === 'home' || name === 'share') startPolling();
  else stopPolling();
  scrollTo(0, 0);
}

// ----- PC status (home and share)

const statusByK = new Map(); // k -> { state, seen }
let statusTimer = null;

async function refreshStatus() {
  store = readStore();
  const pairs = store.pairs;
  if (!pairs.length) return;
  try {
    for (let i = 0; i < pairs.length; i += 32) {
      const chunk = pairs.slice(i, i + 32);
      const keys = await Promise.all(chunk.map((p) => keysFor(p.k)));
      const r = await api('/api/phone/status', { rooms: keys.map((k) => ({ room: k.room, proof: k.proof })) });
      const byRoom = new Map((Array.isArray(r.rooms) ? r.rooms : []).map((x) => [x.room, x]));
      chunk.forEach((p, j) => {
        const x = byRoom.get(keys[j].room);
        if (x) statusByK.set(p.k, { state: x.state, seen: Number.isFinite(x.seen) ? x.seen : null });
      });
    }
  } catch {
    // keep what we knew; the next round tries again
  }
  if (view === 'home') renderHome();
  if (view === 'share') renderSharePcs();
}

function startPolling() {
  stopPolling();
  refreshStatus();
  statusTimer = setInterval(() => {
    if (document.visibilityState === 'visible') refreshStatus();
  }, STATUS_EVERY_MS);
}

function stopPolling() {
  clearInterval(statusTimer);
  statusTimer = null;
}

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible' && (view === 'home' || view === 'share')) refreshStatus();
});

// ----- 1. pairing

let pairing = null; // { k, name }

function renderKeep() {
  if (view !== 'pair' || !pairing || !pairing.done) return;
  const standalone = isStandalone();
  $('keep').hidden = standalone;
  const ios = IS_IOS;
  $('keep-ios').hidden = !ios;
  $('copy-link').hidden = !ios;
  $('keep-ios-note').hidden = !ios;
  $('install').hidden = ios || !installPrompt || installed;
  $('keep-menu').hidden = ios || Boolean(installPrompt) || installed;
  $('keep-android-note').hidden = ios || installed;
  $('keep-installed').hidden = !installed;
  $('keep-h').textContent = IS_IPAD ? 'Keep it on this iPad' : 'Keep it on this phone';
}

function setPairState(state, detail) {
  const title = $('pair-title');
  const sub = $('pair-sub');
  $('keep').hidden = true;
  $('pair-done').hidden = true;
  $('pair-retry').hidden = true;
  $('pair-home').hidden = true;
  $('v-pair').dataset.state = state;
  if (state === 'working') {
    pose('pair-oyen', 'peek');
    setIsland('wait', { title: 'Pairing…' });
    title.textContent = 'Pairing with your PC…';
    sub.textContent = 'This takes a few seconds.';
    sub.hidden = false;
    setStatus('pair', 'Waiting for your PC to answer.', 'wait');
  } else if (state === 'done') {
    pose('pair-oyen', 'sit', 'hop');
    islandRest = () => setIsland('idle');
    setIsland('idle');
    setIsland('done', { title: 'Paired', sub: detail, hold: 3200 });
    titleWithName('pair-title', 'Paired with', detail);
    sub.textContent = `Send photos and files to ${detail} from anywhere, whenever Oyenza Studio is running on it.`;
    sub.hidden = false;
    setStatus('pair', '');
    $('pair-done').hidden = false;
    renderKeep();
  } else {
    pose('pair-oyen', 'asleep');
    setIsland('idle');
    setIsland('oops', { title: 'Couldn\'t pair', hold: 3600 });
    title.textContent = 'Couldn\'t pair';
    sub.hidden = true;
    const text = {
      offline: 'Your PC didn\'t answer. Keep Shelf › Phone › Pair a phone open on the PC and scan again.',
      gone: 'This code was removed on the PC. Make a new one in Shelf › Phone › Pair a phone, and scan it.',
      bad: 'This code doesn\'t work any more. Make a new one in Shelf › Phone › Pair a phone, and scan it.',
      link: 'This pairing link is incomplete. Scan the code on your PC again.',
      network: 'Can\'t reach the internet. Check your connection and try again.',
      'slow-down': 'Too many tries at once. Wait a minute and try again.',
    }[detail] || 'Your PC didn\'t answer. Keep Shelf › Phone › Pair a phone open on the PC and scan again.';
    setStatus('pair', text, ['offline', 'network', 'slow-down'].includes(detail) ? 'nap' : 'bad');
    $('pair-retry').hidden = detail === 'link' || detail === 'gone' || detail === 'bad';
    store = readStore();
    $('pair-home').hidden = !store.pairs.length;
  }
}

async function runPairing(k) {
  pairing = { k, done: false };
  showView('pair');
  setPairState('working');
  if (!secretOk(k)) return setPairState('failed', 'link');
  try {
    const keys = await keysFor(k);
    const sid = randomSid();
    await phoneSend(keys, { t: 'pair', sid, name: phoneName(), sdp: '' }, PAIR_RETRY_MS);
    const welcome = await waitReply(keys, sid, ['welcome'], ANSWER_MS);
    if (!welcome) throw new Problem('offline');
    const name = cleanName(welcome.name) || 'Your PC';
    savePair(k, name, { fresh: true });
    pairing = { k, name, done: true };
    // Keep the secret (and the PC's name) in the address: on iOS, Add to Home Screen saves this address.
    history.replaceState(null, '', `/p#k=${k}&n=${encodeURIComponent(name)}`);
    setPairState('done', name);
  } catch (e) {
    setPairState('failed', e.code || 'offline');
  }
}

// A pairing link that already carries the PC's name was made after a pairing:
// keep it without asking the PC again.
function importPair(k, name) {
  if (!secretOk(k)) return null;
  return savePair(k, name || '', { keepName: true });
}

$('pair-done').addEventListener('click', () => {
  // On iOS Safari the address keeps #k= so Add to Home Screen still works from the list.
  if (!IS_IOS || isStandalone()) history.replaceState(null, '', '/');
  goHome();
});
$('pair-retry').addEventListener('click', () => {
  if (pairing) runPairing(pairing.k);
});
$('pair-home').addEventListener('click', () => {
  history.replaceState(null, '', '/');
  goHome();
});
$('install').addEventListener('click', async () => {
  if (!installPrompt) return;
  const p = installPrompt;
  installPrompt = null;
  try {
    await p.prompt();
    const choice = await p.userChoice;
    if (choice && choice.outcome === 'accepted') installed = true;
  } catch {
    // dismissed
  }
  renderKeep();
});
$('copy-link').addEventListener('click', async () => {
  const link = location.href;
  try {
    await navigator.clipboard.writeText(link);
    $('copy-link-text').textContent = 'Link copied';
    setTimeout(() => {
      $('copy-link-text').textContent = 'Copy pairing link';
    }, 2500);
  } catch {
    const box = $('link-text');
    box.value = link;
    box.hidden = false;
    box.focus();
    box.select();
  }
});

// ----- 2. home: your PCs

let homeNote = '';

function goHome(note = '') {
  homeNote = note;
  closeShareLink();
  showView('home');
  renderHome();
}

function stateLine(st) {
  const line = el('span', 'state');
  const dot = el('span', 'dot');
  line.appendChild(dot);
  if (!st) {
    line.appendChild(el('span', '', 'Checking…'));
  } else if (st.state === 'online') {
    dot.classList.add('on');
    line.appendChild(el('span', '', 'Online'));
  } else if (st.state === 'gone' || st.state === 'bad') {
    dot.classList.add('gone');
    line.classList.add('gone');
    line.appendChild(el('span', '', st.state === 'gone' ? 'Removed from this PC' : 'This pairing stopped working'));
  } else {
    line.appendChild(el('span', '', st.seen ? `Offline · seen ${ago(st.seen)}` : 'Offline'));
  }
  return line;
}

// A PC is drawn as its island, lit when it's online.
function pcIcon(st) {
  const i = el('span', `pc-icon${st && st.state === 'online' ? ' on' : ''}`);
  const screen = el('span', 'mini-screen');
  const isl = el('span', 'mini-island');
  isl.appendChild(el('i'));
  screen.appendChild(isl);
  i.append(screen, el('span', 'mini-stand'));
  return i;
}

function renderHome() {
  store = readStore();
  const list = $('pcs');
  list.textContent = '';
  const any = store.pairs.length > 0;
  pose('home-oyen', any ? 'peek' : 'note');
  $('home-title').textContent = any ? 'Your PCs' : 'Let\'s pair your PC';
  $('home-sub').textContent = any ? 'Tap one to send photos and files to its Shelf.' : 'Pair once, then send to it from anywhere.';
  $('pcs-card').hidden = !any;
  $('empty').hidden = any;
  $('home-foot').hidden = !any;
  $('paste-card').hidden = !(IS_IOS && isStandalone());
  $('home-note').hidden = !homeNote;
  $('home-note').textContent = homeNote;
  for (const p of store.pairs) {
    const st = statusByK.get(p.k);
    const li = el('li');
    const text = el('span', 'pc-text');
    text.appendChild(el('b', '', PcName(p)));
    text.appendChild(stateLine(st));
    if (st && (st.state === 'gone' || st.state === 'bad')) {
      const row = el('div', 'pc-row');
      const forget = el('button', 'save', 'Forget');
      forget.type = 'button';
      forget.setAttribute('aria-label', `Forget ${PcName(p)}`);
      forget.addEventListener('click', () => {
        forgetPair(p.k);
        renderHome();
      });
      row.append(pcIcon(st), text, forget);
      li.appendChild(row);
    } else {
      const b = el('button', 'pc-open');
      b.type = 'button';
      const chev = el('span', 'chev');
      chev.appendChild(svg('chev'));
      b.append(pcIcon(st), text, chev);
      b.addEventListener('click', () => openPc(p.k));
      li.appendChild(b);
    }
    list.appendChild(li);
  }
  const input = $('phone-name');
  if (document.activeElement !== input) input.value = store.phoneName;
  input.placeholder = defaultName;
}

function savePhoneName() {
  const input = $('phone-name');
  const clean = cleanName(input.value);
  store = readStore();
  if (clean === store.phoneName) return;
  store.phoneName = clean;
  writeStore();
  input.value = clean;
  $('phone-name-note').textContent = clean ? 'Saved. Your PCs show this name.' : `Saved. Your PCs show "${defaultName}".`;
}

$('phone-name').addEventListener('change', savePhoneName);
$('phone-name').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    e.preventDefault();
    $('phone-name').blur();
  }
});

$('paste-card').addEventListener('submit', (e) => {
  e.preventDefault();
  const text = $('paste').value;
  const m = /[#&]k=([A-Za-z0-9_-]{22})(?:&n=([^&\s]*))?/.exec(text);
  const note = $('paste-note');
  note.hidden = false;
  if (!m || !secretOk(m[1])) {
    note.textContent = 'That isn\'t a pairing link. Copy it again on the paired phone.';
    return;
  }
  let name = '';
  try {
    name = m[2] ? decodeURIComponent(m[2]) : '';
  } catch {
    name = '';
  }
  importPair(m[1], name);
  $('paste').value = '';
  note.textContent = 'Added.';
  renderHome();
  refreshStatus();
});

// ----- 3. one PC

const pcv = { k: null, pair: null, link: null, connecting: null, busy: false, idle: null, chosen: [], shelf: null, session: 0 };

// True when an error belongs to what is on screen now (not to a view the user left).
const current = (session, e) => session === pcv.session && view === 'pc' && e.code !== 'cancelled' && e.code !== 'aborted';

function openPc(k, push = true) {
  store = readStore();
  const p = findPair(k);
  if (!p) return goHome();
  if (push) history.pushState({ pc: k }, '');
  closePcLink();
  Object.assign(pcv, { k, pair: p, chosen: [], shelf: null, busy: false });
  pcv.session += 1;
  $('pick').value = '';
  showView('pc');
  titleWithName('pc-title', 'Send to', PcName(p));
  $('pc-error').hidden = true;
  $('pc-retry').hidden = true;
  setStatus('send', '');
  setStatus('save', '');
  hideProgress('send');
  armForget(false);
  renderChosen();
  renderShelf();
  connectPc();
}

function setRoad(kind) {
  const ROADS = {
    connecting: [null, 'Connecting…', true],
    wifi: ['wifi', 'Straight over your Wi-Fi'],
    internet: ['globe', 'Straight across the internet'],
    relay: ['lock', 'Through the relay, encrypted'],
    connected: ['check', 'Connected'],
    paused: ['pause', 'Not connected. It connects again when you send or save.', true],
    off: ['off', 'Not connected', true],
  };
  const [icon, text, quiet] = ROADS[kind] || ROADS.off;
  const who = pcv.pair ? PcName(pcv.pair) : 'Your PC';
  const SHORT = { wifi: 'Over your Wi-Fi', internet: 'Across the internet', relay: 'Through the relay', connected: 'Connected' };
  islandRest = () => {
    if (kind === 'connecting') setIsland('wait', { title: who, sub: 'Connecting…' });
    else if (SHORT[kind]) setIsland('link', { title: who, sub: SHORT[kind], icon: icon === 'check' ? 'wifi' : icon, live: true });
    else setIsland('off', { title: who, sub: kind === 'paused' ? 'Resting' : 'Not connected' });
  };
  if (view === 'pc' && $('progress').hidden) islandRest();
  if (SHORT[kind]) pose('pc-oyen', 'shelf');
  const box = $('road-icon');
  box.textContent = '';
  box.appendChild(icon ? svg(icon) : el('span', 'spin'));
  $('road-text').textContent = text;
  $('road').className = `road${quiet ? ' quiet' : ''}`;
  $('road').dataset.road = kind;
}

function armIdle() {
  clearTimeout(pcv.idle);
  pcv.idle = setTimeout(() => {
    if (pcv.busy) return armIdle();
    if (pcv.link) {
      pcv.link.close('idle');
      pcv.link = null;
      if (view === 'pc') setRoad('paused');
    }
  }, IDLE_MS);
}

function closePcLink() {
  clearTimeout(pcv.idle);
  if (pcv.link) pcv.link.close('bye');
  pcv.link = null;
  pcv.connecting = null;
}

function ensureLink() {
  if (pcv.link && pcv.link.ready && !pcv.link.closed) return Promise.resolve(pcv.link);
  if (pcv.connecting) return pcv.connecting;
  const p = pcv.pair;
  const l = new Link(p);
  pcv.link = l;
  l.onshelf = (files) => {
    if (pcv.link === l) {
      pcv.shelf = files;
      renderShelf();
    }
  };
  l.onname = (name) => {
    renamePair(p.k, name);
    p.name = cleanName(name);
    if (pcv.link === l && view === 'pc') titleWithName('pc-title', 'Send to', PcName(p));
  };
  l.onclose = (why, wasReady) => {
    if (pcv.link !== l) return;
    pcv.link = null;
    if (view === 'pc' && wasReady && why !== 'bye' && why !== 'idle') setRoad('paused');
  };
  setRoad('connecting');
  const attempt = (async () => {
    try {
      await l.open();
      if (pcv.link === l) {
        setRoad(await l.road());
        if (l.path) $('road').dataset.path = l.path;
        $('pc-error').hidden = true;
        $('pc-retry').hidden = true;
        armIdle();
      }
      return l;
    } catch (e) {
      l.close('failed');
      if (pcv.link === l) pcv.link = null;
      throw e;
    } finally {
      if (pcv.connecting === attempt) pcv.connecting = null;
    }
  })();
  pcv.connecting = attempt;
  return attempt;
}

function showPcError(code) {
  const p = pcv.pair;
  if (code === 'gone' || code === 'bad') statusByK.set(p.k, { state: code, seen: null });
  setRoad('off');
  if (['offline', 'no-answer', 'no-route', 'network'].includes(code)) pose('pc-oyen', 'asleep');
  $('pc-error').hidden = false;
  $('pc-error').className = `status ${['offline', 'no-answer', 'no-route', 'network', 'busy'].includes(code) ? 'nap' : 'bad'}`;
  $('pc-error-text').textContent = problemText(code, PcName(p));
  $('pc-retry').hidden = code === 'gone' || code === 'bad';
  if (pcv.shelf === null) renderShelf('Not connected.');
}

async function connectPc() {
  const session = pcv.session;
  $('pc-error').hidden = true;
  $('pc-retry').hidden = true;
  try {
    await ensureLink();
  } catch (e) {
    if (current(session, e)) showPcError(e.code);
  }
}

function renderChosen() {
  const list = $('chosen');
  list.textContent = '';
  let total = 0;
  for (const f of pcv.chosen) {
    total += f.size;
    list.appendChild(fileRow(f.name, f.size, undefined, f));
  }
  const any = pcv.chosen.length > 0;
  list.hidden = !any;
  $('chosen-total').hidden = !any;
  $('send-row').hidden = !any || pcv.busy;
  $('pick-label').hidden = any || pcv.busy;
  $('chosen-count').textContent = describe(pcv.chosen);
  $('chosen-size').textContent = fmt(total);
}

function renderShelf(emptyText) {
  const list = $('shelf');
  list.textContent = '';
  if (pcv.shelf === null) {
    list.appendChild(el('li', 'empty', emptyText || 'Connecting…'));
    return;
  }
  if (!pcv.shelf.length) {
    list.appendChild(el('li', 'empty', 'Nothing on the Shelf yet. Files you drop on the island show up here.'));
    return;
  }
  for (const item of pcv.shelf) {
    const b = el('button', 'save', 'Save');
    b.type = 'button';
    b.setAttribute('aria-label', `Save ${item.name}`);
    b.addEventListener('click', () => saveShelfFile(item, b));
    list.appendChild(fileRow(item.name, item.size, b));
  }
}

async function sendChosen() {
  if (pcv.busy || !pcv.chosen.length) return;
  const files = pcv.chosen.slice();
  const total = files.reduce((s, f) => s + f.size, 0);
  if (files.length > MAX_FILES) return setStatus('send', problemText('no-too-many'), 'bad');
  if (total > MAX_TOTAL) return setStatus('send', problemText('no-too-large'), 'bad');
  const session = pcv.session;
  pcv.busy = true;
  renderChosen();
  setStatus('send', pcv.link && pcv.link.ready ? '' : `Connecting to ${pcName(pcv.pair)}…`, 'wait');
  holdWake();
  try {
    const l = await ensureLink();
    const name = PcName(pcv.pair);
    const n = await l.send(files, {
      onAsk: () => setStatus('send', `Waiting for ${name} to accept. Look at the island on the PC.`, 'wait'),
      onStart: () => {
        setStatus('send', '');
        showProgress('send', 0, total, files);
      },
      onProgress: (done) => showProgress('send', done, total, files),
    });
    hideProgress('send');
    pcv.chosen = [];
    $('pick').value = '';
    setStatus('send', `Sent ${describe(files, n)} to ${name}`, 'ok');
  } catch (e) {
    hideProgress('send');
    if (current(session, e)) {
      if (CONNECT_PROBLEMS.includes(e.code)) {
        setStatus('send', '');
        showPcError(e.code);
      } else {
        setStatus('send', problemText(e.code, PcName(pcv.pair), 'send'), 'bad');
      }
    }
  } finally {
    pcv.busy = false;
    renderChosen();
    releaseWake();
    armIdle();
  }
}

// Problems that mean "no connection to this PC" (shown once, at the top of the PC view).
const CONNECT_PROBLEMS = ['offline', 'no-answer', 'busy', 'no-route', 'no-webrtc', 'gone', 'bad', 'network', 'slow-down'];

async function saveShelfFile(item, button) {
  if (pcv.busy) return;
  const session = pcv.session;
  pcv.busy = true;
  button.disabled = true;
  button.textContent = '…';
  setStatus('save', '');
  holdWake();
  let done = false;
  try {
    const l = await ensureLink();
    const file = await l.get(item, (got, size) => {
      button.textContent = `${Math.floor((got * 100) / Math.max(size, 1))}%`;
    });
    done = true;
    const how = await deliver(file, button);
    if (how === 'link') setStatus('save', `${file.name} is ready. Tap Download to save it.`, 'info');
    else if (how === 'tap') setStatus('save', `${file.name} is ready. Tap Share to save it.`, 'info');
  } catch (e) {
    if (current(session, e)) {
      if (CONNECT_PROBLEMS.includes(e.code)) showPcError(e.code);
      else setStatus('save', problemText(e.code, PcName(pcv.pair), 'save'), 'bad');
    }
  } finally {
    if (!done || button.isConnected) {
      button.disabled = false;
      button.textContent = 'Save';
    }
    pcv.busy = false;
    releaseWake();
    armIdle();
  }
}

let forgetArmed = null;
function armForget(on) {
  clearTimeout(forgetArmed);
  forgetArmed = null;
  $('forget').classList.toggle('armed', on);
  $('forget').textContent = on ? 'Tap again to forget this PC' : 'Forget this PC';
  if (on) forgetArmed = setTimeout(() => armForget(false), 4000);
}

$('forget').addEventListener('click', () => {
  if (!forgetArmed) return armForget(true);
  armForget(false);
  const k = pcv.k;
  closePcLink();
  forgetPair(k);
  history.replaceState(null, '', location.href);
  goHome();
});

$('pick').addEventListener('change', () => {
  pcv.chosen = Array.from($('pick').files || []);
  setStatus('send', '');
  renderChosen();
  if (pcv.chosen.length && !pcv.link && !pcv.connecting && view === 'pc') connectPc();
});
$('clear').addEventListener('click', () => {
  pcv.chosen = [];
  $('pick').value = '';
  setStatus('send', '');
  renderChosen();
});
$('send').addEventListener('click', sendChosen);
$('pc-retry').addEventListener('click', connectPc);
$('pc-back').addEventListener('click', () => {
  if (history.state && history.state.pc) history.back();
  else goHome();
});
$('v-pc').addEventListener('pointerdown', () => {
  if (pcv.link) armIdle();
});

addEventListener('popstate', (e) => {
  const s = e.state;
  if (s && s.pc && findPair(s.pc)) openPc(s.pc, false);
  else if (view === 'pc') goHome();
});
addEventListener('pagehide', () => {
  closePcLink();
  closeShareLink();
});

// ----- 4. share sheet (Android)

const sv = { id: null, files: [], busy: false, sent: false, link: null };

function openDb() {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open('oyen-drop', 1);
    r.onupgradeneeded = () => {
      if (!r.result.objectStoreNames.contains('shared')) r.result.createObjectStore('shared');
    };
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

async function sharedOp(mode, fn) {
  const db = await openDb();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction('shared', mode);
      const req = fn(tx.objectStore('shared'));
      tx.oncomplete = () => resolve(req ? req.result : undefined);
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}

const readShared = (id) => sharedOp('readonly', (s) => s.get(id));
const deleteShared = (id) => sharedOp('readwrite', (s) => s.delete(id));

async function openShare(id) {
  Object.assign(sv, { id, files: [], busy: false, sent: false, link: null });
  showView('share');
  $('share-done').hidden = true;
  $('share-progress').hidden = true;
  setStatus('share', '');
  let rec = null;
  if (/^[A-Za-z0-9_-]{8,64}$/.test(id)) {
    try {
      rec = await readShared(id);
    } catch {
      rec = null;
    }
  }
  const files = rec && Array.isArray(rec.files) ? rec.files.filter((f) => f && typeof f.size === 'number') : [];
  if (rec && !files.length) {
    // a shared link or text becomes a small text file
    const lines = [rec.title, rec.text, rec.url].filter((x) => typeof x === 'string' && x.trim());
    if (lines.length) files.push(new File([`${lines.join('\n')}\n`], rec.url ? 'Shared link.txt' : 'Shared text.txt', { type: 'text/plain' }));
  }
  if (!files.length) {
    $('share-title').textContent = 'Nothing to send';
    $('share-sub').textContent = 'Those files aren\'t here any more. Share them again.';
    $('share-sub').hidden = false;
    $('share-files-card').hidden = true;
    $('share-pcs-card').hidden = true;
    $('share-done').hidden = false;
    history.replaceState(null, '', '/');
    return;
  }
  sv.files = files;
  $('share-files-card').hidden = false;
  $('share-pcs-card').hidden = false;
  $('share-title').textContent = `Send ${describe(files)}`;
  $('share-sub').textContent = 'Choose the PC to send them to.';
  $('share-sub').hidden = false;
  const list = $('share-files');
  list.textContent = '';
  let total = 0;
  for (const f of files) {
    total += f.size;
    list.appendChild(fileRow(f.name || 'file', f.size, undefined, f));
  }
  $('share-count').textContent = describe(files);
  $('share-size').textContent = fmt(total);
  renderSharePcs();
}

function renderSharePcs() {
  if (view !== 'share' || !sv.files.length) return;
  store = readStore();
  const box = $('share-pcs');
  box.textContent = '';
  if (!store.pairs.length) {
    box.appendChild(el('p', 'small', 'No PC paired yet. On your PC, open Oyenza Studio › Shelf › Phone › Pair a phone, and scan the code.'));
    return;
  }
  for (const p of store.pairs) {
    const st = statusByK.get(p.k);
    const online = st && st.state === 'online';
    const b = el('button', `btn ${online ? 'primary' : 'quiet'}`);
    b.type = 'button';
    b.disabled = !online || sv.busy || sv.sent;
    b.appendChild(el('span', '', PcName(p)));
    const note = !st ? 'Checking…' : online ? 'Online' : st.state === 'gone' ? 'Removed' : st.state === 'bad' ? 'Not working' : 'Offline';
    b.appendChild(el('span', 'btn-sub', note));
    b.addEventListener('click', () => shareTo(p));
    box.appendChild(b);
  }
}

async function shareTo(p) {
  if (sv.busy || sv.sent) return;
  sv.busy = true;
  renderSharePcs();
  const name = PcName(p);
  const files = sv.files;
  const total = files.reduce((s, f) => s + f.size, 0);
  setStatus('share', `Connecting to ${name}…`, 'wait');
  holdWake();
  const l = new Link(p);
  sv.link = l;
  l.onname = (n) => renamePair(p.k, n);
  try {
    await l.open();
    const n = await l.send(files, {
      onAsk: () => setStatus('share', `Waiting for ${name} to accept. Look at the island on the PC.`, 'wait'),
      onStart: () => {
        setStatus('share', '');
        showProgress('share', 0, total, files);
      },
      onProgress: (done) => showProgress('share', done, total, files),
    });
    sv.sent = true;
    hideProgress('share');
    try {
      await deleteShared(sv.id);
    } catch {
      // dropped after a day anyway
    }
    setStatus('share', `Sent ${describe(files, n)} to ${PcName(findPair(p.k) || p)}`, 'ok');
    $('share-sub').hidden = true;
    $('share-pcs-card').hidden = true;
    $('share-done').hidden = false;
    history.replaceState(null, '', '/');
  } catch (e) {
    hideProgress('share');
    if (e.code !== 'cancelled') setStatus('share', problemText(e.code, name, l.ready ? 'send' : 'connect'), 'bad');
  } finally {
    l.close('bye');
    if (sv.link === l) sv.link = null;
    sv.busy = false;
    renderSharePcs();
    releaseWake();
  }
}

function closeShareLink() {
  if (sv.link) sv.link.close('bye');
  sv.link = null;
}

$('share-done').addEventListener('click', () => {
  history.replaceState(null, '', '/');
  goHome();
});

// ---------------------------------------------------------------- start

async function start() {
  await Promise.race([loadDefaultName(), sleep(800)]);
  const frag = new URLSearchParams(location.hash.slice(1));
  const query = new URLSearchParams(location.search);
  const k = frag.get('k');
  if (k !== null) {
    const n = frag.get('n');
    if (IS_IOS && isStandalone()) {
      // The Home Screen app starts at the address saved in Safari: take its pairing in.
      importPair(k, n);
      history.replaceState(null, '', '/');
      return goHome();
    }
    if (n !== null && secretOk(k)) {
      if (findPair(k)) {
        // already kept here (iOS Safari keeps this address after pairing)
        if (!IS_IOS) history.replaceState(null, '', '/');
        return goHome();
      }
      const p = importPair(k, n);
      pairing = { k, name: PcName(p), done: true };
      showView('pair');
      setPairState('done', PcName(p));
      return;
    }
    if (isStandalone() && findPair(k)) {
      history.replaceState(null, '', '/');
      return goHome();
    }
    return runPairing(k);
  }
  const share = query.get('share');
  if (share) return openShare(share);
  const lost = query.get('shared') === 'lost';
  if (query.has('shared')) history.replaceState(null, '', '/');
  goHome(lost ? 'The shared files didn\'t come through. Share them again now.' : '');
}

// Another pairing link opened in this tab changes only the fragment.
addEventListener('hashchange', () => {
  if (new URLSearchParams(location.hash.slice(1)).get('k') !== null) start();
});

start();

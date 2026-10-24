// Oyen Poni paired phone drop: the shared rules of docs/paired-drop.md.
//
// One plain ES module, WebCrypto only, used by the phone page (app.js), the
// PC emulator in tools/pairdrop-e2e and the Node tests (Node 20+ has the same
// globalThis.crypto.subtle, btoa and atob).

const enc = new TextEncoder();

function subtle() {
  const s = globalThis.crypto && globalThis.crypto.subtle;
  if (!s) throw new Error('This browser has no WebCrypto here (the page needs HTTPS).');
  return s;
}

export const PROTO = 'oyen-drop/1';
export const NAME_MAX = 60; // characters (code points)
export const SDP_MAX = 32 * 1024; // bytes of UTF-8
export const TS_WINDOW_MS = 10 * 60 * 1000; // a message more than 10 minutes off is dropped
export const REPLAY_MS = 15 * 60 * 1000; // (sid, t, ts) is remembered this long

// Data channel
export const FRAME = 16 * 1024; // bytes per binary frame
export const HIGH_WATER = 4 * 1024 * 1024; // pause sending above this bufferedAmount
export const LOW_WATER = 1024 * 1024; // bufferedAmountLowThreshold
export const MAX_FILES = 500;
export const MAX_TOTAL = 64e9; // "at most 64 GB together" (decimal GB, the stricter reading)

// ---------- base64url without padding ----------

const B64U_RE = /^[A-Za-z0-9_-]*$/;

export function b64uEncode(bytes) {
  let s = '';
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

// Returns a Uint8Array, or null for anything that is not canonical b64u.
export function b64uDecode(text) {
  if (typeof text !== 'string' || !B64U_RE.test(text) || text.length % 4 === 1) return null;
  let bin;
  try {
    bin = atob(text.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((text.length + 3) % 4));
  } catch {
    return null;
  }
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return b64uEncode(out) === text ? out : null;
}

// True when text is canonical b64u of exactly `bytes` bytes.
export function isB64u(text, bytes) {
  if (typeof text !== 'string' || text.length !== Math.ceil((bytes * 4) / 3)) return false;
  const d = b64uDecode(text);
  return d !== null && d.length === bytes;
}

export function randomB64u(bytes) {
  return b64uEncode(globalThis.crypto.getRandomValues(new Uint8Array(bytes)));
}

export const randomSid = () => randomB64u(16);

// ---------- names ----------

// "One line": no C0/C1 control characters (so no \r or \n), no DEL, no U+2028/U+2029.
const CONTROL_RE = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/;
const CONTROL_RUN_RE = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]+/g;

export function nameOk(name) {
  return typeof name === 'string' && !CONTROL_RE.test(name) && [...name].length <= NAME_MAX;
}

// Makes any text a valid device name: one line, trimmed, at most 60 characters.
export function cleanName(name) {
  if (typeof name !== 'string') return '';
  const one = name.replace(CONTROL_RUN_RE, ' ').replace(/\s+/g, ' ').trim();
  return [...one].slice(0, NAME_MAX).join('').trim();
}

// ---------- keys ----------

async function sha256(label, secret) {
  const l = enc.encode(label);
  const buf = new Uint8Array(l.length + secret.length);
  buf.set(l);
  buf.set(secret, l.length);
  return new Uint8Array(await subtle().digest('SHA-256', buf));
}

export function secretOk(secret) {
  return isB64u(secret, 16);
}

// secret: b64u text (22 chars) or 16 raw bytes. Returns {room, proof, K, key}.
export async function deriveKeys(secret) {
  const S = typeof secret === 'string' ? b64uDecode(secret) : secret;
  if (!S || S.length !== 16) throw new Error('The pairing secret must be 16 bytes.');
  const room = b64uEncode(await sha256(`${PROTO} room`, S));
  const proof = b64uEncode(await sha256(`${PROTO} proof`, S));
  const K = await sha256(`${PROTO} mac`, S);
  const key = await subtle().importKey('raw', K, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify']);
  return { room, proof, K, key };
}

// ---------- signed messages ----------

const T_RE = /^[a-z][a-z-]{0,15}$/;
const MSG_KEYS = ['t', 'sid', 'ts', 'name', 'sdp', 'mac'];

export function utf8Length(text) {
  return enc.encode(text).length;
}

// The shape every signalling message must have (the server checks the same).
export function msgShapeOk(m) {
  if (!m || typeof m !== 'object' || Array.isArray(m)) return false;
  const keys = Object.keys(m);
  if (keys.length !== MSG_KEYS.length || !MSG_KEYS.every((k) => Object.prototype.hasOwnProperty.call(m, k))) return false;
  return (
    typeof m.t === 'string' && T_RE.test(m.t) &&
    isB64u(m.sid, 16) &&
    Number.isSafeInteger(m.ts) && m.ts >= 0 &&
    nameOk(m.name) &&
    typeof m.sdp === 'string' && m.sdp.length <= SDP_MAX && utf8Length(m.sdp) <= SDP_MAX &&
    isB64u(m.mac, 32)
  );
}

// The text the MAC covers: seven lines joined with \n, the SDP verbatim.
export function macText(room, m) {
  return [PROTO, room, m.t, m.sid, String(m.ts), m.name, m.sdp].join('\n');
}

export async function signMsg(keys, { t, sid, ts = Date.now(), name = '', sdp = '' }) {
  const m = { t, sid, ts, name, sdp, mac: 'A'.repeat(42) + 'A' };
  if (!msgShapeOk(m)) throw new Error(`Message "${t}" does not fit the contract.`);
  const sig = await subtle().sign('HMAC', keys.key, enc.encode(macText(keys.room, m)));
  m.mac = b64uEncode(new Uint8Array(sig));
  return m;
}

// Checks shape, type, session, clock window, MAC and replay (in that order).
export async function verifyMsg(keys, m, { now = Date.now(), types = null, sid = null, replay = null } = {}) {
  if (!msgShapeOk(m)) return false;
  if (types && !types.includes(m.t)) return false;
  if (sid !== null && m.sid !== sid) return false;
  if (Math.abs(now - m.ts) > TS_WINDOW_MS) return false;
  const ok = await subtle().verify('HMAC', keys.key, b64uDecode(m.mac), enc.encode(macText(keys.room, m)));
  if (!ok) return false;
  if (replay && !replay.check(m, now)) return false;
  return true;
}

// Remembers (sid, t, ts) of accepted messages for 15 minutes.
export class ReplayCache {
  constructor(ms = REPLAY_MS) {
    this.ms = ms;
    this.seen = new Map();
  }

  check(m, now = Date.now()) {
    for (const [k, at] of this.seen) {
      if (now - at < this.ms) break;
      this.seen.delete(k);
    }
    const k = `${m.sid}\n${m.t}\n${m.ts}`;
    if (this.seen.has(k)) return false;
    this.seen.set(k, now);
    return true;
  }
}

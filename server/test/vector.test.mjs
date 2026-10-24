// The contract's test vector (docs/paired-drop.md), with node:crypto and with
// the browser module public/drop-core.js (WebCrypto), plus that module's rules.
import test from 'node:test';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import * as core from '../public/drop-core.js';
import { derive, sign as nodeSign } from './helpers.mjs';

const V = {
  S: Buffer.from('000102030405060708090a0b0c0d0e0f', 'hex'),
  b64u: 'AAECAwQFBgcICQoLDA0ODw',
  room: 'geDM0jEhk4eiOz_li-FtORW2YlPBbebr7NsHjLkHI_4',
  proof: '6JmvhsXX2_bzKYvHQ_iFgLcPnlG-m636-rWXAI5Vubo',
  K: 'PbxVzGOl5YKxg6Fd-uSD6Yqr9716j9MddvFbISuFxL4',
  msg: { t: 'offer', sid: 'AAECAwQFBgcICQoLDA0ODw', ts: 1790000000000, name: 'Pixel 10', sdp: 'v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\n' },
  mac: '3QYhMYuCNPzit2tYBw0ejDAQsE1vxoO4DQERkznV8o8',
};

test('test vector with node:crypto', () => {
  const k = derive(V.S);
  assert.equal(k.S, V.b64u);
  assert.equal(k.room, V.room);
  assert.equal(k.proof, V.proof);
  assert.equal(k.K.toString('base64url'), V.K);
  assert.equal(nodeSign(k, V.msg).mac, V.mac);
});

test('test vector with drop-core.js (WebCrypto)', async () => {
  assert.equal(core.b64uEncode(new Uint8Array(V.S)), V.b64u);
  for (const secret of [V.b64u, new Uint8Array(V.S)]) {
    const k = await core.deriveKeys(secret);
    assert.equal(k.room, V.room);
    assert.equal(k.proof, V.proof);
    assert.equal(core.b64uEncode(k.K), V.K);
    const m = await core.signMsg(k, V.msg);
    assert.equal(m.mac, V.mac);
    assert.deepEqual(Object.keys(m), ['t', 'sid', 'ts', 'name', 'sdp', 'mac']);
    assert.equal(await core.verifyMsg(k, m, { now: V.msg.ts }), true);
  }
  assert.equal(
    core.macText(V.room, V.msg),
    `oyen-drop/1\n${V.room}\noffer\nAAECAwQFBgcICQoLDA0ODw\n1790000000000\nPixel 10\nv=0\r\no=- 1 2 IN IP4 127.0.0.1\r\n`,
  );
});

test('drop-core and node:crypto agree on random secrets and messages', async () => {
  for (let i = 0; i < 20; i++) {
    const S = crypto.randomBytes(16);
    const a = derive(S);
    const b = await core.deriveKeys(core.b64uEncode(new Uint8Array(S)));
    assert.equal(b.room, a.room);
    assert.equal(b.proof, a.proof);
    const fields = { t: 'answer', sid: core.randomSid(), ts: Date.now(), name: 'RTX-5090-Astral ünï 📱', sdp: `v=0\r\na=${i}\r\n` };
    assert.equal((await core.signMsg(b, fields)).mac, nodeSign(a, fields).mac);
    assert.equal(await core.verifyMsg(b, nodeSign(a, fields)), true);
  }
});

test('b64u is strict and canonical', () => {
  assert.deepEqual([...core.b64uDecode('AAECAw')], [0, 1, 2, 3]);
  assert.equal(core.b64uDecode('AAECAw=='), null); // padding
  assert.equal(core.b64uDecode('AAEC+w'), null); // not url-safe
  assert.equal(core.b64uDecode('AAECA'), null); // impossible length
  assert.equal(core.b64uDecode('AAECAx'), null); // non-canonical last character
  assert.equal(core.isB64u(V.b64u, 16), true);
  assert.equal(core.isB64u(V.room, 32), true);
  assert.equal(core.isB64u(V.room, 16), false);
  assert.equal(core.secretOk('AAECAwQFBgcICQoLDA0ODx'), false);
  assert.equal(core.secretOk(V.b64u), true);
  assert.equal(core.randomSid().length, 22);
  assert.equal(core.isB64u(core.randomSid(), 16), true);
});

test('deriveKeys refuses secrets that are not 16 bytes', async () => {
  await assert.rejects(core.deriveKeys('AAECAwQFBgcICQoLDA0ODw8'));
  await assert.rejects(core.deriveKeys(new Uint8Array(32)));
  await assert.rejects(core.deriveKeys('not a secret'));
});

test('verifyMsg drops tampered, stale, foreign and replayed messages', async () => {
  const k = await core.deriveKeys(V.b64u);
  const other = await core.deriveKeys(core.randomB64u(16));
  const now = Date.now();
  const sid = core.randomSid();
  const m = await core.signMsg(k, { t: 'answer', sid, ts: now, name: 'RTX-5090-Astral', sdp: 'v=0\r\n' });
  assert.equal(await core.verifyMsg(k, m, { now }), true);
  for (const field of ['t', 'sid', 'name', 'sdp']) {
    const bad = { ...m, [field]: field === 'sid' ? core.randomSid() : field === 't' ? 'busy' : `${m[field]}x` };
    assert.equal(await core.verifyMsg(k, bad, { now }), false, field);
  }
  assert.equal(await core.verifyMsg(k, { ...m, ts: m.ts + 1 }, { now }), false);
  assert.equal(await core.verifyMsg(other, m, { now }), false); // another room's key
  assert.equal(await core.verifyMsg(k, m, { now: now + 10 * 60 * 1000 }), true); // exactly 10 minutes is fine
  assert.equal(await core.verifyMsg(k, m, { now: now + 10 * 60 * 1000 + 1 }), false);
  assert.equal(await core.verifyMsg(k, m, { now: now - 10 * 60 * 1000 - 1 }), false);
  assert.equal(await core.verifyMsg(k, m, { now, types: ['busy'] }), false);
  assert.equal(await core.verifyMsg(k, m, { now, sid: core.randomSid() }), false);
  const replay = new core.ReplayCache();
  assert.equal(await core.verifyMsg(k, m, { now, replay }), true);
  assert.equal(await core.verifyMsg(k, m, { now, replay }), false);
  assert.equal(await core.verifyMsg(k, { ...m, extra: 1 }, { now }), false);
  const { mac, ...noMac } = m;
  assert.equal(await core.verifyMsg(k, noMac, { now }), false);
  assert.ok(mac);
});

test('the replay cache forgets after 15 minutes', () => {
  const c = new core.ReplayCache();
  const m = { sid: 'AAECAwQFBgcICQoLDA0ODw', t: 'offer', ts: 1 };
  assert.equal(c.check(m, 1000), true);
  assert.equal(c.check(m, 2000), false);
  assert.equal(c.check(m, 1000 + 15 * 60 * 1000), true);
});

test('names are one line of at most 60 characters', async () => {
  const k = await core.deriveKeys(V.b64u);
  const sid = core.randomSid();
  assert.equal(core.nameOk('a'.repeat(60)), true);
  assert.equal(core.nameOk('📱'.repeat(60)), true); // 60 characters, 120 UTF-16 units
  assert.equal(core.nameOk('a'.repeat(61)), false);
  assert.equal(core.nameOk('two\nlines'), false);
  assert.equal(core.nameOk('tab\there'), false);
  await assert.rejects(core.signMsg(k, { t: 'pair', sid, name: 'a\nb' }));
  await assert.rejects(core.signMsg(k, { t: 'offer', sid, sdp: 'x'.repeat(32 * 1024 + 1) }));
  assert.equal(core.cleanName('  Luca’s\n\tiPhone  '), 'Luca’s iPhone');
  assert.equal([...core.cleanName('é'.repeat(80))].length, 60);
  assert.equal(core.cleanName(42), '');
});

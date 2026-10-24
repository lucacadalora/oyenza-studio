// /api/ice: STUN always, Cloudflare TURN with a key (fetch mocked), port 53 dropped, one shared cache;
// relay passes only for a room whose PC is online, rationed per address and per hour.
import test from 'node:test';
import assert from 'node:assert/strict';
import { start as startServer, newPair, newSid, rp, sleep } from './helpers.mjs';

// Waits end at once here, so a PC's wait makes its room online straight away.
const start = (opts = {}) => startServer({ holdMs: 20, ...opts });

// A pair whose PC is online (it has just waited).
async function onlinePair(s) {
  const k = newPair();
  await s.call('/api/pc/wait', { rooms: [rp(k)], client: newSid() });
  return k;
}

const STUN = { urls: ['stun:stun.cloudflare.com:3478'] };

// What Cloudflare's generate-ice-servers answers (shape from their docs).
const CF_ANSWER = {
  iceServers: [
    { urls: ['stun:stun.cloudflare.com:3478', 'stun:stun.cloudflare.com:53'] },
    {
      urls: [
        'turn:turn.cloudflare.com:3478?transport=udp',
        'turn:turn.cloudflare.com:53?transport=udp',
        'turn:turn.cloudflare.com:3478?transport=tcp',
        'turn:turn.cloudflare.com:80?transport=tcp',
        'turns:turn.cloudflare.com:5349?transport=tcp',
        'turns:turn.cloudflare.com:443?transport=tcp',
        'turn:[2606:4700:48::1]:53?transport=udp',
      ],
      username: 'user-1',
      credential: 'pass-1',
    },
  ],
};

const WANT_TURN = {
  urls: [
    'turn:turn.cloudflare.com:3478?transport=udp',
    'turn:turn.cloudflare.com:3478?transport=tcp',
    'turn:turn.cloudflare.com:80?transport=tcp',
    'turns:turn.cloudflare.com:5349?transport=tcp',
    'turns:turn.cloudflare.com:443?transport=tcp',
  ],
  username: 'user-1',
  credential: 'pass-1',
};

function mockFetch(answer, { status = 200, delay = 0 } = {}) {
  const calls = [];
  const fn = async (url, init) => {
    calls.push({ url, init });
    if (delay) await sleep(delay);
    if (answer instanceof Error) throw answer;
    return new Response(JSON.stringify(answer), { status, headers: { 'Content-Type': 'application/json' } });
  };
  fn.calls = calls;
  return fn;
}

test('without a relay key: STUN only, Cloudflare never asked', async (t) => {
  const fetch = mockFetch(CF_ANSWER);
  const s = await start({ fetch });
  t.after(s.close);
  const r = await s.call('/api/ice', rp(await onlinePair(s)));
  assert.equal(r.status, 200);
  assert.deepEqual(r.json, { iceServers: [STUN] });
  assert.equal(fetch.calls.length, 0);
});

test('a room whose PC is offline gets STUN only, and Cloudflare is not asked', async (t) => {
  const fetch = mockFetch(CF_ANSWER);
  const s = await start({ fetch, turnKeyId: 'k', turnToken: 't' });
  t.after(s.close);
  const r = await s.call('/api/ice', rp(newPair()));
  assert.deepEqual(r.json, { iceServers: [STUN] });
  assert.equal(fetch.calls.length, 0);
});

test('relay passes are rationed per address and per hour', async (t) => {
  const fetch = mockFetch(CF_ANSWER);
  const s = await start({ fetch, turnKeyId: 'k', turnToken: 't', turnPerAddressHour: 2, turnPerHour: 3, trustProxy: true });
  t.after(s.close);
  const from = (ip) => ({ headers: { 'CF-Connecting-IP': ip } });
  const k = await onlinePair(s);
  const one = await s.call('/api/ice', rp(k), from('203.0.113.1'));
  const two = await s.call('/api/ice', rp(k), from('203.0.113.1'));
  const three = await s.call('/api/ice', rp(k), from('203.0.113.1'));
  assert.equal(one.json.iceServers.length, 2);
  assert.equal(two.json.iceServers.length, 2);
  assert.deepEqual(three.json, { iceServers: [STUN] }, 'the address used its two');
  const other = await s.call('/api/ice', rp(k), from('203.0.113.2'));
  assert.equal(other.json.iceServers.length, 2, 'another address still gets one');
  const capped = await s.call('/api/ice', rp(k), from('203.0.113.3'));
  assert.deepEqual(capped.json, { iceServers: [STUN] }, 'three in all this hour');
  await s.call('/api/ice', rp(k), from('203.0.113.4'));
  assert.equal(s.logs.filter((l) => /relay passes this hour/.test(l)).length, 1, 'one log line for the cap');
});

test('with a relay key: STUN plus Cloudflare TURN without port 53, cached for every room', async (t) => {
  const fetch = mockFetch(CF_ANSWER, { delay: 50 });
  const s = await start({ fetch, turnKeyId: 'key-id-1', turnToken: 'secret-token-1' });
  t.after(s.close);
  // two rooms at the same time share one request
  const [p1, p2] = [await onlinePair(s), await onlinePair(s)];
  const [a, b] = await Promise.all([s.call('/api/ice', rp(p1)), s.call('/api/ice', rp(p2))]);
  assert.deepEqual(a.json, { iceServers: [STUN, WANT_TURN] });
  assert.deepEqual(b.json, a.json);
  assert.equal(fetch.calls.length, 1);
  const call = fetch.calls[0];
  assert.equal(call.url, 'https://rtc.live.cloudflare.com/v1/turn/keys/key-id-1/credentials/generate-ice-servers');
  assert.equal(call.init.method, 'POST');
  assert.equal(call.init.headers.Authorization, 'Bearer secret-token-1');
  assert.deepEqual(JSON.parse(call.init.body), { ttl: 3600 });
  // later rooms get the cached set
  await s.call('/api/ice', rp(await onlinePair(s)));
  assert.equal(fetch.calls.length, 1);
  assert.deepEqual(s.logs, []);
});

test('the cached set is renewed after the cache time', async (t) => {
  const fetch = mockFetch(CF_ANSWER);
  const s = await start({ fetch, turnKeyId: 'k', turnToken: 't', iceCacheMs: 150 });
  t.after(s.close);
  await s.call('/api/ice', rp(await onlinePair(s)));
  await sleep(200);
  await s.call('/api/ice', rp(await onlinePair(s)));
  assert.equal(fetch.calls.length, 2);
});

test('the older one-object answer shape works too', async (t) => {
  const fetch = mockFetch({ iceServers: { urls: ['turn:turn.example:3478'], username: 'u', credential: 'c' } });
  const s = await start({ fetch, turnKeyId: 'k', turnToken: 't' });
  t.after(s.close);
  const r = await s.call('/api/ice', rp(await onlinePair(s)));
  assert.deepEqual(r.json.iceServers, [STUN, { urls: ['turn:turn.example:3478'], username: 'u', credential: 'c' }]);
});

for (const [why, answer, opts] of [
  ['Cloudflare refuses', { error: 'nope' }, { status: 401 }],
  ['Cloudflare is unreachable', new Error('fetch failed'), {}],
  ['the answer has no TURN entry', { iceServers: [{ urls: ['stun:stun.cloudflare.com:3478'] }] }, {}],
  ['the TURN entry has only port 53', { iceServers: [{ urls: ['turn:turn.cloudflare.com:53'], username: 'u', credential: 'c' }] }, {}],
]) {
  test(`${why}: STUN only, one log line without secrets, no retry storm`, async (t) => {
    const fetch = mockFetch(answer, opts);
    const s = await start({ fetch, turnKeyId: 'key-id-2', turnToken: 'secret-token-2' });
    t.after(s.close);
    const r = await s.call('/api/ice', rp(await onlinePair(s)));
    assert.equal(r.status, 200);
    assert.deepEqual(r.json, { iceServers: [STUN] });
    await s.call('/api/ice', rp(await onlinePair(s)));
    assert.equal(fetch.calls.length, 1, 'waits before asking again');
    assert.equal(s.logs.length, 1);
    assert.match(s.logs[0], /relay passes failed/);
    assert.doesNotMatch(s.logs[0], /secret-token-2|key-id-2/);
  });
}

test('a slow Cloudflare times out into STUN only', async (t) => {
  const fetch = async (url, init) =>
    new Promise((resolve, reject) => {
      init.signal.addEventListener('abort', () => reject(init.signal.reason));
    });
  const s = await start({ fetch, turnKeyId: 'k', turnToken: 't', iceTimeoutMs: 200 });
  t.after(s.close);
  const r = await s.timed('/api/ice', rp(await onlinePair(s)));
  assert.deepEqual(r.json, { iceServers: [STUN] });
  assert.ok(r.ms < 1500, `${r.ms} ms`);
  assert.equal(s.logs.length, 1);
});

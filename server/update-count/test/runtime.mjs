// The Worker in Cloudflare's own runtime, on this computer, against the live
// update file. No account, nothing deployed. Not part of `npm test`, since it
// needs Miniflare and the network:
//
//   npm i --prefix <a scratch folder> miniflare@4
//   MINIFLARE=<a scratch folder>/node_modules/miniflare/dist/src/index.js node server/update-count/test/runtime.mjs

import assert from 'node:assert/strict';
import http from 'node:http';
import { mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';

const { Miniflare } = await import(process.env.MINIFLARE);
const here = join(dirname(fileURLToPath(import.meta.url)), '..');
const persist = mkdtempSync(join(tmpdir(), 'update-count-'));
const READ_KEY = 'test-read-key';
const FILE = 'https://studio.oyenza.com/latest.json';
const CHECK = 'https://studio-check.oyenza.com/latest.json';
const READ = 'https://studio-update-count.example.workers.dev/days?host=studio-check.oyenza.com';
const UA = (v) => `OyenzaStudio/${v} (+https://github.com/lucacadalora/oyenza-studio)`;
const WINDOWS = (ip, v = '0.2.25') => ({ 'user-agent': UA(v), 'accept-encoding': 'gzip, deflate', 'cf-connecting-ip': ip });
const MAC = (ip, v = '0.2.25') => ({ 'user-agent': UA(v), accept: '*/*', 'accept-language': 'en-US,en;q=0.9', 'cf-connecting-ip': ip });

const mf = new Miniflare({
    modulesRoot: here,
    modules: [
        { type: 'ESModule', path: join(here, 'worker.mjs') },
        { type: 'ESModule', path: join(here, 'count.mjs') },
        { type: 'ESModule', path: join(here, 'system.mjs') },
    ],
    compatibilityDate: '2026-07-01',
    durableObjects: { TALLY: { className: 'Tally', useSQLite: true } },
    durableObjectsPersist: persist,
    bindings: { READ_KEY },
});

const passed = [];
function ok(name) {
    passed.push(name);
    console.log(`ok - ${name}`);
}
// Plain node:http, so a request carries only these headers: fetch would add
// Accept and Accept-Language, which is exactly what tells a Mac from Windows.
const base = await mf.ready;
function check(headers, method = 'GET') {
    return new Promise((done, fail) => {
        const sent = http.request({ host: base.hostname, port: base.port, path: '/latest.json', method, headers: { host: new URL(CHECK).host, ...headers } }, (reply) => {
            const chunks = [];
            reply.on('data', (chunk) => chunks.push(chunk));
            reply.on('end', () => {
                const bytes = Buffer.concat(chunks);
                const body = reply.headers['content-encoding'] === 'gzip' && bytes.length ? gunzipSync(bytes) : bytes;
                done({ status: reply.statusCode, headers: new Headers(reply.headers), body: body.toString() });
            });
        });
        sent.on('error', fail);
        sent.end();
    });
}
async function counted() {
    const reply = await mf.dispatchFetch(READ, { headers: { authorization: `Bearer ${READ_KEY}` } });
    assert.equal(reply.status, 200);
    return reply.json();
}
// waitUntil work finishes after the answer: give it a moment.
const settle = () => new Promise((done) => setTimeout(done, 400));

try {
    const direct = await fetch(FILE, { headers: { 'user-agent': 'runtime-test' } });
    const directBody = await direct.text();
    const etag = direct.headers.get('etag');

    const reply = await check(WINDOWS('203.0.113.1'));
    assert.equal(reply.status, 200);
    assert.equal(reply.body, directBody);
    assert.ok(JSON.parse(reply.body).signature, 'the signed file');
    ok('the file comes back byte for byte');
    for (const name of ['content-type', 'cache-control', 'etag', 'access-control-allow-origin']) {
        assert.equal(reply.headers.get(name), direct.headers.get(name), name);
    }
    assert.equal(reply.headers.get('cache-control'), 'max-age=600');
    ok('its headers are unchanged, max-age=600 included');

    const again = await check({ ...WINDOWS('203.0.113.1'), 'if-none-match': etag });
    assert.equal(again.status, 304);
    ok('"nothing new" stays a 304');

    await check(WINDOWS('203.0.113.2'));
    await check(WINDOWS('2001:db8:1:2:aaaa::1'));
    await check(WINDOWS('2001:db8:1:2:bbbb::2')); // the same place, within the hour
    await check(MAC('203.0.113.3'));
    await check({ ...WINDOWS('203.0.113.4'), 'user-agent': 'curl/8.5' });
    await check(WINDOWS('203.0.113.5', '9.9.9'));
    await check(WINDOWS('203.0.113.6', '0.0.0'));
    await settle();
    await settle();
    let t = (await counted()).today;
    console.log(JSON.stringify(t));
    assert.equal(t.checks, 6);
    assert.equal(t.busiest_hour_checks, 6);
    assert.equal(t.addresses, 4, '.1, .2, one IPv6 place, .3');
    assert.equal(t.copies, 6, 'two checks in one hour read as two copies: .1 and the IPv6 place did that');
    assert.equal(t.systems.mac.copies, 1);
    assert.equal(t.systems.windows.copies, 5);
    assert.equal(t.versions['0.2.25'].addresses, 4);
    assert.equal(t.test.checks, 1);
    assert.equal(t.other, 2, 'curl and a version we never released');
    ok('checks, addresses, copies, systems, tests and others add up');

    const noKey = await mf.dispatchFetch(READ);
    assert.equal(noKey.status, 404);
    await noKey.text();
    const wrongKey = await mf.dispatchFetch(READ, { headers: { authorization: 'Bearer wrong' } });
    assert.equal(wrongKey.status, 404);
    await wrongKey.text();
    ok('the totals need the read key');

    const notCounted = await mf.dispatchFetch('https://studio-update-count.example.workers.dev/latest.json', { headers: WINDOWS('203.0.113.7') });
    assert.equal(notCounted.status, 404);
    await notCounted.text();
    const head = await check(WINDOWS('203.0.113.8'), 'HEAD');
    assert.equal(head.status, 200);
    const before = (await counted()).today.checks;
    await settle();
    assert.equal((await counted()).today.checks, before);
    assert.equal(before, 6);
    ok('only a GET on the feed counts');

    const bad = await check(WINDOWS('not an address'));
    assert.equal(bad.status, 200, 'still answered');
    await settle();
    assert.equal((await counted()).today.checks, before);
    ok('a check with a bad address is answered and not counted');

    // Day end, through the object itself: yesterday's tally becomes totals.
    const tally = (await mf.getDurableObjectNamespace('TALLY')).get((await mf.getDurableObjectNamespace('TALLY')).idFromName('rollover'));
    // Tomorrow's last minute, so its alarm doesn't fire during this test.
    const day = new Date(Date.now() + 86_400_000).toISOString().slice(0, 10);
    const end = Date.parse(`${day}T23:59:00Z`);
    const nextDay = new Date(end + 120_000).toISOString().slice(0, 10);
    await tally.note({ address: '198.51.100.1', userAgent: UA('0.2.25'), network: 1, system: 'windows', at: end });
    await tally.note({ address: '198.51.100.2', userAgent: UA('0.2.25'), network: 1, system: 'mac', at: end });
    await tally.note({ address: '198.51.100.3', userAgent: UA('0.2.25'), network: 1, system: 'windows', at: end + 120_000 });
    const rolled = await tally.days();
    assert.equal(rolled.days.length, 1);
    assert.equal(rolled.days[0].day, day);
    assert.equal(rolled.days[0].copies, 2);
    assert.equal(rolled.today.day, nextDay);
    assert.equal(rolled.today.copies, 1);
    assert.ok(!JSON.stringify(rolled).match(/[0-9a-f]{16}/), 'no code or key in what is read');
    ok('at the end of the UTC day only the totals stay');
    await mf.dispose();

    // What actually reached the disk.
    const files = readdirSync(persist, { recursive: true }).map(String).filter((f) => f.endsWith('.sqlite') || f.endsWith('.sqlite-wal'));
    assert.ok(files.length > 0, 'the object saved itself');
    for (const f of files) {
        const text = readFileSync(join(persist, f)).toString('latin1');
        for (const secret of ['203.0.113.', '198.51.100.', '2001:db8']) assert.ok(!text.includes(secret), `${secret} in ${f}`);
    }
    ok('no address is on disk');
    console.log(`\n${passed.length} passed`);
} finally {
    rmSync(persist, { recursive: true, force: true });
    await mf.dispose().catch(() => {});
}

// The phone page files, their types and caching, and the headers every answer carries.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { start } from './helpers.mjs';

const PUBLIC = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'public');
const CSP = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; manifest-src 'self'; worker-src 'self'; base-uri 'none'; frame-ancestors 'none'";

function securityHeaders(r, where) {
  assert.equal(r.headers.get('content-security-policy'), CSP, where);
  assert.equal(r.headers.get('referrer-policy'), 'no-referrer', where);
  assert.equal(r.headers.get('x-content-type-options'), 'nosniff', where);
  assert.equal(r.headers.get('permissions-policy'), 'camera=(), microphone=(), geolocation=()', where);
}

test('/, /p and /share serve index.html, never cached stale', async (t) => {
  const s = await start();
  t.after(s.close);
  const index = fs.readFileSync(path.join(PUBLIC, 'index.html'), 'utf8');
  for (const p of ['/', '/p', '/share', '/?share=abc', '/p?x=1']) {
    const r = await fetch(s.base + p);
    assert.equal(r.status, 200, p);
    assert.equal(r.headers.get('content-type'), 'text/html; charset=utf-8');
    assert.equal(r.headers.get('cache-control'), 'no-cache');
    assert.equal(await r.text(), index);
    securityHeaders(r, p);
  }
});

test('assets have the right types and caching', async (t) => {
  const s = await start();
  t.after(s.close);
  const want = {
    '/app.js': ['text/javascript; charset=utf-8', 'public, max-age=300'],
    '/drop-core.js': ['text/javascript; charset=utf-8', 'public, max-age=300'],
    '/app.css': ['text/css; charset=utf-8', 'public, max-age=300'],
    '/sw.js': ['text/javascript; charset=utf-8', 'no-cache'],
    '/manifest.webmanifest': ['application/manifest+json', 'no-cache'],
    '/icon-192.png': ['image/png', 'public, max-age=300'],
    '/icon-512.png': ['image/png', 'public, max-age=300'],
    '/apple-touch-icon.png': ['image/png', 'public, max-age=300'],
  };
  for (const [p, [type, cache]] of Object.entries(want)) {
    const r = await fetch(s.base + p);
    assert.equal(r.status, 200, p);
    assert.equal(r.headers.get('content-type'), type, p);
    assert.equal(r.headers.get('cache-control'), cache, p);
    const body = Buffer.from(await r.arrayBuffer());
    assert.deepEqual(body, fs.readFileSync(path.join(PUBLIC, p.slice(1))), p);
    securityHeaders(r, p);
    // ETag round trip
    const etag = r.headers.get('etag');
    assert.ok(etag, p);
    const again = await fetch(s.base + p, { headers: { 'If-None-Match': etag } });
    assert.equal(again.status, 304, p);
  }
  const head = await fetch(`${s.base}/app.js`, { method: 'HEAD' });
  assert.equal(head.status, 200);
  assert.equal(await head.text(), '');
});

test('the manifest says what the phone page needs', async (t) => {
  const s = await start();
  t.after(s.close);
  const m = await (await fetch(`${s.base}/manifest.webmanifest`)).json();
  assert.equal(m.name, 'Oyen Poni');
  assert.equal(m.short_name, 'Oyen Poni');
  assert.equal(m.start_url, '/');
  assert.equal(m.scope, '/');
  assert.equal(m.display, 'standalone');
  assert.equal(m.background_color, '#ffffff');
  assert.equal(m.theme_color, '#ffffff');
  assert.deepEqual(m.share_target, {
    action: '/share',
    method: 'POST',
    enctype: 'multipart/form-data',
    params: { title: 'title', text: 'text', url: 'url', files: [{ name: 'files', accept: ['*/*'] }] },
  });
  const sizes = m.icons.map((i) => `${i.sizes} ${i.purpose || 'any'}`);
  assert.ok(sizes.includes('192x192 any'));
  assert.ok(sizes.includes('512x512 maskable'));
});

test('icons are the sizes their names say', () => {
  const size = (f) => {
    const b = fs.readFileSync(path.join(PUBLIC, f));
    assert.equal(b.toString('latin1', 1, 4), 'PNG', f);
    return [b.readUInt32BE(16), b.readUInt32BE(20)];
  };
  assert.deepEqual(size('icon-192.png'), [192, 192]);
  assert.deepEqual(size('icon-512.png'), [512, 512]);
  assert.deepEqual(size('apple-touch-icon.png'), [180, 180]);
});

test('no inline scripts or styles anywhere in the page', () => {
  const html = fs.readFileSync(path.join(PUBLIC, 'index.html'), 'utf8');
  assert.doesNotMatch(html, /<script(?![^>]*\ssrc=)[^>]*>/i, 'every script has a src');
  assert.doesNotMatch(html, /<style/i);
  assert.doesNotMatch(html, /\sstyle=/i);
  assert.doesNotMatch(html, /\son[a-z]+=/i, 'no inline event handlers');
  assert.doesNotMatch(html, /rel=["']?manifest/i, 'the manifest link is added from JS, never on iOS');
  for (const f of ['app.js', 'sw.js', 'drop-core.js']) {
    const js = fs.readFileSync(path.join(PUBLIC, f), 'utf8');
    assert.doesNotMatch(js, /setAttribute\(\s*['"]style['"]/, f);
    assert.doesNotMatch(js, /\beval\(|new Function\(/, f);
  }
});

test('unknown paths are 404, wrong methods 405', async (t) => {
  const s = await start();
  t.after(s.close);
  for (const p of ['/nope', '/../drop-server.mjs', '/%2e%2e/drop-server.mjs', '/public/app.js', '/.gitignore']) {
    const r = await fetch(s.base + p);
    assert.equal(r.status, 404, p);
    securityHeaders(r, p);
  }
  assert.equal((await fetch(`${s.base}/app.js`, { method: 'POST' })).status, 405);
  assert.equal((await fetch(`${s.base}/`, { method: 'DELETE' })).status, 405);
});

test('POST /share without a service worker goes back to the page', async (t) => {
  const s = await start();
  t.after(s.close);
  const form = new FormData();
  form.append('files', new Blob([new Uint8Array(1000)]), 'IMG_1.jpg');
  const r = await fetch(`${s.base}/share`, { method: 'POST', body: form, redirect: 'manual' });
  assert.equal(r.status, 303);
  assert.equal(r.headers.get('location'), '/?shared=lost');
  securityHeaders(r, 'POST /share');
});

test('API answers carry the same security headers', async (t) => {
  const s = await start();
  t.after(s.close);
  for (const [route, init] of [
    ['/api/health', {}],
    ['/api/phone/status', { method: 'POST', body: '{}' }],
    ['/api/nope', { method: 'POST', body: '{}' }],
  ]) {
    const r = await fetch(s.base + route, init);
    securityHeaders(r, route);
    assert.equal(r.headers.get('cache-control'), 'no-store', route);
    assert.equal(r.headers.get('access-control-allow-origin'), null, 'no CORS');
  }
});

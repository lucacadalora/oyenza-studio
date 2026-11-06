// Opt-in usage counts: a report becomes totals and nothing else stays.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { COUNTS, createCounts, readReport, weekOf } from '../counts.mjs';
import { view } from '../counts-view.mjs';
import { start, tempDir } from './helpers.mjs';

const NOW = Date.UTC(2026, 10, 9, 12); // Monday 9 Nov 2026
const report = (more = {}) => ({
  schema: 1,
  version: '0.2.27',
  os: 'windows',
  installed: '2026-11',
  days_used: 4,
  tabs: { media: 4, shelf: 1 },
  addons: ['sholat'],
  features: ['phone_drop'],
  ...more,
});

test('weeks are ISO weeks, over a new year too', () => {
  assert.equal(weekOf(NOW), '2026-W46');
  assert.equal(weekOf(Date.UTC(2026, 10, 8, 23)), '2026-W45');
  assert.equal(weekOf(Date.UTC(2027, 0, 1)), '2026-W53');
  assert.equal(weekOf(Date.UTC(2024, 11, 30)), '2025-W01');
});

test('a report of the wrong shape is refused whole', () => {
  assert.ok(readReport(report(), NOW));
  assert.ok(readReport(report({ installed: null }), NOW));
  for (const bad of [
    { schema: 2 },
    { version: 'my name' },
    { os: 'linux' },
    { installed: 'Budi' },
    { installed: '2026-12' }, // later than now
    { installed: '2025-01' }, // before the app existed
    { days_used: 0 },
    { days_used: 8 },
    { days_used: 1.5 },
    { tabs: [] },
    { tabs: { media: 5 } }, // more days than the week had
    { tabs: { media: 'x' } },
    { addons: 'sholat' },
    { features: [1] },
    { features: ['x'.repeat(33)] },
  ]) {
    assert.equal(readReport(report(bad), NOW), null, JSON.stringify(bad));
  }
});

test('names off the lists never reach the totals', () => {
  const r = readReport(report({ tabs: { media: 1, 'someone@mail.example': 1 }, addons: ['my-secret'], features: ['free text'] }), NOW);
  assert.deepEqual(r.tabs, [['media', 1]]);
  assert.deepEqual(r.addons, []);
  assert.deepEqual(r.features, []);
});

test('totals add up, and a week reloads after a restart', () => {
  const dataDir = tempDir();
  const a = createCounts({ dataDir, now: () => NOW });
  assert.ok(a.add(report()));
  assert.ok(a.add(report({ os: 'mac', days_used: 7, tabs: { media: 7 }, installed: null, features: [] })));
  assert.equal(a.add(report({ schema: 9 })), false);
  a.flush();
  const b = createCounts({ dataDir, now: () => NOW });
  assert.ok(b.add(report()));
  b.flush();
  const t = JSON.parse(fs.readFileSync(path.join(dataDir, 'counts', '2026-W46.json'), 'utf8'));
  assert.equal(t.reports, 3);
  assert.deepEqual(t.os, { windows: 2, mac: 1 });
  assert.deepEqual(t.installed, { '2026-11': 2, unknown: 1 });
  assert.deepEqual(t.days_used, { 4: 2, 7: 1 });
  assert.deepEqual(t.tabs, { media: { 4: 2, 7: 1 }, shelf: { 1: 2 } });
  assert.deepEqual(t.features, { phone_drop: 2 });
});

test('a new week starts new totals and keeps the last', () => {
  const dataDir = tempDir();
  let now = NOW;
  const c = createCounts({ dataDir, now: () => now });
  c.add(report());
  now += 7 * 86_400_000;
  c.add(report());
  c.flush();
  assert.deepEqual(fs.readdirSync(path.join(dataDir, 'counts')).sort(), ['2026-W46.json', '2026-W47.json']);
});

test('the route stores totals only: no address, no report', async () => {
  const s = await start();
  const secret = '203.0.113.77';
  const r = await s.call('/api/counts', report(), { headers: { 'X-Forwarded-For': secret, 'User-Agent': 'OyenzaStudio/0.2.27' } });
  assert.equal(r.status, 204);
  assert.equal((await s.call('/api/counts', report({ os: 'linux' }))).status, 400);
  assert.equal((await s.call('/api/counts', undefined, { method: 'GET' })).status, 405);
  await s.close();
  const dir = path.join(s.dataDir, 'counts');
  const files = fs.readdirSync(dir);
  assert.equal(files.length, 1);
  const text = fs.readFileSync(path.join(dir, files[0]), 'utf8');
  assert.equal(JSON.parse(text).reports, 1);
  assert.ok(!text.includes(secret) && !text.includes('OyenzaStudio'));
  assert.deepEqual(s.logs, []);
});

test('one address can send only so many reports an hour', async () => {
  const s = await start();
  let last;
  for (let i = 0; i <= COUNTS.perAddressHour; i += 1) last = await s.call('/api/counts', report());
  assert.equal(last.status, 429);
  await s.close();
});

test('a report too big to be one is refused', async () => {
  const s = await start();
  const r = await s.call('/api/counts', report({ features: Array(60).fill('x'.repeat(32)), padding: 'y'.repeat(5000) }));
  assert.equal(r.status, 400);
  await s.close();
});

test('the view hides small groups and small weeks', () => {
  const t = {
    week: '2026-W46',
    reports: 100,
    version: { '0.2.27': 100 },
    os: { windows: 95, mac: 5 },
    installed: {},
    days_used: {},
    tabs: { media: { 7: 50, 1: 50 }, ielts: { 2: 3 } },
    addons: {},
    features: {},
  };
  const v = view(t, 20);
  assert.match(v, /windows: 95 \(95%\)/);
  assert.match(v, /mac: under 20\n/);
  assert.match(v, /media: 100 \(100%\), 4\.0 days/);
  assert.match(v, /ielts: under 20$/m);
  const few = view({ ...t, reports: 19 }, 20);
  assert.match(few, /Too few to show/);
  assert.ok(!few.includes('windows'));
});

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { add, classify, codeFor, dayAndHour, MAX_COPIES_PER_ADDRESS, newDay, newKey, placeOf, released, totals } from '../count.mjs';

const ua = (version) => `OyenzaStudio/${version} (+https://github.com/lucacadalora/oyenza-studio)`;

test('only the exact User-Agent of a released version is a copy', () => {
    assert.deepEqual(classify(ua('0.2.25'), 7713, '0.2.25'), { kind: 'copy', version: '0.2.25' });
    for (const other of [null, '', 'curl/8.5', 'Mozilla/5.0', 'OyenzaStudio smoke', 'OyenzaStudio/0.2.25', `${ua('0.2.25')} extra`, `x ${ua('0.2.25')}`, 'OyenzaStudio/test']) {
        assert.deepEqual(classify(other, 7713, '0.2.25'), { kind: 'other' }, String(other));
    }
    assert.deepEqual(classify(ua('0.2.5'), 7713, '0.2.25'), { kind: 'other' }, 'no such version reads the file');
    assert.deepEqual(classify(ua('9.9.9'), 7713, '0.2.25'), { kind: 'other' }, 'a version we never released');
});

test('the next version up is allowed before the file is read again', () => {
    assert.ok(released('0.2.26', '0.2.25'));
    assert.ok(released('0.3.0', '0.2.25'));
    assert.ok(released('1.0.0', '0.2.25'));
    assert.ok(!released('0.2.27', '0.2.25'));
    assert.ok(!released('0.3.1', '0.2.25'));
    assert.ok(released('0.2.25', null), 'the file not read yet: any version from 0.2.6 on');
    assert.ok(!released('0.2.5', null));
});

test('our tests and our CI stay out of the copies', () => {
    assert.deepEqual(classify(ua('0.0.0'), 7713, '0.2.25'), { kind: 'test' });
    assert.deepEqual(classify(ua('0.0.0'), 8075, '0.2.25'), { kind: 'test' }, 'a test from a GitHub runner is still a test');
    assert.deepEqual(classify(ua('0.2.25'), 8075, '0.2.25'), { kind: 'left_out' });
});

test('an address becomes the part that stands for one place', () => {
    assert.equal(placeOf('203.0.113.9'), '203.0.113.9');
    assert.equal(placeOf('::ffff:203.0.113.9'), '203.0.113.9');
    assert.equal(placeOf('2001:db8:1:2:3:4:5:6'), '2001:db8:1:2');
    assert.equal(placeOf('2001:db8:1:2::5'), '2001:db8:1:2', 'the end of the address changes by itself');
    assert.equal(placeOf('2001:DB8:1:2:ffff::'), '2001:db8:1:2');
    assert.equal(placeOf('2001:db8::1'), '2001:db8:0:0');
    assert.equal(placeOf('2001:0db8:0001:0002::1'), '2001:db8:1:2');
    assert.equal(placeOf('::1'), '0:0:0:0');
    for (const bad of [null, undefined, '', 'nonsense', '<script>', '1'.repeat(60)]) assert.equal(placeOf(bad), null);
});

test('a code changes with the key and never holds the address', async () => {
    const [a, b] = [newKey(), newKey()];
    assert.notEqual(a, b);
    assert.match(a, /^[0-9a-f]{64}$/);
    const code = await codeFor('203.0.113.9', a);
    assert.match(code, /^[0-9a-f]{16}$/);
    assert.equal(await codeFor('203.0.113.9', a), code, 'the same all day');
    assert.notEqual(await codeFor('203.0.113.9', b), code, 'another key, another day: no following a copy across days');
    assert.notEqual(await codeFor('203.0.113.10', a), code);
});

test('the day and hour are UTC', () => {
    assert.deepEqual(dayAndHour(Date.parse('2026-11-05T23:59:59Z')), { day: '2026-11-05', hour: 23 });
    assert.deepEqual(dayAndHour(Date.parse('2026-11-06T00:00:00Z')), { day: '2026-11-06', hour: 0 });
    assert.deepEqual(dayAndHour(Date.parse('2026-11-06T06:59:59+07:00')), { day: '2026-11-05', hour: 23 });
});

const copy = (code, hour, version = '0.2.25', system = 'windows') => ({ kind: 'copy', version, system, code, hour });

test('one copy checking all day is one copy', () => {
    const s = newDay('2026-11-05', 'k');
    for (let hour = 0; hour < 24; hour++) add(s, copy('a', hour));
    const t = totals(s);
    assert.equal(t.checks, 24);
    assert.equal(t.addresses, 1);
    assert.equal(t.copies, 1);
    assert.equal(t.busiest_hour_checks, 1);
});

test('several copies behind one address show in its busiest hour', () => {
    const s = newDay('2026-11-05', 'k');
    for (let i = 0; i < 7; i++) add(s, copy('office', 10));
    for (let i = 0; i < 3; i++) add(s, copy('office', 11));
    add(s, copy('home', 10));
    const t = totals(s);
    assert.equal(t.addresses, 2);
    assert.equal(t.copies, 8, '7 at the office, 1 at home');
    assert.equal(t.busiest_hour_checks, 8);
});

test('coming back to an earlier hour starts that hour afresh', () => {
    const s = newDay('2026-11-05', 'k');
    add(s, copy('a', 10));
    add(s, copy('a', 11));
    add(s, copy('a', 10));
    assert.equal(totals(s).copies, 1);
});

test('one address counts as a limited number of copies', () => {
    const s = newDay('2026-11-05', 'k');
    for (let i = 0; i < MAX_COPIES_PER_ADDRESS * 4; i++) add(s, copy('script', 10));
    const t = totals(s);
    assert.equal(t.copies, MAX_COPIES_PER_ADDRESS);
    assert.equal(t.checks, MAX_COPIES_PER_ADDRESS * 4);
});

test('a copy that updates during the day counts once in the total, and under both versions', () => {
    const s = newDay('2026-11-05', 'k');
    add(s, copy('a', 9, '0.2.25'));
    add(s, copy('a', 10, '0.2.26'));
    const t = totals(s);
    assert.equal(t.copies, 1);
    assert.deepEqual(t.versions, { '0.2.25': { addresses: 1, copies: 1 }, '0.2.26': { addresses: 1, copies: 1 } });
});

test('systems are added up when known', () => {
    const s = newDay('2026-11-05', 'k');
    add(s, copy('a', 9, '0.2.25', 'windows'));
    add(s, copy('b', 9, '0.2.25', 'mac'));
    add(s, copy('c', 9, '0.2.25', null));
    const t = totals(s);
    assert.equal(t.copies, 3);
    assert.deepEqual(t.systems, { windows: { addresses: 1, copies: 1 }, mac: { addresses: 1, copies: 1 } });
});

test('other requests, CI and tests have their own numbers', () => {
    const s = newDay('2026-11-05', 'k');
    add(s, { kind: 'other', hour: 1 });
    add(s, { kind: 'left_out', hour: 1 });
    add(s, { kind: 'test', code: 't1', hour: 1 });
    add(s, { kind: 'test', code: 't1', hour: 2 });
    add(s, { kind: 'test', code: 't2', hour: 2 });
    const t = totals(s);
    assert.equal(t.checks, 0);
    assert.equal(t.copies, 0);
    assert.equal(t.busiest_hour_checks, 0);
    assert.equal(t.other, 1);
    assert.equal(t.left_out, 1);
    assert.deepEqual(t.test, { checks: 3, addresses: 2 });
});

test('totals hold no key, no code and no address', async () => {
    const key = newKey();
    const code = await codeFor('203.0.113.9', key);
    const s = newDay('2026-11-05', key);
    add(s, copy(code, 3));
    add(s, { kind: 'test', code, hour: 3 });
    const text = JSON.stringify(totals(s));
    for (const secret of [key, code, '203.0.113.9']) assert.ok(!text.includes(secret), secret);
});

test('the state survives being saved', () => {
    const s = newDay('2026-11-05', 'k');
    add(s, copy('a', 10));
    const back = JSON.parse(JSON.stringify(s));
    add(back, copy('a', 10));
    assert.equal(totals(back).copies, 2);
    assert.equal(totals(back).checks, 2);
});

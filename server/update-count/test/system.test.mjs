import assert from 'node:assert/strict';
import { test } from 'node:test';
import { systemOf } from '../system.mjs';

test('each system is told by the headers its web library adds', () => {
    assert.equal(systemOf(new Headers({ 'accept-encoding': 'gzip, br' })), 'windows');
    assert.equal(systemOf(new Headers({ accept: '*/*', 'accept-language': 'en-US,en;q=0.9', 'accept-encoding': 'gzip, br' })), 'mac');
    assert.equal(systemOf(new Headers({ accept: '*/*', 'accept-language': 'id-ID,id;q=0.9' })), 'mac');
});

test('anything else stays unknown', () => {
    assert.equal(systemOf(new Headers({ accept: '*/*' })), null, 'curl');
    assert.equal(systemOf(new Headers({ accept: 'text/html', 'accept-language': 'en' })), null, 'a browser');
    assert.equal(systemOf(new Headers({ 'accept-language': 'en' })), null);
});

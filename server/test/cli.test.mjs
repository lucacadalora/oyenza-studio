// The command line: start line, relay key from the environment (never printed), bad options, clean stop.
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { tempDir } from './helpers.mjs';

const SERVER = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'drop-server.mjs');

function run(args, env = {}) {
  const child = spawn(process.execPath, [SERVER, ...args], { env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'] });
  let out = '';
  let err = '';
  child.stdout.on('data', (d) => (out += d));
  child.stderr.on('data', (d) => (err += d));
  const exited = new Promise((resolve) => child.on('exit', (code) => resolve(code)));
  const firstLine = new Promise((resolve) => {
    const check = () => out.includes('\n') && resolve(out.split('\n')[0]);
    child.stdout.on('data', check);
    child.on('exit', () => resolve(out.split('\n')[0]));
  });
  return { child, exited, firstLine, output: () => ({ out, err }) };
}

test('starts on a free port, says so in one line without secrets, stops on SIGTERM', async () => {
  const p = run(['--port', '0', '--data', tempDir()], { TURN_KEY_ID: 'cli-key-id', TURN_KEY_API_TOKEN: 'cli-secret-token' });
  const line = await p.firstLine;
  assert.match(line, /^drop-server listening on http:\/\/127\.0\.0\.1:\d+ \(relay on\)$/);
  const base = /http:\/\/[^ ]+/.exec(line)[0];
  const r = await fetch(`${base}/api/health`);
  assert.deepEqual(await r.json(), { ok: true });
  p.child.kill('SIGTERM');
  assert.equal(await p.exited, 0);
  const { out, err } = p.output();
  assert.doesNotMatch(out + err, /cli-secret-token|cli-key-id/);
  assert.equal(err, '');
});

test('without a relay key it says relay off', async () => {
  const p = run(['--port', '0', '--data', tempDir()], { TURN_KEY_ID: '', TURN_KEY_API_TOKEN: '' });
  assert.match(await p.firstLine, /\(relay off\)$/);
  p.child.kill('SIGINT');
  assert.equal(await p.exited, 0);
});

test('unknown options print the usage and exit 2', async () => {
  const p = run(['--port', '0', '--bogus']);
  assert.equal(await p.exited, 2);
  assert.match(p.output().err, /unknown option --bogus[\s\S]*Usage: node server\/drop-server\.mjs --port 8797/);
});

test('a port in use is one error line and exit 1', async () => {
  const a = run(['--port', '0', '--data', tempDir()]);
  const port = /:(\d+) /.exec(await a.firstLine)[1];
  const b = run(['--port', port, '--data', tempDir()]);
  assert.equal(await b.exited, 1);
  assert.match(b.output().err, /can't listen on 127\.0\.0\.1:\d+/);
  a.child.kill('SIGTERM');
  await a.exited;
});

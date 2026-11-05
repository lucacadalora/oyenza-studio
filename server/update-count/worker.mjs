// Counts Oyenza Studio's hourly update checks, on the way past. README.md says
// what it does and how to run it. The file itself is never touched: GitHub
// Pages' answer goes back as it came, and anything that goes wrong in here only
// loses a count, never a check.

import { DurableObject } from 'cloudflare:workers';
import { add, classify, codeFor, dayAndHour, KEEP_DAYS, newDay, newKey, placeOf, totals } from './count.mjs';
import { systemOf } from './system.mjs';

const FEED_HOST = 'studio.oyenza.com';
const FEED_PATH = '/latest.json';
/** How often the day's tally is saved. A restart loses at most this much. */
const SAVE_EVERY_MS = 60_000;
/** How often the update file's version is read again. */
const READ_FEED_EVERY_MS = 3_600_000;
/** SQLite keeps a row under 2 MB; the tally is split well below that. */
const PART_CHARS = 1_000_000;

export class Tally extends DurableObject {
    constructor(ctx, env) {
        super(ctx, env);
        this.state = null;
        this.dirty = false;
        this.latest = null;
        this.latestAt = 0;
        ctx.blockConcurrencyWhile(async () => this.load());
    }

    load() {
        const sql = this.ctx.storage.sql;
        sql.exec('CREATE TABLE IF NOT EXISTS days (day TEXT PRIMARY KEY, totals TEXT NOT NULL)');
        sql.exec('CREATE TABLE IF NOT EXISTS today (part INTEGER PRIMARY KEY, text TEXT NOT NULL)');
        const text = sql.exec('SELECT text FROM today ORDER BY part').toArray().map((row) => row.text).join('');
        this.state = text ? JSON.parse(text) : null;
    }

    save() {
        if (!this.dirty || !this.state) return;
        const sql = this.ctx.storage.sql;
        const text = JSON.stringify(this.state);
        this.ctx.storage.transactionSync(() => {
            sql.exec('DELETE FROM today');
            for (let i = 0; i * PART_CHARS < text.length; i++) {
                sql.exec('INSERT INTO today (part, text) VALUES (?, ?)', i, text.slice(i * PART_CHARS, (i + 1) * PART_CHARS));
            }
        });
        this.dirty = false;
    }

    /** The day is over: keep its totals, and delete its key and codes. */
    finish() {
        if (!this.state) return;
        const sql = this.ctx.storage.sql;
        const done = totals(this.state);
        const oldest = new Date(Date.parse(`${done.day}T00:00:00Z`) - KEEP_DAYS * 86_400_000).toISOString().slice(0, 10);
        this.ctx.storage.transactionSync(() => {
            sql.exec('INSERT OR REPLACE INTO days (day, totals) VALUES (?, ?)', done.day, JSON.stringify(done));
            sql.exec('DELETE FROM today');
            sql.exec('DELETE FROM days WHERE day < ?', oldest);
        });
        this.state = null;
        this.dirty = false;
    }

    /** One check. The address stops here: only its scrambled code is kept. */
    async note({ address, userAgent, network, system, at }) {
        const { day, hour } = dayAndHour(at);
        if (this.state && this.state.day !== day) this.finish();
        if (!this.latestAt) await this.readFeed(at);
        const seen = classify(userAgent, network, this.latest);
        if (!this.state) this.state = newDay(day, newKey());
        let code = null;
        if (seen.kind === 'copy' || seen.kind === 'test') {
            const place = placeOf(address);
            if (!place) return;
            const { key } = this.state;
            code = await codeFor(place, key);
            if (this.state?.key !== key) return; // the day ended while this waited
        }
        add(this.state, { ...seen, system, code, hour });
        this.dirty = true;
        await this.remind(at);
    }

    async remind(at) {
        if ((await this.ctx.storage.getAlarm()) === null) await this.ctx.storage.setAlarm(at + SAVE_EVERY_MS);
    }

    async alarm() {
        const now = Date.now();
        if (this.state && this.state.day !== dayAndHour(now).day) this.finish();
        this.save();
        if (now - this.latestAt >= READ_FEED_EVERY_MS) await this.readFeed(now);
        // Stay awake while a day is open, so its end is noticed with nobody checking.
        if (this.state) await this.ctx.storage.setAlarm(now + SAVE_EVERY_MS);
    }

    async readFeed(now) {
        this.latestAt = now;
        try {
            const reply = await fetch(`https://${FEED_HOST}${FEED_PATH}`, { headers: { 'user-agent': 'studio-update-count' } });
            if (reply.ok) this.latest = JSON.parse(JSON.parse(await reply.text()).payload).version ?? this.latest;
        } catch {
            // Keep the last version read: the check is only a filter.
        }
    }

    /** Day totals, oldest first, and today so far. Never a key or a code. */
    days() {
        const rows = this.ctx.storage.sql.exec('SELECT totals FROM days ORDER BY day').toArray();
        return { days: rows.map((row) => JSON.parse(row.totals)), today: this.state ? totals(this.state) : null };
    }
}

/** Count one check. Runs after the answer is on its way, and can't reach it. */
async function count(request, env) {
    if (request.method !== 'GET') return;
    const tally = env.TALLY.get(env.TALLY.idFromName(new URL(request.url).hostname));
    await tally.note({
        address: request.headers.get('cf-connecting-ip'),
        userAgent: request.headers.get('user-agent'),
        network: request.cf?.asn ?? 0,
        system: systemOf(request.headers),
        at: Date.now(),
    });
}

function json(body, status = 200) {
    return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json', 'cache-control': 'no-store' } });
}

/** The stats page's read. Only on the Worker's own address, with the key. */
async function read(request, env, url) {
    const given = request.headers.get('authorization') ?? '';
    if (!env.READ_KEY || given !== `Bearer ${env.READ_KEY}`) return json({ error: 'not found' }, 404);
    const host = url.searchParams.get('host') || FEED_HOST;
    return json(await env.TALLY.get(env.TALLY.idFromName(host)).days());
}

export default {
    async fetch(request, env, ctx) {
        // Anything that throws in here: the request goes on to GitHub Pages as if we weren't here.
        ctx.passThroughOnException();
        const url = new URL(request.url);
        if (url.pathname === '/days' && url.hostname.endsWith('.workers.dev')) return read(request, env, url);
        if (url.pathname !== FEED_PATH || url.hostname.endsWith('.workers.dev')) return json({ error: 'not found' }, 404);
        ctx.waitUntil(count(request, env).catch(() => {}));
        if (url.hostname === FEED_HOST) return fetch(request);
        // The trial name, studio-check.oyenza.com: the real file, counted apart.
        url.protocol = 'https:';
        url.hostname = FEED_HOST;
        return fetch(new Request(url, request));
    },
};

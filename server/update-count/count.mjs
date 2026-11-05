// The counting itself, with no Cloudflare in it, so `node --test` can run it.
// README.md says what this is for; PRIVACY.md says what people are told.

/** One address counts as this many copies at most, so one script can't inflate the number. */
export const MAX_COPIES_PER_ADDRESS = 50;
/** Days of totals kept here. The stats page keeps the history. */
export const KEEP_DAYS = 90;
/** The first version that reads the update file. */
const FIRST_CHECKING = [0, 2, 6];
/** The version our own test checks claim: they get a row of their own. */
export const TEST_VERSION = '0.0.0';
/** Microsoft's network, where our CI runs. Real copies there are rare. */
const LEFT_OUT_NETWORKS = new Set([8075]);
/** Exactly what every copy sends (crates/poni/src/services/addons.rs). */
const USER_AGENT = /^OyenzaStudio\/(\d{1,4}\.\d{1,4}\.\d{1,4}) \(\+https:\/\/github\.com\/lucacadalora\/oyenza-studio\)$/;

const parts = (version) => version.split('.').map(Number);

function compare(a, b) {
    for (let i = 0; i < 3; i++) {
        if (a[i] !== b[i]) return a[i] - b[i];
    }
    return 0;
}

/**
 * Could this version be one we released? `latest` is what the update file
 * names, read about once an hour, so the next version up is allowed too: it
 * can reach people before the next read here.
 */
export function released(version, latest) {
    const v = parts(version);
    if (compare(v, FIRST_CHECKING) < 0) return false;
    if (!latest) return true;
    const l = parts(latest);
    if (compare(v, l) <= 0) return true;
    return compare(v, [l[0], l[1], l[2] + 1]) === 0 || compare(v, [l[0], l[1] + 1, 0]) === 0 || compare(v, [l[0] + 1, 0, 0]) === 0;
}

/** What this request is: `copy` (with its version), `test`, `left_out` or `other`. */
export function classify(userAgent, network, latest) {
    const version = USER_AGENT.exec(userAgent ?? '')?.[1];
    if (!version) return { kind: 'other' };
    if (version === TEST_VERSION) return { kind: 'test' };
    if (!released(version, latest)) return { kind: 'other' };
    if (LEFT_OUT_NETWORKS.has(network)) return { kind: 'left_out' };
    return { kind: 'copy', version };
}

/**
 * The part of an address that stands for one place: all of an IPv4 address,
 * the first 64 bits of an IPv6 one (the rest changes by itself). Null if it
 * isn't an address.
 */
export function placeOf(address) {
    if (typeof address !== 'string' || address.length > 45) return null;
    const mapped = /^::ffff:(\d+\.\d+\.\d+\.\d+)$/i.exec(address);
    if (mapped) return mapped[1];
    if (/^\d+\.\d+\.\d+\.\d+$/.test(address)) return address;
    if (!/^[0-9a-f:]+$/i.test(address) || !address.includes(':')) return null;
    const [head, tail] = address.toLowerCase().split('::');
    const front = head ? head.split(':') : [];
    const back = tail ? tail.split(':') : [];
    const missing = address.includes('::') ? 8 - front.length - back.length : 0;
    const groups = [...front, ...Array(Math.max(missing, 0)).fill('0'), ...back];
    return groups.slice(0, 4).map((g) => g.replace(/^0+(?=.)/, '')).join(':');
}

function hex(bytes) {
    return [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
}

function fromHex(text) {
    return Uint8Array.from(text.match(/../g), (h) => parseInt(h, 16));
}

/** A new random key, as the state keeps it. */
export function newKey() {
    return hex(crypto.getRandomValues(new Uint8Array(32)));
}

const signing = new Map();

/** An address's code for the day: scrambled with the day's key, 64 bits of it kept. */
export async function codeFor(place, keyText) {
    let key = signing.get(keyText);
    if (!key) {
        key = await crypto.subtle.importKey('raw', fromHex(keyText), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
        signing.clear(); // only today's stays in memory
        signing.set(keyText, key);
    }
    const mac = await crypto.subtle.sign('HMAC', key, new TextEncoder().encode(place));
    return hex(new Uint8Array(mac).subarray(0, 8));
}

/** The UTC day and hour of a moment in milliseconds. */
export function dayAndHour(at) {
    const iso = new Date(at).toISOString();
    return { day: iso.slice(0, 10), hour: Number(iso.slice(11, 13)) };
}

/** A new day's state. It holds the key and the codes until `totals` replaces it. */
export function newDay(day, key) {
    return { day, key, checks: 0, hours: Array(24).fill(0), other: 0, left_out: 0, groups: { all: {}, test: {} }, test_checks: 0 };
}

// A code's entry in a group: [the hour it last checked, its checks in that
// hour, its most checks in any one hour].
function note(group, code, hour) {
    const entry = group[code] ?? (group[code] = [hour, 0, 0]);
    if (entry[0] !== hour) {
        entry[0] = hour;
        entry[1] = 0;
    }
    entry[1] += 1;
    if (entry[1] > entry[2]) entry[2] = entry[1];
}

/** Add one check. `system` is 'windows', 'mac' or null; `code` may be null for requests we only add up. */
export function add(state, { kind, version, system, code, hour }) {
    if (kind === 'other') {
        state.other += 1;
    } else if (kind === 'left_out') {
        state.left_out += 1;
    } else if (kind === 'test') {
        state.test_checks += 1;
        note(state.groups.test, code, hour);
    } else {
        state.checks += 1;
        state.hours[hour] += 1;
        note(state.groups.all, code, hour);
        const v = `v${version}`;
        note(state.groups[v] ?? (state.groups[v] = {}), code, hour);
        if (system) note(state.groups[system] ?? (state.groups[system] = {}), code, hour);
    }
}

function sizes(group) {
    let copies = 0;
    const codes = Object.values(group);
    for (const entry of codes) copies += Math.min(entry[2], MAX_COPIES_PER_ADDRESS);
    return { addresses: codes.length, copies };
}

/** Only numbers: this is what stays once the key and the codes are gone. */
export function totals(state) {
    const all = sizes(state.groups.all);
    const versions = {};
    const systems = {};
    for (const [name, group] of Object.entries(state.groups)) {
        if (name.startsWith('v')) versions[name.slice(1)] = sizes(group);
        if (name === 'windows' || name === 'mac') systems[name] = sizes(group);
    }
    return {
        day: state.day,
        checks: state.checks,
        busiest_hour_checks: Math.max(...state.hours),
        addresses: all.addresses,
        copies: all.copies,
        versions,
        systems,
        other: state.other,
        left_out: state.left_out,
        test: { checks: state.test_checks, addresses: sizes(state.groups.test).addresses },
    };
}

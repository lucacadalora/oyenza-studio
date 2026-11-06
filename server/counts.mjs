// Opt-in usage counts (docs/usage-counts.md). A copy of Oyenza Studio whose
// owner said yes posts one small report a week. A report is added to that
// week's totals and thrown away: no report, address or ID is ever stored, and
// nothing here reads the rooms or the messages of the phone drop.
//
// The lists below are the whole vocabulary. A name that isn't on them is left
// out of the totals, so a report can't carry free text onto the disk, and a
// report of the wrong shape is refused whole. A newer app's new name waits for
// a server that lists it.

import fs from 'node:fs';
import path from 'node:path';

export const COUNTS = Object.freeze({
  bodyMax: 4 * 1024,
  perAddressHour: 30, // many copies can share one address (mobile networks)
  writeMs: 5_000,
  firstMonth: '2026-10', // Oyenza Studio's first month
  tabs: new Set([
    'media', 'today', 'shelf', 'tasks', 'focus', 'ai', 'oyenza', 'health', 'ielts', 'notes', 'system',
    'script', 'record', 'prayer', 'stocks', 'downloads', 'notifications', 'addons', 'settings',
  ]),
  addons: new Set(['sholat', 'ja', 'script', 'system', 'health', 'stocks', 'recorder', 'downloader', 'ai', 'ielts', 'notes']),
  features: new Set([
    'media_controls', 'phone_drop', 'paired_phone', 'ielts', 'notes', 'downloader', 'record', 'pin', 'advanced_settings',
  ]),
  os: new Set(['windows', 'mac']),
});

const VERSION = /^\d{1,3}\.\d{1,3}\.\d{1,3}$/;
const MONTH = /^20\d\d-(0[1-9]|1[0-2])$/;

const day = (v) => Number.isInteger(v) && v >= 0 && v <= 7;
const names = (v) => Array.isArray(v) && v.length <= 64 && v.every((n) => typeof n === 'string' && n.length <= 32);

// The report as totals can take it, or null when it isn't one.
export function readReport(r, now = Date.now()) {
  if (!r || r.schema !== 1) return null;
  if (typeof r.version !== 'string' || !VERSION.test(r.version)) return null;
  if (!COUNTS.os.has(r.os)) return null;
  const month = new Date(now).toISOString().slice(0, 7);
  if (r.installed !== null && (typeof r.installed !== 'string' || !MONTH.test(r.installed) || r.installed < COUNTS.firstMonth || r.installed > month)) return null;
  if (!day(r.days_used) || r.days_used === 0) return null;
  if (!r.tabs || typeof r.tabs !== 'object' || Array.isArray(r.tabs)) return null;
  const tabs = Object.entries(r.tabs);
  if (tabs.length > 64 || !tabs.every(([k, v]) => k.length <= 32 && day(v) && v <= r.days_used)) return null;
  if (!names(r.addons) || !names(r.features)) return null;
  return {
    version: r.version,
    os: r.os,
    installed: r.installed ?? 'unknown',
    days_used: String(r.days_used),
    tabs: tabs.filter(([k, v]) => COUNTS.tabs.has(k) && v > 0),
    addons: [...new Set(r.addons)].filter((a) => COUNTS.addons.has(a)),
    features: [...new Set(r.features)].filter((f) => COUNTS.features.has(f)),
  };
}

// The ISO week of `now`, as "2026-W45".
export function weekOf(now) {
  const d = new Date(now);
  const t = Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
  const weekday = (new Date(t).getUTCDay() + 6) % 7; // Monday 0
  const thursday = new Date(t + (3 - weekday) * 86_400_000);
  const year = thursday.getUTCFullYear();
  const first = Date.UTC(year, 0, 1);
  const n = Math.floor((thursday.getTime() - first) / 604_800_000) + 1;
  return `${year}-W${String(n).padStart(2, '0')}`;
}

const emptyWeek = (week) => ({
  v: 1, week, reports: 0, version: {}, os: {}, installed: {}, days_used: {}, tabs: {}, addons: {}, features: {},
});

function bump(map, key, n = 1) {
  map[key] = (map[key] || 0) + n;
}

export function createCounts({ dataDir, logError = () => {}, now = Date.now }) {
  const dir = path.join(dataDir, 'counts');
  let totals = null;
  let timer = null;

  const fileOf = (week) => path.join(dir, `${week}.json`);

  function load(week) {
    if (totals?.week === week) return totals;
    write();
    try {
      const t = JSON.parse(fs.readFileSync(fileOf(week), 'utf8'));
      if (t?.week === week) return (totals = t);
    } catch (e) {
      if (e.code !== 'ENOENT') logError(`counts: can't read ${week}: ${e.message}`);
    }
    return (totals = emptyWeek(week));
  }

  function write() {
    clearTimeout(timer);
    timer = null;
    if (!totals?.reports) return;
    try {
      fs.mkdirSync(dir, { recursive: true });
      const file = fileOf(totals.week);
      const tmp = `${file}.${process.pid}.tmp`;
      fs.writeFileSync(tmp, JSON.stringify(totals));
      fs.renameSync(tmp, file);
    } catch (e) {
      logError(`counts: can't write ${totals.week}: ${e.message}`);
    }
  }

  // Add a report to this week's totals. False when it isn't one.
  function add(report) {
    const t = now();
    const r = readReport(report, t);
    if (!r) return false;
    const w = load(weekOf(t));
    w.reports += 1;
    bump(w.version, r.version);
    bump(w.os, r.os);
    bump(w.installed, r.installed);
    bump(w.days_used, r.days_used);
    // Each tab: how many copies opened it on that many days of the week.
    for (const [tab, days] of r.tabs) bump((w.tabs[tab] ||= {}), String(days));
    for (const a of r.addons) bump(w.addons, a);
    for (const f of r.features) bump(w.features, f);
    if (!timer) {
      timer = setTimeout(write, COUNTS.writeMs);
      timer.unref();
    }
    return true;
  }

  return { add, flush: write };
}

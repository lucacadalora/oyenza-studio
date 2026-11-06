#!/usr/bin/env node
// Reads the week totals of the opt-in usage counts (docs/usage-counts.md).
//
//   node server/counts-view.mjs --data <dir> [--week 2026-W45] [--min 20]
//
// Prints the newest week, or the one named. Any number under --min reads
// "under 20", and a week with fewer reports than that shows nothing but its
// count: a group that small could be one person.

import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

export const MIN_GROUP = 20;

export function view(t, min = MIN_GROUP) {
  const shown = (n) => (n < min ? `under ${min}` : String(n));
  const lines = [`${t.week}: ${t.reports} reports`];
  if (t.reports < min) {
    lines.push(`Too few to show (under ${min}).`);
    return lines.join('\n');
  }
  const pct = (n) => (n < min ? '' : ` (${Math.round((n / t.reports) * 100)}%)`);
  const section = (title, map) => {
    const rows = Object.entries(map).sort((a, b) => b[1] - a[1]);
    if (!rows.length) return;
    lines.push('', title);
    for (const [k, n] of rows) lines.push(`  ${k}: ${shown(n)}${pct(n)}`);
  };
  section('Version', t.version);
  section('System', t.os);
  section('Installed in', t.installed);
  section('Days used this week', t.days_used);
  const tabs = Object.entries(t.tabs).map(([tab, byDays]) => {
    const n = Object.values(byDays).reduce((a, b) => a + b, 0);
    const days = Object.entries(byDays).reduce((a, [d, c]) => a + Number(d) * c, 0);
    return [tab, n, n ? days / n : 0];
  });
  if (tabs.length) {
    lines.push('', 'Tabs (copies that opened it, and average days)');
    for (const [tab, n, avg] of tabs.sort((a, b) => b[1] - a[1])) {
      lines.push(`  ${tab}: ${shown(n)}${pct(n)}${n < min ? '' : `, ${avg.toFixed(1)} days`}`);
    }
  }
  section('Add-ons on', t.addons);
  section('Features used', t.features);
  return lines.join('\n');
}

function main(argv) {
  const arg = (name) => {
    const i = argv.indexOf(name);
    return i < 0 ? undefined : argv[i + 1];
  };
  const data = arg('--data');
  if (!data) {
    console.error('usage: counts-view.mjs --data <dir> [--week 2026-W45] [--min 20]');
    return 2;
  }
  const dir = path.join(data, 'counts');
  const weeks = fs.existsSync(dir) ? fs.readdirSync(dir).filter((f) => /^\d{4}-W\d\d\.json$/.test(f)).sort() : [];
  const week = arg('--week') ? `${arg('--week')}.json` : weeks.at(-1);
  if (!week || !weeks.includes(week)) {
    console.log('No counts yet.');
    return 0;
  }
  const min = Number(arg('--min') ?? MIN_GROUP);
  console.log(view(JSON.parse(fs.readFileSync(path.join(dir, week), 'utf8')), min));
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) process.exit(main(process.argv.slice(2)));

// Visitor counts for studio.oyenza.com (Luca, 29 Oct 2026: "we need to track them all"): a copy of oyenza.com's own
// small sender (catat landing/track.js) with this site's ids. The site is on GitHub Pages, so it sends straight to our
// own servers: Umami "Oyenza Studio site" on analytics.vantis.sh and Databuddy "Oyenza Studio site" on data.oyenza.com
// (its collector at /ingest/). Both answer this host's cross-site requests.
//
// What is sent: the page, its title and language, where the visitor came from (referrer, utm_…), screen size, which
// link was tapped (what kind: the Windows or Mac download, GitHub, the Oyenza app, a section; and where on the page),
// how long the page stayed in front and how far down it was read, load speed and script errors. Never anything typed,
// never a cookie, nothing stored. Links to oyenza.app carry via=studio.oyenza.com (and ref + utm when counted), so an
// account made from here says so.
// Off on any host but studio.oyenza.com, for automated browsers, with Do Not Track / Global Privacy Control, and when
// localStorage has oyenza.notrack (our own devices).
(() => {
  const SITE = 'a319a2c7-b045-42a3-8334-fd8f159d7ad9';      // Umami
  const DB_SITE = '5f21ac14-cf50-4f8a-ae1b-b8ba4c67951a';   // Databuddy
  const HOSTS = new Set(['studio.oyenza.com']);
  const MAX = 300; // per page load and receiver: a stuck loop can never flood the counter

  // Links to the app say they came from here (`via`), and, when the visit may be counted, from where the visitor
  // reached this page (`ref` = that site's host name, plus its utm tags), so the app knows a Threads visitor who taps
  // "Open the app" came from Threads (the app's lib/arrival.ts). Added as the link is pressed, so the page's own
  // links stay clean. Done on any host, so a local preview shows it too.
  let carry = {};
  const tagLink = (e) => {
    try {
      const a = e.target instanceof Element ? e.target.closest('a[href]') : null;
      if (!a) return;
      const u = new URL(a.getAttribute('href') || '', location.href);
      if (u.host !== 'oyenza.app' || u.searchParams.has('via')) return;
      u.searchParams.set('via', 'studio.oyenza.com');
      for (const [k, v] of Object.entries(carry)) if (!u.searchParams.has(k)) u.searchParams.set(k, v);
      a.setAttribute('href', u.toString());
    } catch { /* an odd link: leave it */ }
  };
  for (const type of ['pointerdown', 'keydown', 'click']) document.addEventListener(type, tagLink, { capture: true, passive: true });

  try {
    if (!HOSTS.has(location.hostname) || navigator.webdriver || typeof fetch !== 'function') return;
    if (navigator.doNotTrack === '1' || window.doNotTrack === '1' || navigator.globalPrivacyControl === true) return;
    try { if (localStorage.getItem('oyenza.notrack')) return; } catch { /* storage blocked: count as usual */ }
  } catch { return; }

  const clip = (s, n) => String(s ?? '').replace(/\s+/g, ' ').trim().slice(0, n);
  const uuid = () => (crypto.randomUUID ? crypto.randomUUID() : '10000000-1000-4000-8000-100000000000'.replace(/[018]/g, (c) => (+c ^ (crypto.getRandomValues(new Uint8Array(1))[0] & (15 >> (+c / 4)))).toString(16)));
  const ids = { anonymousId: `anon_${uuid()}`, anonymizeVisitorIds: false, sessionId: `sess_${uuid()}` };
  const start = Date.now();
  const path = location.pathname;
  const pageUrl = `${location.origin}${path}`;
  const lang = () => document.documentElement.lang || navigator.language || '';

  // ---------------------------------------------------------------- Umami
  let sent = 0;
  let cache = ''; // Umami's token for this page load (memory only)
  // One at a time: in this Umami (2.14) a tap with data sent before the page's token came back fails with a 500.
  let line = Promise.resolve();
  function send(extra) {
    if (sent >= MAX) return;
    sent += 1;
    const payload = {
      website: SITE, hostname: location.hostname, language: lang(), screen: `${screen.width || 0}x${screen.height || 0}`,
      referrer: document.referrer || '', url: path, title: clip(document.title, 120), ...extra,
    };
    const body = JSON.stringify({ type: 'event', payload });
    line = line.then(() => {
      const headers = { 'Content-Type': 'application/json' };
      if (cache) headers['x-umami-cache'] = cache;
      return fetch('https://analytics.vantis.sh/api/send', { method: 'POST', headers, body, keepalive: true, credentials: 'omit' })
        .then((r) => (r.ok ? r.text() : ''))
        .then((t) => { if (t && t.length < 4000) cache = t; });
    }).catch(() => { /* counting never gets in the way */ });
  }

  // ---------------------------------------------------------------- Databuddy
  // Four small queues, one per collector route, sent together every few seconds and when the page is hidden.
  const ROUTES = { batch: 'client_id', track: 'website_id', vitals: 'client_id', errors: 'client_id' };
  const queues = { batch: [], track: [], vitals: [], errors: [] };
  let dbSent = 0;
  let timer = 0;
  function queue(kind, item) {
    if (dbSent >= MAX) return;
    dbSent += 1;
    queues[kind].push(item);
    if (!timer) timer = setTimeout(flush, 2500);
  }
  function flush(leaving = false) {
    clearTimeout(timer);
    timer = 0;
    for (const kind of Object.keys(queues)) {
      const items = queues[kind].splice(0);
      if (!items.length) continue;
      const url = `https://data.oyenza.com/ingest/${kind}?${ROUTES[kind]}=${DB_SITE}`;
      const body = JSON.stringify(items);
      try {
        // text/plain keeps a leaving beacon free of a preflight; the collector reads JSON from either
        if (leaving && navigator.sendBeacon?.(url, new Blob([body], { type: 'text/plain;charset=UTF-8' }))) continue;
        fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body, keepalive: true, credentials: 'omit' }).catch(() => {});
      } catch { /* counting never gets in the way */ }
    }
  }

  // Where this visit came from, for the links to the app (tagLink above): only once the visit may be counted.
  try {
    const q = new URLSearchParams(location.search);
    const from = document.referrer ? new URL(document.referrer) : null;
    if (from && /^https?:$/.test(from.protocol) && from.host !== location.host) carry.ref = from.hostname.replace(/^www\./, '');
    else if (q.get('ref')) carry.ref = clip(q.get('ref'), 100);
    for (const k of ['utm_source', 'utm_medium', 'utm_campaign']) if (q.get(k)) carry[k] = clip(q.get(k), 100);
  } catch { /* an odd address: the links say only `via` */ }

  const size = (w, h) => (w >= 240 && h >= 240 && w <= 10000 && h <= 10000 ? `${w}x${h}` : undefined);
  function arrival() {
    const a = {};
    try {
      const q = new URLSearchParams(location.search);
      for (const k of ['utm_source', 'utm_medium', 'utm_campaign', 'utm_term', 'utm_content', 'gclid']) {
        const v = q.get(k);
        if (v) a[k] = clip(v, 200);
      }
      const r = document.referrer ? new URL(document.referrer) : null;
      if (r && /^https?:$/.test(r.protocol) && r.host !== location.host) a.referrer = `${r.origin}${r.pathname}`;
    } catch { /* odd address: arrive as direct */ }
    return a;
  }
  function pageEvent(name, extra) {
    queue('batch', {
      eventId: uuid(), name, ...ids, timestamp: Date.now(), sessionStartTime: start, path: pageUrl, title: clip(document.title, 120),
      viewport_size: size(innerWidth, innerHeight), screen_resolution: size(screen.width || 0, screen.height || 0),
      language: lang() || undefined,
      timezone: (() => { try { return Intl.DateTimeFormat().resolvedOptions().timeZone; } catch { return undefined; } })(),
      connection_type: navigator.connection?.effectiveType || undefined,
      ...arrival(), ...extra,
    });
  }
  function event(name, data) {
    const d = {};
    for (const [k, v] of Object.entries(data)) if (v != null && v !== '') d[k] = typeof v === 'number' || typeof v === 'boolean' ? v : clip(v, 80);
    send({ name: clip(name, 50), data: d });
    taps += 1;
    queue('track', { eventId: uuid(), name: clip(name, 50), timestamp: Date.now(), path: pageUrl, properties: d, ...ids, websiteId: DB_SITE, source: 'browser' });
  }

  // ---------------------------------------------------------------- the page
  let taps = 0;
  let frontMs = 0;
  let frontSince = document.hidden ? 0 : Date.now();
  let deepest = 0;
  const frontNow = () => frontMs + (frontSince ? Date.now() - frontSince : 0);
  const readTo = () => {
    const h = document.documentElement.scrollHeight - innerHeight;
    deepest = Math.max(deepest, h > 0 ? Math.min(100, Math.round((scrollY / h) * 100)) : 100);
  };
  addEventListener('scroll', readTo, { passive: true });

  send({ url: `${path}${clip(location.search, 250)}` });
  pageEvent('screen_view', { page_count: 1 });
  flush(); // at once: many visits to a landing page are shorter than the queue's few seconds

  // Which link was tapped: what kind of place it leads to, and where on the page it sits. Never the link's text.
  const kind = (a) => {
    const href = a.getAttribute('href') || '';
    if (href.startsWith('mailto:')) return { to: 'email' };
    let u;
    try { u = new URL(href, location.href); } catch { return null; }
    if (/\.exe$/i.test(u.pathname)) return { to: 'download', target: 'windows' };
    if (/\.dmg$/i.test(u.pathname)) return { to: 'download', target: 'mac' };
    if (u.host === 'github.com') return { to: 'github', target: clip(u.pathname.split('/').slice(3, 4).join('') || 'repo', 20) };
    if (u.host === 'oyenza.app') return { to: 'app' };
    if (u.host !== location.host) return /^https?:$/.test(u.protocol) ? { to: 'out', target: u.host } : null;
    if (u.pathname === path && u.hash) return { to: 'section', target: clip(u.hash.slice(1), 40) };
    return { to: 'page', target: clip(u.pathname.replace(/\/+$/, '') || '/', 80) };
  };
  const where = (el) => {
    const r = el.closest('.float-cta, .site-menu, .site-header, .site-footer, section[id], article');
    if (!r) return 'page';
    if (r.matches('.float-cta')) return 'float';
    if (r.matches('.site-menu')) return 'menu';
    if (r.matches('.site-header')) return 'header';
    if (r.matches('.site-footer')) return 'footer';
    if (r.matches('article')) return 'article';
    return r.id === 'top' ? 'hero' : clip(r.id, 30);
  };
  document.addEventListener('click', (e) => {
    try {
      const t = e.target instanceof Element ? e.target : null;
      const toggle = t?.closest('[data-lang-toggle]');
      if (toggle) { event('lang', { to: lang() === 'en' ? 'id' : 'en', where: where(toggle) }); return; }
      const a = t?.closest('a[href]');
      const k = a && kind(a);
      if (k) { event('tap', { ...k, where: where(a) }); flush(); } // at once: most taps leave the page
    } catch { /* counting never gets in the way */ }
  }, { capture: true, passive: true });

  // How long the page stayed in front, how far down it was read, how many taps: once, as the page goes.
  let left = false;
  const leave = () => {
    if (left) return;
    left = true;
    readTo();
    pageEvent('page_exit', { time_on_page: Math.min(86400, Math.round(frontNow() / 1000)), scroll_depth: deepest, interaction_count: Math.min(taps, 9999), page_count: 1 });
  };
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) { frontSince = Date.now(); return; }
    frontMs = frontNow();
    frontSince = 0;
    flush(true);
  });
  addEventListener('pagehide', () => { leave(); flush(true); });

  // Script errors (ours, not a browser extension's): the message and where, never page content.
  let errors = 0;
  const fault = (message, more = {}) => {
    if (errors >= 10 || !message || /^Script error\.?$/.test(message)) return;
    if (/(chrome|moz|safari(-web)?)-extension:/.test(`${more.filename || ''}${more.stack || ''}`)) return;
    errors += 1;
    queue('errors', { eventId: uuid(), timestamp: Date.now(), path, message: clip(message, 500), ...more, ...ids });
  };
  addEventListener('error', (e) => fault(e.message, {
    filename: clip(e.filename, 300) || undefined, lineno: e.lineno || undefined, colno: e.colno || undefined,
    stack: e.error?.stack ? String(e.error.stack).slice(0, 2000) : undefined, errorType: e.error?.name || 'Error',
  }));
  addEventListener('unhandledrejection', (e) => fault(String(e.reason?.message || e.reason || ''), {
    stack: e.reason?.stack ? String(e.reason.stack).slice(0, 2000) : undefined, errorType: e.reason?.name || 'UnhandledRejection',
  }));

  // Load speed, once per page load: first byte, first paint, largest paint, layout shift.
  const vital = (metricName, metricValue) => queue('vitals', { eventId: uuid(), timestamp: Date.now(), path, metricName, metricValue, ...ids });
  try {
    const nav = performance.getEntriesByType('navigation')[0];
    if (nav?.responseStart > 0) vital('TTFB', Math.round(nav.responseStart));
    const watch = (type, fn) => { try { new PerformanceObserver((l) => fn(l.getEntries())).observe({ type, buffered: true }); } catch { /* not in this browser */ } };
    let fcp = false;
    watch('paint', (es) => { for (const e of es) if (!fcp && e.name === 'first-contentful-paint') { fcp = true; vital('FCP', Math.round(e.startTime)); } });
    let lcp = 0;
    let cls = 0;
    watch('largest-contentful-paint', (es) => { lcp = es[es.length - 1]?.startTime || lcp; });
    watch('layout-shift', (es) => { for (const e of es) if (!e.hadRecentInput) cls += e.value; });
    let told = false;
    document.addEventListener('visibilitychange', () => {
      if (told || !document.hidden) return;
      told = true;
      if (lcp) vital('LCP', Math.round(lcp));
      vital('CLS', Math.round(cls * 1000) / 1000);
      flush(true);
    }, true);
  } catch { /* no timing API */ }
})();
